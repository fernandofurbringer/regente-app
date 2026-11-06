//! Índice de código do Regente (RM-24, H1-05): símbolos por arquivo, em memória, atualizado a cada chamada.
//! Só lê; nunca grava no workspace. A raiz é o cwd do painel que chama, e nunca há índice em workspace sensível
//! (isso é checado pela ferramenta, não aqui).

pub mod outline;
pub mod search;

use crate::files::IGNORED;
use crate::util::lk;
use ignore::WalkBuilder;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Arquivos examinados numa passada, e tempo da passada: a montagem fria para aqui e devolve parcial.
pub const MAX_FILES: usize = 5000;
pub const TEMPO_MAX: Duration = Duration::from_secs(2);
/// Arquivo acima disso não é lido (nem indexado, nem pesquisado).
pub const ARQUIVO_MAX: u64 = 1024 * 1024;

pub struct Entrada {
    /// (mtime em nanos, tamanho): se não mudou, o arquivo não é lido de novo.
    pub key: (u128, u64),
    pub simbolos: Vec<outline::Symbol>,
}

#[derive(Default)]
pub struct Index {
    /// Caminho relativo (com `/`) -> símbolos, só das linguagens com índice.
    pub arquivos: BTreeMap<String, Entrada>,
    pub atualizado_ms: i64,
    /// A última passada parou no teto (arquivos ou tempo): o índice está incompleto.
    pub parcial: bool,
}

// ponytail: sem limite de raízes em memória; o uso típico é um workspace por vez.
static INDICES: OnceLock<Mutex<HashMap<PathBuf, Index>>> = OnceLock::new();

/// Walker com as regras de busca: .gitignore, ocultos de fora, pastas de `IGNORED` de fora, ordem por nome.
pub fn caminhar(raiz: &Path) -> ignore::Walk {
    WalkBuilder::new(raiz).require_git(false).sort_by_file_name(|a, b| a.cmp(b)).filter_entry(|e| !e.file_name().to_str().is_some_and(|n| IGNORED.contains(&n)) && !(e.depth() > 0 && e.metadata().is_ok_and(|m| crate::projeto::e_link(&m)))).build()
}

/// Caminho relativo com `/`, a partir da raiz.
pub(crate) fn rel_de(raiz: &Path, p: &Path) -> Option<String> {
    p.strip_prefix(raiz).ok().map(|r| r.to_string_lossy().replace('\\', "/"))
}

/// Tira a raiz do cache (workspace que virou sensível): o índice guardado não pode mais ser lido.
pub fn esquecer(raiz: &Path) {
    let mapa = INDICES.get_or_init(|| Mutex::new(HashMap::new()));
    lk(mapa).remove(raiz);
}

/// Workspace sensível que é a própria raiz, está dentro dela ou a contém: a raiz não pode alcançá-lo.
/// Compara caminhos canônicos (resolve nomes 8.3 e junções); sem canônico, cai no texto normalizado.
pub fn toca_sensivel(s: &crate::runtime::AppState, raiz: &Path) -> bool {
    let ids = lk(&s.settings).sensitive_workspaces.clone();
    let norm = |p: &Path| {
        let c = std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string_lossy().into_owned());
        c.trim_start_matches(r"\\?\").trim_end_matches(|c| c == '\\' || c == '/').to_lowercase().replace('/', "\\")
    };
    let r = norm(raiz);
    ids.iter().filter_map(|id| s.db.get_workspace(id).ok()).any(|w| {
        let p = norm(Path::new(&w.path));
        r == p || r.starts_with(&format!("{p}\\")) || p.starts_with(&format!("{r}\\"))
    })
}

/// Atualiza o índice da raiz e roda `f` sobre ele (com o lock segurado: a ferramenta já roda fora do runtime async).
// ponytail: one global lock held across the walk; per-root Arc<Mutex<Index>> or build outside the lock if panes contend
pub fn com_indice<T>(raiz: &Path, f: impl FnOnce(&Index) -> T) -> T {
    let mapa = INDICES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut m = lk(mapa);
    let idx = m.entry(raiz.to_path_buf()).or_default();
    atualizar(idx, raiz, MAX_FILES, TEMPO_MAX);
    f(idx)
}

/// Roda `f` sobre o índice já guardado, sem atualizar nem construir (estatísticas baratas).
pub fn em_cache<T>(raiz: &Path, f: impl FnOnce(&Index) -> T) -> Option<T> {
    let mapa = INDICES.get_or_init(|| Mutex::new(HashMap::new()));
    lk(mapa).get(raiz).map(f)
}

