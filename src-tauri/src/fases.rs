//! Métricas por fase da missão (X1.6): planejamento, execução, verificação e correções, com o tempo de parede, os
//! tokens e o custo de cada uma. Tokens e custo são o delta do painel entre a abertura e o fechamento da fase (a
//! base fica na memória; se o app reiniciar com a fase aberta, ela fecha com zero de tokens).
use crate::db::FaseRow;
use crate::error::AppResult;
use crate::runtime::AppState;
use crate::util::{lk, now_ms};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tauri::State;

pub const PLANEJAMENTO: &str = "planejamento";
pub const EXECUCAO: &str = "execucao";
pub const VERIFICACAO: &str = "verificacao";
pub const CORRECOES: &str = "correcoes";
const FASES: [&str; 4] = [PLANEJAMENTO, EXECUCAO, VERIFICACAO, CORRECOES];

/// Base de cada fase aberta: (painel medido, tok_in, tok_out, custo) na abertura. Chave: id da linha.
type Base = (Option<String>, i64, i64, f64);
static BASES: LazyLock<Mutex<HashMap<i64, Base>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Tokens e custo acumulados do painel agora (zero sem painel).
fn medida(s: &AppState, pane: Option<&str>) -> (i64, i64, f64) {
    pane.and_then(|p| s.db.get_pane(p).ok()).map_or((0, 0, 0.0), |r| (r.tok_in, r.tok_out, r.cost_usd))
}

/// Abre uma fase da missão (com o painel medido, se houver). Falha de gravação vira log: métrica nunca para o plano.
pub fn abrir(s: &AppState, missao: &str, pane: Option<&str>, task: Option<&str>, fase: &str) {
    let (ti, to, c) = medida(s, pane);
    match s.db.fase_abrir(missao, pane, task, fase, now_ms()) {
        Ok(id) => {
            lk(&BASES).insert(id, (pane.map(String::from), ti, to, c));
        }
        Err(e) => tracing::warn!("fase {fase} da missão {missao} não aberta: {}", e.code()),
    }
}

/// Fecha a fase aberta mais recente de (missão, tarefa, fase), com o delta de tokens e custo. Sem fase aberta: nada.
pub fn fechar(s: &AppState, missao: &str, task: Option<&str>, fase: &str) {
    let Ok(Some(id)) = s.db.fase_aberta(missao, task, fase) else { return };
    fechar_linha(s, id, missao, fase);
}

/// Fecha uma linha de fase aberta: o delta de tokens e custo desde a abertura (a base sai da memória).
fn fechar_linha(s: &AppState, id: i64, missao: &str, fase: &str) {
    let (painel, ti, to, c) = lk(&BASES).remove(&id).unwrap_or((None, 0, 0, 0.0));
    let (ni, no, nc) = medida(s, painel.as_deref());
    if let Err(e) = s.db.fase_fechar(id, now_ms(), (ni - ti).max(0), (no - to).max(0), (nc - c).max(0.0)) {
        tracing::warn!("fase {fase} da missão {missao} não fechada: {}", e.code());
    }
}

/// Fecha todas as fases abertas de uma tarefa (bloqueada, executor fechou): a tarefa não fica com fase em curso.
pub fn fechar_tarefa(s: &AppState, missao: &str, tarefa: &str) {
    fechar_abertas(s, missao, |r| r.task_id.as_deref() == Some(tarefa));
}

/// Fecha todas as fases abertas da missão (arquivada): nenhuma linha fica em curso.
pub fn fechar_missao(s: &AppState, missao: &str) {
    fechar_abertas(s, missao, |_| true);
}

fn fechar_abertas(s: &AppState, missao: &str, quer: impl Fn(&FaseRow) -> bool) {
    let Ok(rows) = s.db.fases_da_missao(missao) else { return };
    for r in rows.iter().filter(|r| r.ended_at.is_none() && quer(r)) {
        fechar_linha(s, r.id, missao, &r.phase);
    }
}

fn dur(r: &FaseRow, agora: i64) -> i64 {
    // fase ainda aberta conta até agora; intervalo invertido (relógio) não passa de zero
    (r.ended_at.unwrap_or(agora) - r.started_at).max(0)
}

/// Agregado para a tela: uma barra por fase (tempo, tokens, custo), a tarefa mais lenta (execução + correções) e
/// tempo de testes × tempo de código. Função pura: os testes usam linhas sintéticas.
pub fn agregar(rows: &[FaseRow], agora: i64) -> Value {
    let por_fase: Vec<Value> = FASES
        .iter()
        .map(|f| {
            let rs: Vec<&FaseRow> = rows.iter().filter(|r| r.phase == *f).collect();
            json!({
                "phase": f,
                "wall_ms": rs.iter().map(|r| dur(r, agora)).sum::<i64>(),
                "tok_in": rs.iter().map(|r| r.tok_in).sum::<i64>(),
                "tok_out": rs.iter().map(|r| r.tok_out).sum::<i64>(),
                "cost_usd": rs.iter().map(|r| r.cost_usd).sum::<f64>(),
            })
        })
        .collect();
    let mut por_tarefa: HashMap<&str, i64> = HashMap::new();
    for r in rows.iter().filter(|r| r.phase == EXECUCAO || r.phase == CORRECOES) {
        if let Some(t) = &r.task_id {
            *por_tarefa.entry(t.as_str()).or_default() += dur(r, agora);
        }
    }
    // empate: a de id menor, para a resposta não mudar entre chamadas
    let mais_lenta = por_tarefa.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map_or(Value::Null, |(t, ms)| json!({"task_id": t, "wall_ms": ms}));
    let soma = |fase: &str| rows.iter().filter(|r| r.phase == fase).map(|r| dur(r, agora)).sum::<i64>();
    // a fase em curso (a aberta mais nova), se houver: o chip da barra mostra esta
    let atual = rows.iter().filter(|r| r.ended_at.is_none()).max_by_key(|r| r.id).map_or(Value::Null, |r| json!({"phase": r.phase, "started_at": r.started_at}));
    json!({
        "atual": atual,
        "per_phase": por_fase,
        "slowest_task": mais_lenta,
        "test_ms": soma(VERIFICACAO),
        "code_ms": soma(EXECUCAO) + soma(CORRECOES),
    })
}

