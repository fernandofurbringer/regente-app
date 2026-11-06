//! Watcher de arquivos (crate `notify`): a árvore de arquivos e a branch do rodapé mudam na hora quando algo muda por
//! fora (agente, Explorer, terminal), sem esperar o ⟳ nem o polling.
//!
//! A árvore registra a raiz que mostra (`fs_watch` / `fs_unwatch`, contados por raiz). Cada raiz é vigiada com recursão;
//! a pasta do git dela (`.git`, ou a de worktree que fica fora da raiz) também, para o `HEAD`. Os eventos são juntados
//! por raiz e saem no máximo uma vez a cada `FLUSH` como `fs-changed {root, dirs}` (as pastas cujo conteúdo mudou);
//! uma troca de branch sai como `git-head {root}`. Pastas ignoradas (`files::IGNORED`) não geram evento de árvore.
//! Workspace remoto não é vigiado (continua no polling).

use crate::runtime::AppState;
use crate::util::lk;
use ::notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Janela que junta os eventos de uma raiz (um build gera centenas por segundo).
const FLUSH: Duration = Duration::from_millis(300);
/// Pastas por evento: acima disso a árvore relê tudo o que está aberto (`dirs` vazio).
const DIRS_MAX: usize = 64;

struct Raiz {
    /// a raiz como a árvore mandou (volta igual no evento)
    original: String,
    contagem: usize,
    /// pasta do git fora da raiz (worktree, ou workspace numa subpasta do repositório): vigiada sem recursão
    gitdir_fora: Option<PathBuf>,
}

#[derive(Default)]
struct Estado {
    watcher: Option<RecommendedWatcher>,
    /// chave: a raiz em minúsculas com `\`
    raizes: HashMap<String, Raiz>,
    /// pasta do git (minúsculas) → raízes que dependem dela
    gitdirs: HashMap<String, BTreeSet<String>>,
}

static ESTADO: OnceLock<Mutex<Estado>> = OnceLock::new();

fn estado() -> &'static Mutex<Estado> {
    ESTADO.get_or_init(Default::default)
}

/// Chave de comparação de caminho no Windows: minúsculas, `\`, sem `\` no fim.
pub fn chave(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// A pasta do git de `dir` (sobe até achar `.git`; arquivo `gitdir:` de worktree resolve para a pasta de verdade).
pub fn git_dir_of(dir: &Path) -> Option<PathBuf> {
    let mut at = Some(dir);
    while let Some(p) = at {
        let g = p.join(".git");
        if g.is_dir() {
            return Some(g);
        }
        if g.is_file() {
            let s = std::fs::read_to_string(&g).ok()?;
            return Some(p.join(s.trim().strip_prefix("gitdir:")?.trim()));
        }
        at = p.parent();
    }
    None
}

/// Para onde vai um caminho de evento: a raiz dona (a mais funda que o contém) e se ele conta para a árvore.
/// Caminho dentro de pasta ignorada (`.git`, `node_modules`, `target`…) não conta.
pub fn classificar<'a>(raizes: impl Iterator<Item = &'a String>, p: &str) -> Option<(&'a String, bool)> {
    let dona = raizes.filter(|r| p == r.as_str() || p.starts_with(&format!("{r}\\"))).max_by_key(|r| r.len())?;
    let resto = p[dona.len()..].trim_start_matches('\\');
    let ignorado = resto.split('\\').any(|c| crate::files::IGNORED.iter().any(|i| i.eq_ignore_ascii_case(c)));
    Some((dona, !ignorado))
}

/// Liga o watcher e a thread que junta os eventos. Chamado uma vez no boot; sem watcher (erro do sistema) a árvore
/// segue no foco e no ⟳, como antes.
pub fn start(s: Arc<AppState>) {
    let (tx, rx) = channel::<::notify::Result<Event>>();
    match ::notify::recommended_watcher(move |ev| {
        let _ = tx.send(ev);
    }) {
        Ok(w) => lk(estado()).watcher = Some(w),
        Err(e) => {
            tracing::warn!("watcher de arquivos indisponível: {e}");
            return;
        }
    }
    let _ = std::thread::Builder::new().name("regente-fswatch".into()).spawn(move || {
        let mut pastas: HashMap<String, HashSet<String>> = HashMap::new();
        let mut cheias: HashSet<String> = HashSet::new();
        let mut heads: HashSet<String> = HashSet::new();
        let mut ultimo = Instant::now();
        loop {
            match rx.recv_timeout(FLUSH) {
                Ok(Ok(ev)) => juntar(&ev, &mut pastas, &mut cheias, &mut heads),
                Ok(Err(e)) => tracing::debug!("watcher: {e}"),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if ultimo.elapsed() < FLUSH || (pastas.is_empty() && heads.is_empty()) {
                continue;
            }
            ultimo = Instant::now();
            let raizes: HashMap<String, String> = lk(estado()).raizes.iter().map(|(k, r)| (k.clone(), r.original.clone())).collect();
            for (k, dirs) in pastas.drain() {
                let Some(original) = raizes.get(&k) else { continue };
                crate::files::invalidate_within(Path::new(original));
                let dirs: Vec<String> = if cheias.contains(&k) { vec![] } else { dirs.into_iter().collect() };
                s.emit("fs-changed", json!({"root": original, "dirs": dirs}));
            }
            cheias.clear();
            for k in heads.drain() {
                if let Some(original) = raizes.get(&k) {
                    s.emit("git-head", json!({"root": original}));
                }
            }
        }
    });
}

fn juntar(ev: &Event, pastas: &mut HashMap<String, HashSet<String>>, cheias: &mut HashSet<String>, heads: &mut HashSet<String>) {
    // conteúdo de arquivo mudando não muda a árvore (as marcas do git têm o polling delas); acesso também não
    let estrutura = matches!(ev.kind, EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(::notify::event::ModifyKind::Name(_)) | EventKind::Any | EventKind::Other);
    let st = lk(estado());
    for p in &ev.paths {
        let k = chave(p);
        // HEAD de uma pasta do git vigiada: a branch mudou (o git grava HEAD.lock e renomeia para HEAD)
        if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("HEAD")) {
            if let Some(rs) = p.parent().and_then(|d| st.gitdirs.get(&chave(d))) {
                heads.extend(rs.iter().cloned());
                continue;
            }
        }
        if !estrutura {
            continue;
        }
        let Some((dona, conta)) = classificar(st.raizes.keys(), &k) else { continue };
        if !conta {
            continue;
        }
        let pai = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let set = pastas.entry(dona.clone()).or_default();
        set.insert(pai);
        if set.len() > DIRS_MAX {
            cheias.insert(dona.clone());
        }
    }
}