/// Passada incremental. Arquivos que sumiram são tirados, a não ser que a passada tenha parado no teto.
/// O teto de `max` conta só os arquivos lidos e analisados (cache miss); o de tempo vale para a caminhada toda.
pub fn atualizar(idx: &mut Index, raiz: &Path, max: usize, tempo: Duration) {
    let inicio = Instant::now();
    let mut vistos = HashSet::new();
    let mut analisados = 0usize;
    idx.parcial = false;
    for item in caminhar(raiz) {
        let Ok(e) = item else { continue };
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if inicio.elapsed() > tempo {
            idx.parcial = true;
            break;
        }
        let path = e.path();
        let Some(rel) = rel_de(raiz, path) else { continue };
        let Some(lingua) = path.extension().and_then(|x| x.to_str()).and_then(outline::lingua_da_extensao) else { continue };
        let Ok(meta) = e.metadata() else { continue };
        if meta.len() > ARQUIVO_MAX {
            continue;
        }
        let key = (mtime_ns(&meta), meta.len());
        vistos.insert(rel.clone());
        if idx.arquivos.get(&rel).is_some_and(|x| x.key == key) {
            continue;
        }
        if analisados >= max {
            idx.parcial = true;
            break;
        }
        analisados += 1;
        // não UTF-8 = não é código que a gente indexe
        let Ok(texto) = std::fs::read_to_string(path) else { continue };
        let simbolos = outline::simbolos(lingua, &texto);
        idx.arquivos.insert(rel, Entrada { key, simbolos });
    }
    if !idx.parcial {
        idx.arquivos.retain(|k, _| vistos.contains(k));
    }
    idx.atualizado_ms = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
}

fn mtime_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos())
}

/// (arquivos com símbolos, símbolos no total) do índice.
pub fn estatisticas(idx: &Index) -> (usize, usize) {
    (idx.arquivos.len(), idx.arquivos.values().map(|e| e.simbolos.len()).sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, texto: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, texto).unwrap();
    }

    fn sem_pressa(idx: &mut Index, raiz: &Path) {
        atualizar(idx, raiz, MAX_FILES, Duration::from_secs(60));
    }

    #[test]
    fn gitignored_and_ignored_dirs_are_not_indexed() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), ".gitignore", "segredo/\n");
        put(d.path(), "a.rs", "fn a() {}\n");
        put(d.path(), "segredo/b.rs", "fn b() {}\n");
        put(d.path(), "node_modules/c.js", "function c() {}\n");
        let mut idx = Index::default();
        sem_pressa(&mut idx, d.path());
        assert_eq!(idx.arquivos.keys().collect::<Vec<_>>(), vec!["a.rs"]);
    }

    #[test]
    fn second_pass_skips_unchanged_and_prunes_deleted() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "a.rs", "fn a() {}\n");
        put(d.path(), "b.rs", "fn b() {}\n");
        let mut idx = Index::default();
        sem_pressa(&mut idx, d.path());
        assert_eq!(estatisticas(&idx), (2, 2));
        std::fs::remove_file(d.path().join("b.rs")).unwrap();
        put(d.path(), "a.rs", "fn a() {}\nfn a2() {}\n");
        sem_pressa(&mut idx, d.path());
        assert_eq!(estatisticas(&idx), (1, 2), "b sumiu, a mudou");
    }

    #[test]
    fn cold_build_stops_at_the_file_cap_and_says_so() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..10 {
            put(d.path(), &format!("f{i}.rs"), "fn x() {}\n");
        }
        let mut idx = Index::default();
        atualizar(&mut idx, d.path(), 4, Duration::from_secs(60));
        assert!(idx.parcial);
        assert!(idx.arquivos.len() <= 4);
    }

    #[test]
    fn cap_counts_only_parsed_files_so_code_after_non_code_is_indexed() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5000 {
            put(d.path(), &format!("a/n{i:04}.txt"), "texto que não é código\n");
        }
        for i in 0..3 {
            put(d.path(), &format!("b/c{i}.rs"), "fn c() {}\n");
        }
        let mut idx = Index::default();
        atualizar(&mut idx, d.path(), MAX_FILES, Duration::from_secs(60));
        assert!(!idx.parcial);
        assert_eq!(idx.arquivos.len(), 3);
    }

    /// 5000 arquivos pequenos na montagem fria, com os arquivos já em cache (o primeiro acesso no Windows, com antivírus, custa mais).
    #[test]
    #[ignore = "perf: rodar com --release --ignored --nocapture"]
    fn five_thousand_small_files_under_the_cap() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5000 {
            put(d.path(), &format!("src/m{}/f{i}.rs", i % 50), &format!("pub fn f{i}() -> u32 {{ {i} }}\npub struct S{i};\n"));
        }
        // aquecimento: primeiro acesso a arquivo recém-criado é mais lento no Windows
        for it in caminhar(d.path()).flatten() {
            if it.file_type().is_some_and(|t| t.is_file()) {
                let _ = std::fs::read(it.path());
            }
        }
        let mut idx = Index::default();
        let t = Instant::now();
        atualizar(&mut idx, d.path(), MAX_FILES, TEMPO_MAX);
        let ms = t.elapsed().as_millis();
        eprintln!("5000 arquivos: {ms} ms, parcial={}, {:?}", idx.parcial, estatisticas(&idx));
        assert!(ms < 2000 && !idx.parcial, "{ms} ms");
    }
}