/// Desempenho da missão (tela "Desempenho da missão").
#[tauri::command(async)]
pub fn mission_phases(s: State<'_, Arc<AppState>>, mission_id: String) -> AppResult<Value> {
    let rows = s.db.fases_da_missao(&mission_id)?;
    Ok(agregar(&rows, now_ms()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linha(fase: &str, task: Option<&str>, ini: i64, fim: Option<i64>, tok: (i64, i64, f64)) -> FaseRow {
        FaseRow { id: 0, mission_id: "m".into(), pane_id: None, task_id: task.map(String::from), phase: fase.into(), started_at: ini, ended_at: fim, tok_in: tok.0, tok_out: tok.1, cost_usd: tok.2 }
    }

    fn fase<'a>(v: &'a Value, nome: &str) -> &'a Value {
        v["per_phase"].as_array().unwrap().iter().find(|x| x["phase"] == nome).unwrap()
    }

    #[test]
    fn sem_linhas_tudo_zero_e_sem_tarefa() {
        let v = agregar(&[], 1000);
        assert_eq!(fase(&v, EXECUCAO)["wall_ms"], 0);
        assert!(v["slowest_task"].is_null());
        assert_eq!((v["test_ms"].as_i64(), v["code_ms"].as_i64()), (Some(0), Some(0)));
    }

    #[test]
    fn fase_aberta_conta_ate_agora_e_relogio_invertido_nao_passa_de_zero() {
        let rows = [linha(PLANEJAMENTO, None, 100, None, (0, 0, 0.0)), linha(EXECUCAO, Some("m-A"), 500, Some(300), (0, 0, 0.0))];
        let v = agregar(&rows, 1100);
        assert_eq!(fase(&v, PLANEJAMENTO)["wall_ms"], 1000);
        assert_eq!(fase(&v, EXECUCAO)["wall_ms"], 0);
    }

    #[test]
    fn tokens_e_custo_somam_por_fase() {
        let rows = [linha(EXECUCAO, Some("m-A"), 0, Some(10), (5, 7, 0.25)), linha(EXECUCAO, Some("m-B"), 0, Some(10), (1, 2, 0.5))];
        let v = agregar(&rows, 100);
        let e = fase(&v, EXECUCAO);
        assert_eq!((e["tok_in"].as_i64(), e["tok_out"].as_i64()), (Some(6), Some(9)));
        assert!((e["cost_usd"].as_f64().unwrap() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn tarefa_mais_lenta_soma_execucao_e_correcoes_e_testes_contam_separado() {
        let rows = [
            linha(EXECUCAO, Some("m-A"), 0, Some(100), (0, 0, 0.0)),
            linha(CORRECOES, Some("m-A"), 100, Some(400), (0, 0, 0.0)), // A: 400 ms no total
            linha(EXECUCAO, Some("m-B"), 0, Some(300), (0, 0, 0.0)),
            linha(VERIFICACAO, None, 300, Some(350), (0, 0, 0.0)),
        ];
        let v = agregar(&rows, 1000);
        assert_eq!(v["slowest_task"]["task_id"], "m-A");
        assert_eq!(v["slowest_task"]["wall_ms"], 400);
        assert_eq!(v["test_ms"], 50);
        assert_eq!(v["code_ms"], 700, "execução 400 + correções 300");
    }

    #[test]
    fn fase_em_curso_aparece_como_atual() {
        let rows = [linha(PLANEJAMENTO, None, 0, Some(5), (0, 0, 0.0)), linha(EXECUCAO, Some("m-A"), 10, None, (0, 0, 0.0))];
        let v = agregar(&rows, 100);
        assert_eq!(v["atual"]["phase"], EXECUCAO);
        assert_eq!(v["atual"]["started_at"], 10);
        assert!(agregar(&[], 0)["atual"].is_null());
    }

    #[test]
    fn fechar_tarefa_e_missao_nao_deixam_fase_em_curso() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        s.db.fase_abrir("m", None, Some("m-A"), EXECUCAO, 10).unwrap();
        s.db.fase_abrir("m", None, Some("m-B"), EXECUCAO, 10).unwrap();
        fechar_tarefa(&s, "m", "m-A");
        let abertas = |s: &AppState| s.db.fases_da_missao("m").unwrap().iter().filter(|r| r.ended_at.is_none()).count();
        assert_eq!(abertas(&s), 1, "só a tarefa A fechou");
        fechar_missao(&s, "m");
        assert_eq!(abertas(&s), 0);
    }

    #[test]
    fn empate_de_tarefa_nao_muda_entre_chamadas() {
        let rows = [linha(EXECUCAO, Some("m-B"), 0, Some(10), (0, 0, 0.0)), linha(EXECUCAO, Some("m-A"), 0, Some(10), (0, 0, 0.0))];
        assert_eq!(agregar(&rows, 100)["slowest_task"]["task_id"], "m-A");
    }
}