/// A árvore passou a mostrar `root`. Contado: duas árvores na mesma raiz pedem uma vigia só.
pub fn watch(root: &str) {
    if crate::remote::is_remote(root) || !Path::new(root).is_dir() {
        return;
    }
    let k = chave(Path::new(root));
    let mut st = lk(estado());
    if let Some(r) = st.raizes.get_mut(&k) {
        r.contagem += 1;
        return;
    }
    let raiz = PathBuf::from(root);
    let gitdir = git_dir_of(&raiz);
    let gitdir_fora = gitdir.clone().filter(|g| !chave(g).starts_with(&format!("{k}\\")));
    let Some(w) = st.watcher.as_mut() else { return };
    if let Err(e) = w.watch(&raiz, RecursiveMode::Recursive) {
        tracing::warn!("não deu para vigiar {root}: {e}");
        return;
    }
    if let Some(g) = &gitdir_fora {
        // a mesma pasta do git pode já estar vigiada por outra raiz (dois worktrees do mesmo repositório não: cada um tem a sua)
        let ja = st.gitdirs.contains_key(&chave(g));
        if !ja {
            if let Some(w) = st.watcher.as_mut() {
                let _ = w.watch(g, RecursiveMode::NonRecursive);
            }
        }
    }
    if let Some(g) = &gitdir {
        st.gitdirs.entry(chave(g)).or_default().insert(k.clone());
    }
    st.raizes.insert(k, Raiz { original: root.to_string(), contagem: 1, gitdir_fora });
}

/// A árvore deixou de mostrar `root` (trocou de raiz ou fechou o workspace).
pub fn unwatch(root: &str) {
    let k = chave(Path::new(root));
    let mut st = lk(estado());
    let Some(r) = st.raizes.get_mut(&k) else { return };
    r.contagem -= 1;
    if r.contagem > 0 {
        return;
    }
    let r = st.raizes.remove(&k).expect("acabou de ser lida");
    if let Some(w) = st.watcher.as_mut() {
        let _ = w.unwatch(Path::new(&r.original));
    }
    // a raiz sai das pastas do git; a vigia da pasta de fora sai quando nenhuma raiz depende mais dela
    let mut vazias = vec![];
    for (g, rs) in st.gitdirs.iter_mut() {
        rs.remove(&k);
        if rs.is_empty() {
            vazias.push(g.clone());
        }
    }
    for g in vazias {
        st.gitdirs.remove(&g);
    }
    if let Some(g) = r.gitdir_fora {
        if !st.gitdirs.contains_key(&chave(&g)) {
            if let Some(w) = st.watcher.as_mut() {
                let _ = w.unwatch(&g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evento_vai_para_a_raiz_mais_funda_e_ignorados_nao_contam() {
        let raizes = ["d:\\proj".to_string(), "d:\\proj\\.regente\\worktrees\\m1".to_string()];
        assert_eq!(classificar(raizes.iter(), "d:\\proj\\src\\a.ts"), Some((&raizes[0], true)));
        assert_eq!(classificar(raizes.iter(), "d:\\proj\\.regente\\worktrees\\m1\\src\\a.ts"), Some((&raizes[1], true)));
        assert_eq!(classificar(raizes.iter(), "d:\\proj\\node_modules\\x\\i.js"), Some((&raizes[0], false)));
        assert_eq!(classificar(raizes.iter(), "d:\\proj\\.git\\index"), Some((&raizes[0], false)));
        assert_eq!(classificar(raizes.iter(), "d:\\projeto\\a.ts"), None, "prefixo de nome não é a raiz");
    }

    #[test]
    fn pasta_do_git_de_repo_e_de_worktree() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        crate::git::git(r, &["init", "-q", "-b", "main"]).unwrap();
        crate::git::git(r, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a"]).unwrap();
        assert_eq!(chave(&git_dir_of(r).unwrap()), chave(&r.join(".git")));
        let wt = r.join("wt");
        crate::git::git(r, &["worktree", "add", "-q", "-b", "dev", &wt.to_string_lossy()]).unwrap();
        let g = git_dir_of(&wt).unwrap();
        assert!(g.join("HEAD").is_file(), "{}", g.display());
        assert!(chave(&g).starts_with(&chave(&r.join(".git").join("worktrees"))));
    }

    #[test]
    fn chave_ignora_caixa_barra_e_fim() {
        assert_eq!(chave(Path::new("D:/Proj/Src/")), "d:\\proj\\src");
    }
}
