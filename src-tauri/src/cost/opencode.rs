//! Custo do OpenCode (anomalyco/opencode). Ele grava as sessões num SQLite, `opencode.db`, em
//! `~/.local/share/opencode` (também no Windows; `XDG_DATA_HOME` vale se definido). A tabela `session` já traz o
//! custo e os tokens acumulados da sessão (`cost`, `tokens_input`, `tokens_output`, `tokens_reasoning`,
//! `tokens_cache_read`, `tokens_cache_write`). Só leitura: o banco é aberto com SQLITE_OPEN_READ_ONLY.
//!
//! O `Tailer` guarda um caminho por painel e usa esse caminho para não dar a mesma sessão a dois painéis
//! (`claimed`). Aqui o caminho é sintético: `<opencode.db>/<id da sessão>` (nunca é aberto como arquivo).

use super::prices::Prices;
use super::{CostSnapshot, Usage};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Pasta de dados do OpenCode.
pub fn data_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("share"))
        .join("opencode")
}

pub fn db_path(data: &Path) -> PathBuf {
    data.join("opencode.db")
}

/// Caminho sintético de uma sessão (ver o topo do arquivo).
pub fn session_path(db: &Path, session_id: &str) -> PathBuf {
    db.join(session_id)
}

fn open(db: &Path) -> Option<Connection> {
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    // o OpenCode escreve em WAL: espera um pouco em vez de falhar se ele estiver gravando
    c.busy_timeout(Duration::from_millis(500)).ok()?;
    Some(c)
}

/// `D:\Projetos\App\` e `d:/projetos/app` são a mesma pasta (o OpenCode grava com `/`).
fn norm(p: &str) -> String {
    p.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// A sessão principal (sem `parent_id`) mais antiga, criada a partir de `since`, na pasta `cwd` e ainda não
/// reivindicada por outro painel. None = ainda não existe (o OpenCode só cria a sessão no primeiro pedido).
// ponytail: dois painéis OpenCode na mesma pasta abertos quase juntos podem trocar de sessão; comparar o horário do primeiro pedido se acontecer.
pub fn find_session(db: &Path, cwd: &str, since: SystemTime, claimed: &HashSet<PathBuf>) -> Option<PathBuf> {
    let c = open(db)?;
    let since_ms = since.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    let want = norm(cwd);
    let mut st = c.prepare("SELECT id, directory FROM session WHERE parent_id IS NULL AND time_created >= ?1 ORDER BY time_created ASC, id ASC").ok()?;
    let rows = st.query_map(params![since_ms], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?;
    let found = rows.flatten().find(|(id, dir)| norm(dir) == want && !claimed.contains(&session_path(db, id))).map(|(id, _)| session_path(db, &id));
    found
}

/// Totais da sessão. None = banco ou sessão ilegível (tenta de novo na próxima leitura).
/// Sem custo gravado (0) mas com tokens, estima pelo preço do modelo, como o Gemini.
pub fn snapshot(db: &Path, session_id: &str, prices: &Prices) -> Option<CostSnapshot> {
    let c = open(db)?;
    c.query_row(
        "SELECT cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write, CASE WHEN json_valid(model) THEN json_extract(model, '$.id') END FROM session WHERE id = ?1",
        params![session_id],
        |r| {
            // f64 lê INTEGER e REAL; valor ausente ou negativo conta como 0
            let n = |i: usize| r.get::<_, f64>(i).unwrap_or(0.0).max(0.0) as u64;
            let model = r.get::<_, Option<String>>(6).unwrap_or(None).filter(|m| !m.is_empty());
            let usage = Usage { input: n(1), output: n(2) + n(3), cache_read: n(4), cache_write: n(5), ..Default::default() };
            let mut cost = r.get::<_, f64>(0).unwrap_or(0.0).max(0.0);
            let tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
            if cost == 0.0 && tokens > 0 {
                cost = model.as_deref().map_or(0.0, |m| prices.cost(m, &usage));
            }
            Ok(CostSnapshot { usage, cost_usd: cost, model, ..Default::default() })
        },
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PRICES_DEFAULT;

    /// Banco no esquema confirmado em packages/core/src/session/sql.ts (tag v1.19.3), só as colunas usadas.
    fn banco() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let db = db_path(d.path());
        let c = Connection::open(&db).unwrap();
        c.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, model TEXT, time_created INTEGER NOT NULL,
               cost REAL NOT NULL DEFAULT 0, tokens_input INTEGER NOT NULL DEFAULT 0, tokens_output INTEGER NOT NULL DEFAULT 0,
               tokens_reasoning INTEGER NOT NULL DEFAULT 0, tokens_cache_read INTEGER NOT NULL DEFAULT 0, tokens_cache_write INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        (d, db)
    }

    fn sessao(db: &Path, id: &str, parent: Option<&str>, dir: &str, created: i64, cost: f64, tokens: [i64; 5]) {
        Connection::open(db)
            .unwrap()
            .execute(
                "INSERT INTO session (id, parent_id, directory, model, time_created, cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write)
                 VALUES (?1, ?2, ?3, '{\"id\":\"claude-sonnet-4-5\",\"providerID\":\"anthropic\"}', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![id, parent, dir, created, cost, tokens[0], tokens[1], tokens[2], tokens[3], tokens[4]],
            )
            .unwrap();
    }

    fn prices() -> Prices {
        serde_json::from_str(PRICES_DEFAULT).unwrap()
    }

    fn ms(t: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(t as u64)
    }

    #[test]
    fn model_fora_do_formato_json_nao_derruba_o_custo() {
        let (_d, db) = banco();
        sessao(&db, "ses_b", None, "D:/proj", 1000, 0.10, [10, 5, 0, 0, 0]);
        Connection::open(&db).unwrap().execute("UPDATE session SET model = 'texto-solto' WHERE id = 'ses_b'", []).unwrap();
        let s = snapshot(&db, "ses_b", &prices()).expect("o custo continua lido");
        assert!((s.cost_usd - 0.10).abs() < 1e-9);
        assert_eq!(s.model, None);
    }

    #[test]
    fn snapshot_uses_the_cost_recorded_by_opencode() {
        let (_d, db) = banco();
        sessao(&db, "ses_a", None, "D:/proj", 1000, 0.42, [1000, 200, 50, 300, 10]);
        let s = snapshot(&db, "ses_a", &prices()).unwrap();
        assert_eq!(s.usage, Usage { input: 1000, output: 250, cache_read: 300, cache_write: 10, ..Default::default() });
        assert!((s.cost_usd - 0.42).abs() < 1e-9, "{}", s.cost_usd);
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-5"));
    }

    #[test]
    fn zero_cost_with_tokens_is_estimated_from_the_price_table() {
        let (_d, db) = banco();
        sessao(&db, "ses_a", None, "D:/proj", 1000, 0.0, [1_000_000, 0, 0, 0, 0]);
        let s = snapshot(&db, "ses_a", &prices()).unwrap();
        assert!(s.cost_usd > 0.0, "{}", s.cost_usd);
        // sem tokens e sem custo: zero mesmo
        sessao(&db, "ses_b", None, "D:/proj", 2000, 0.0, [0; 5]);
        assert_eq!(snapshot(&db, "ses_b", &prices()).unwrap().cost_usd, 0.0);
    }

    #[test]
    fn unreadable_database_or_session_is_none() {
        let (d, db) = banco();
        assert!(snapshot(&db, "nao-existe", &prices()).is_none());
        assert!(snapshot(&d.path().join("sem-banco.db"), "x", &prices()).is_none());
        let torto = d.path().join("torto.db");
        std::fs::write(&torto, b"isso nao e sqlite").unwrap();
        assert!(snapshot(&torto, "x", &prices()).is_none());
        assert!(find_session(&torto, "D:/proj", ms(0), &HashSet::new()).is_none());
    }

    #[test]
    fn find_session_picks_the_oldest_unclaimed_main_session_in_the_folder() {
        let (_d, db) = banco();
        sessao(&db, "ses_velha", None, "D:/proj", 500, 0.0, [0; 5]); // antes do spawn
        sessao(&db, "ses_filha", Some("ses_b"), "D:/proj", 1100, 0.0, [0; 5]); // subsessão: ignorada
        sessao(&db, "ses_outra_pasta", None, "D:/outro", 1200, 0.0, [0; 5]);
        sessao(&db, "ses_b", None, "D:/proj", 1500, 0.0, [0; 5]);
        sessao(&db, "ses_c", None, "D:/proj", 1800, 0.0, [0; 5]);
        let none = HashSet::new();
        // o cwd do painel vem do Windows, com barra invertida e caixa diferente
        let primeira = find_session(&db, "d:\\Proj\\", ms(1000), &none).unwrap();
        assert_eq!(primeira, session_path(&db, "ses_b"));
        let claimed: HashSet<PathBuf> = [primeira].into();
        assert_eq!(find_session(&db, "D:/proj", ms(1000), &claimed).unwrap(), session_path(&db, "ses_c"));
        assert!(find_session(&db, "D:/nada", ms(1000), &none).is_none());
        assert!(find_session(&db, "D:/proj", ms(5000), &none).is_none());
    }

    #[test]
    fn data_dir_follows_xdg_or_home() {
        // sem mexer no ambiente do processo: só a forma padrão
        if std::env::var_os("XDG_DATA_HOME").is_none_or(|v| v.is_empty()) {
            assert_eq!(data_dir(Path::new("C:/Users/x")), Path::new("C:/Users/x").join(".local").join("share").join("opencode"));
        }
        assert_eq!(db_path(Path::new("a")), Path::new("a").join("opencode.db"));
    }
}
