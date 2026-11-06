use crate::db::{Mission, Workspace};
use crate::error::{fail, AppError, AppResult};
use crate::git::{branch_exists, current_branch, git, is_repo};
use crate::runtime::AppState;
use crate::util::{new_id, now_ms};
use std::path::{Path, PathBuf};

/// Opções de criação de missão, fora nome, brief e dono. `Default` = livre, sem worktree, sem receita, sem perfil.
#[derive(Debug, Clone, Default)]
pub struct MissionOpts {
    pub worktree: bool,
    pub recipe_id: Option<String>,
    pub profile: Option<String>,
    /// Missão pai (só squads: o maestro de quem delegou).
    pub parent: Option<String>,
}

/// A new "livre" mission in `ws` (the caller inserts it). With `worktree`, creates its branch and checkout.
/// Returns the mission and the folder its panes run in.
pub fn new_livre(ws: &Workspace, name: &str, worktree: bool) -> AppResult<(Mission, String)> {
    let name = if name.trim().is_empty() { "Missão".to_string() } else { name.trim().to_string() };
    let wt = if worktree { Some(create_worktree(Path::new(&ws.path), &slugify(&name))?) } else { None };
    let cwd = wt.as_ref().map(|w| w.cwd.to_string_lossy().to_string()).unwrap_or_else(|| ws.path.clone());
    let m = Mission {
        id: new_id(),
        workspace_id: ws.id.clone(),
        name,
        mode: "livre".into(),
        status: "ativa".into(),
        status_reason: None,
        worktree_path: wt.as_ref().map(|w| w.path.to_string_lossy().to_string()),
        branch: wt.as_ref().map(|w| w.branch.clone()),
        base_branch: wt.as_ref().map(|w| w.base_branch.clone()),
        pinned: false,
        created_at: now_ms(),
        archived_at: None,
        squad_id: None,
        recipe_id: None,
        brief: None,
        style: None,
        profile: None, plan_json: None, verify_json: None, adopted: false,
    };
    Ok((m, cwd))
}

pub fn worktree_of(m: &Mission) -> AppResult<Worktree> {
    if m.adopted {
        // adotada: a branch pode faltar (HEAD destacado = "livre") e a comparação é contra a ref do remoto quando existe
        let (Some(p), Some(base)) = (&m.worktree_path, &m.base_branch) else { return fail("no_worktree", "esta missão não usa worktree") };
        let path = PathBuf::from(p);
        return Ok(Worktree { cwd: path.clone(), base_branch: base_ref(&path, base), branch: m.branch.clone().unwrap_or_default(), path });
    }
    match (&m.worktree_path, &m.branch, &m.base_branch) {
        (Some(p), Some(b), Some(base)) => Ok(Worktree { path: p.into(), cwd: p.into(), branch: b.clone(), base_branch: base.clone() }),
        _ => fail("no_worktree", "esta missão não usa worktree"),
    }
}

/// The worktree of a mission that uses one and whose folder still exists.
pub fn live_worktree(m: &Mission) -> AppResult<Worktree> {
    let wt = worktree_of(m)?;
    if !wt.path.is_dir() {
        return fail("no_worktree", "o worktree da missão não existe mais");
    }
    Ok(wt)
}

/// Ref para comparar com a base: `origin/<base>` quando o remoto a tem (é o que o fluxo do projeto usa), senão `<base>`.
pub fn base_ref(dir: &Path, base: &str) -> String {
    if git(dir, &["rev-parse", "-q", "--verify", &format!("refs/remotes/origin/{base}")]).is_ok() { format!("origin/{base}") } else { base.to_string() }
}

/// Missão adotada sobre um worktree do projeto (bloco A): nome = pasta, branch = a atual (None se destacado), base = `base`.
/// Nada é criado nem removido no git.
pub fn new_adotada(ws: &Workspace, path: &str, base: &str) -> AppResult<Mission> {
    let p = Path::new(path);
    if !p.is_dir() {
        return fail("no_worktree", "a pasta do worktree não existe");
    }
    let branch = current_branch(p)?;
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "worktree".into());
    Ok(Mission {
        id: new_id(),
        workspace_id: ws.id.clone(),
        name,
        mode: "livre".into(),
        status: "ativa".into(),
        worktree_path: Some(path.to_string()),
        branch: (!branch.is_empty()).then_some(branch),
        base_branch: Some(base.to_string()),
        created_at: now_ms(),
        adopted: true,
        ..Default::default()
    })
}

/// Estado vivo de um worktree: branch (None = destacado), ↑ahead ↓behind contra `base_ref`, alterados, último commit (ms)
/// e HEAD curto. Sem `fetch`. Base inexistente: contagens zero.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeState {
    pub branch: Option<String>,
    pub base: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub last_commit_at: Option<i64>,
    pub head: String,
}

pub fn worktree_state(path: &Path, base_ref: &str) -> AppResult<WorktreeState> {
    let branch = current_branch(path)?;
    // `--left-right --count A...B`: "<só em A> <só em B>" = atrás, à frente
    let counts = git(path, &["rev-list", "--left-right", "--count", &format!("{base_ref}...HEAD")]).unwrap_or_default();
    let mut it = counts.split_whitespace().map(|n| n.parse::<u32>().unwrap_or(0));
    let (behind, ahead) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    let dirty = git(path, &["status", "--porcelain"])?.lines().filter(|l| !l.trim().is_empty()).count() as u32;
    let last_commit_at = git(path, &["log", "-1", "--format=%ct"]).ok().and_then(|t| t.trim().parse::<i64>().ok()).map(|t| t * 1000);
    let head = git(path, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    Ok(WorktreeState { branch: (!branch.is_empty()).then_some(branch), base: base_ref.to_string(), ahead, behind, dirty, last_commit_at, head })
}

/// Merges the mission's worktree into its base branch. A failure marks the mission "bloqueada" with the reason.
/// Arquivos que o branch da missão tem e a base ainda não (0 se o git falhar). Usado na memória ao integrar e arquivar.
pub fn files_pending(ws: &Path, wt: &Worktree) -> usize {
    git(ws, &["diff", "--name-only", &format!("{}...{}", wt.base_branch, wt.branch)]).map(|s| s.lines().filter(|l| !l.is_empty()).count()).unwrap_or(0)
}

pub fn integrate_mission(s: &AppState, id: &str) -> AppResult<Mission> {
    let mut m = s.db.get_mission(id)?;
    if m.adopted {
        return fail("adopted", "missão adotada: integre por PR na base, pelo fluxo do projeto");
    }
    let ws = s.db.get_workspace(&m.workspace_id)?;
    let wt = worktree_of(&m)?;
    let arquivos = files_pending(Path::new(&ws.path), &wt);
    match integrate(Path::new(&ws.path), &wt) {
        Ok(()) => {
            m.status = "concluida".into();
            m.status_reason = None;
            s.db.update_mission(&m)?;
            crate::memory::suggest(s, &m.workspace_id, Some(&m.id), "checkpoint", &format!("Missão {} integrada em {}: {arquivos} arquivos", m.name, wt.base_branch));
            Ok(m)
        }
        Err(e) => {
            m.status = "bloqueada".into();
            m.status_reason = Some(e.to_string());
            s.db.update_mission(&m)?;
            Err(e)
        }
    }
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_ascii_alphanumeric() => c,
            _ => '-',
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    let s: String = out.trim_end_matches('-').chars().take(40).collect();
    let s = s.trim_end_matches('-').to_string();
    let reserved = matches!(s.as_str(), "con" | "prn" | "aux" | "nul")
        || ((s.starts_with("com") || s.starts_with("lpt")) && s.len() == 4 && s[3..].chars().all(|c| c.is_ascii_digit()));
    if s.is_empty() { "missao".into() } else if reserved { format!("{s}-m") } else { s }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub base_branch: String,
    /// Where the agent runs: the worktree root, plus the workspace's subfolder when the workspace lives inside a larger repo.
    pub cwd: PathBuf,
}

/// `.regente/` fica fora do git, menos `projeto.json` e `glossario.md` (que a equipe pode versionar).
/// `**/` faz valer em subpastas (workspace dentro de um repositório maior). A linha antiga `.regente/` é trocada.
/// Tudo em `.regente/` fica fora do `git status` (abrir um repo não o deixa "sujo"). Quem quer dividir o `projeto.json`
/// com a equipe faz `git add -f .regente/projeto.json` uma vez: arquivo versionado continua versionado.
const REGENTE_EXCLUDE: [&str; 1] = ["**/.regente/*"];
/// Linhas antigas que liberavam o `projeto.json` e o glossário: saem na próxima abertura.
const REGENTE_EXCLUDE_ANTIGAS: [&str; 3] = [".regente/", "!**/.regente/projeto.json", "!**/.regente/glossario.md"];

pub(crate) fn ensure_excluded(ws: &Path) -> AppResult<()> {
    let common = git(ws, &["rev-parse", "--git-common-dir"])?;
    let common = if Path::new(&common).is_absolute() { PathBuf::from(common) } else { ws.join(common) };
    let exclude = common.join("info").join("exclude");
    let cur = std::fs::read_to_string(&exclude).unwrap_or_default();
    let mut lines: Vec<String> = cur.lines().filter(|l| l.trim() != ".regente" && !REGENTE_EXCLUDE_ANTIGAS.contains(&l.trim())).map(String::from).collect();
    for need in REGENTE_EXCLUDE {
        if !lines.iter().any(|l| l.trim() == need) {
            lines.push(need.to_string());
        }
    }
    let next = format!("{}\n", lines.join("\n"));
    if next != cur {
        std::fs::create_dir_all(exclude.parent().unwrap())?;
        std::fs::write(&exclude, next)?;
    }
    Ok(())
}

/// Worktree root + the `--show-prefix` subfolder (component by component); falls back to the root if that folder doesn't exist.
fn worktree_cwd(path: &Path, prefix: &str) -> PathBuf {
    let cwd = prefix.split('/').filter(|c| !c.is_empty()).fold(path.to_path_buf(), |p, c| p.join(c));
    if cwd.is_dir() { cwd } else { path.to_path_buf() }
}

/// Where a mission's pane runs: its worktree plus the workspace's subfolder (as in `create_worktree`); the worktree root if git fails.
pub fn mission_cwd(ws: &Path, wt: &Path) -> PathBuf {
    match git(ws, &["rev-parse", "--show-prefix"]) {
        Ok(prefix) => worktree_cwd(wt, &prefix),
        Err(_) => wt.to_path_buf(),
    }
}

/// Same folder on Windows: separators, trailing separator and case don't matter.
pub fn same_dir(a: &str, b: &str) -> bool {
    let n = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    n(a) == n(b)
}

/// `git worktree add <ws>\.regente\worktrees\<slug> -b regente/<slug>`, with a numeric suffix if the name is taken.
pub fn create_worktree(ws: &Path, slug: &str) -> AppResult<Worktree> {
    if crate::remote::is_remote(&ws.to_string_lossy()) {
        return fail("remote_unsupported", "missões com worktree não funcionam em workspace remoto");
    }
    if !is_repo(ws) {
        return fail("not_a_repo", "a pasta do workspace não é um repositório git");
    }
    let slug = slugify(slug);
    let slug = slug.as_str();
    let base = current_branch(ws)?;
    if base.is_empty() {
        return fail("detached_head", "o workspace está sem branch (HEAD destacado)");
    }
    let prefix = git(ws, &["rev-parse", "--show-prefix"])?;
    let root = ws.join(".regente").join("worktrees");
    let mut name = slug.to_string();
    let mut n = 2;
    while branch_exists(ws, &format!("regente/{name}")) || root.join(&name).exists() {
        name = format!("{slug}-{n}");
        n += 1;
    }
    ensure_excluded(ws)?;
    std::fs::create_dir_all(&root)?;
    let path = root.join(&name);
    let branch = format!("regente/{name}");
    git(ws, &["worktree", "add", &path.to_string_lossy(), "-b", &branch])?;
    let cwd = worktree_cwd(&path, &prefix);
    // o node_modules do checkout principal é ligado depois, em `worktree_cache::acelerar` (fora da criação)
    Ok(Worktree { path, branch, base_branch: base, cwd })
}

/// Merges the worktree branch into the base branch of the workspace. On conflict: abort, error `merge_conflict`.
pub fn integrate(ws: &Path, wt: &Worktree) -> AppResult<()> {
    if current_branch(&wt.path)? != wt.branch {
        return fail("wrong_branch", format!("o worktree da missão não está na branch '{}'", wt.branch));
    }
    if !git(&wt.path, &["status", "--porcelain"])?.is_empty() {
        return fail("dirty_worktree", "há mudanças sem commit no worktree da missão");
    }
    let cur = current_branch(ws)?;
    if cur.is_empty() {
        return fail("detached_head", "o workspace está sem branch (HEAD destacado)");
    }
    if cur != wt.base_branch {
        return fail("wrong_branch", format!("o workspace está em '{cur}', esperado '{}'", wt.base_branch));
    }
    if !git(ws, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return fail("dirty_workspace", "há mudanças sem commit no workspace");
    }
    match git(ws, &["merge", "--no-ff", "--no-edit", &wt.branch]) {
        Ok(_) => Ok(()),
        Err(e) => {
            if git(ws, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
                let _ = git(ws, &["merge", "--abort"]);
                Err(AppError::new("merge_conflict", e.to_string()))
            } else {
                Err(e)
            }
        }
    }
}

/// Tira a junção `node_modules` da worktree (só o link: o alvo fica). Se não sair, não segue: o arquivo é do checkout.
fn desligar_node_modules(path: &Path) -> AppResult<()> {
    let nm = path.join("node_modules");
    let Ok(m) = std::fs::symlink_metadata(&nm) else { return Ok(()) };
    if crate::projeto::e_link(&m) || m.file_type().is_symlink() {
        std::fs::remove_dir(&nm).or_else(|_| std::fs::remove_file(&nm)).map_err(|e| AppError::new("worktree_locked", format!("não consegui soltar o node_modules ligado: {e}")))?;
    }
    Ok(())
}

/// The `gitdir:` a worktree's `.git` file points to still exists.
fn admin_dir_exists(path: &Path) -> bool {
    let Ok(s) = std::fs::read_to_string(path.join(".git")) else { return false };
    s.trim().strip_prefix("gitdir:").is_some_and(|g| path.join(g.trim()).is_dir())
}

/// Worktree com HEAD destacado: se há alteração, ou se o commit do HEAD não está em nenhuma branch, cria
/// `regente/wip-<pasta>-<segundos>` no HEAD (o WIP vira commit nela, no fluxo normal). Limpo e alcançável: nada a guardar.
fn guardar_head_destacado(path: &Path) -> AppResult<Option<String>> {
    let sujo = !git(path, &["status", "--porcelain"])?.is_empty();
    let solto = git(path, &["branch", "--contains", "HEAD", "--format=%(refname:short)"])?.is_empty();
    if !sujo && !solto {
        return Ok(None);
    }
    let pasta: String = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let nome = format!("regente/wip-{}-{}", if pasta.is_empty() { "missao" } else { pasta.as_str() }, crate::util::now_ms() / 1000);
    git(path, &["switch", "-c", &nome])?;
    tracing::info!("worktree {} sem branch: o trabalho ficou na branch {nome}", path.display());
    Ok(Some(nome))
}

/// Archives the worktree: uncommitted work is committed to the mission branch first (the branch stays), then the worktree is removed.
/// HEAD destacado não bloqueia: o que não está em branch nenhuma ganha uma (`guardar_head_destacado`).
/// Safe to call again after `worktree_locked`.
pub fn remove_worktree(ws: &Path, path: &Path) -> AppResult<()> {
    // junção do node_modules sai ANTES de qualquer git/remove: apagar a árvore por dentro dela apagaria o checkout principal
    desligar_node_modules(path)?;
    // A live worktree has a `.git` FILE; without it git would walk up to the main repo and commit the user's own changes.
    if path.join(".git").is_file() {
        match current_branch(path) {
            Ok(b) => {
                if b.is_empty() {
                    guardar_head_destacado(path)?;
                }
                if !git(path, &["status", "--porcelain"])?.is_empty() {
                    git(path, &["add", "-A"])?;
                    git(path, &["-c", "commit.gpgsign=false", "commit", "--no-verify", "-m", "wip: missão arquivada"])?;
                }
            }
            // broken worktree (its admin dir under .git/worktrees is gone): git can't commit there; prune + delete below
            Err(_) if !admin_dir_exists(path) => {}
            Err(e) => return Err(e),
        }
    }
    if let Err(e) = git(ws, &["worktree", "remove", &path.to_string_lossy()]) {
        // only a broken worktree may be deleted by hand; a live one that git refuses (locked, etc.) keeps its files
        // a `.git` DIRECTORY is a real repo at that path, not a worktree: never delete it
        let g = path.join(".git");
        let broken = !g.exists() || (g.is_file() && !admin_dir_exists(path));
        if !broken {
            return fail("worktree_locked", format!("o git recusou remover o worktree: {e}"));
        }
    } else {
        return Ok(());
    }
    let _ = git(ws, &["worktree", "prune"]);
    if path.exists() {
        // de novo, logo antes do fallback: a junção pode ter sido religada depois da primeira chamada
        desligar_node_modules(path)?;
        let _ = std::fs::remove_dir_all(path);
    }
    if path.exists() {
        return fail("worktree_locked", "arquivos em uso; feche processos e tente de novo");
    }
    let _ = git(ws, &["worktree", "prune"]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Teste"]).unwrap();
        git(p, &["config", "user.email", "teste@example.com"]).unwrap();
        std::fs::write(p.join("a.txt"), "base\n").unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-m", "base"]).unwrap();
        d
    }

    fn commit(dir: &Path, file: &str, content: &str) {
        std::fs::write(dir.join(file), content).unwrap();
        git(dir, &["add", "."]).unwrap();
        git(dir, &["commit", "-m", file]).unwrap();
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Landing Página Nova!"), "landing-pagina-nova");
        assert_eq!(slugify("  ---  "), "missao");
        assert_eq!(slugify(&"x".repeat(80)).len(), 40);
    }

    #[test]
    fn create_integrate_remove() {
        let d = repo();
        let wt = create_worktree(d.path(), "feat").unwrap();
        assert_eq!(wt.branch, "regente/feat");
        assert_eq!(wt.base_branch, "main");
        assert!(wt.path.join("a.txt").exists());
        let wt2 = create_worktree(d.path(), "feat").unwrap();
        assert_eq!(wt2.branch, "regente/feat-2");
        let exclude = std::fs::read_to_string(d.path().join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.lines().filter(|l| l.trim() == "**/.regente/*").count(), 1);
        assert!(!exclude.lines().any(|l| l.trim() == ".regente/"));

        commit(&wt.path, "b.txt", "nova\n");
        integrate(d.path(), &wt).unwrap();
        assert!(d.path().join("b.txt").exists());
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
    }

    #[test]
    fn exclude_hides_all_of_regente_and_migrates_old_lines() {
        let d = repo();
        std::fs::write(d.path().join(".git/info/exclude"), "# meu comentário\n.regente/\n!**/.regente/projeto.json\n").unwrap();
        ensure_excluded(d.path()).unwrap();
        ensure_excluded(d.path()).unwrap();
        let ex = std::fs::read_to_string(d.path().join(".git/info/exclude")).unwrap();
        assert!(ex.contains("# meu comentário"));
        assert!(!ex.lines().any(|l| l.trim() == ".regente/"));
        assert!(!ex.lines().any(|l| l.trim().starts_with('!')));
        assert_eq!(ex.lines().filter(|l| l.trim() == "**/.regente/*").count(), 1);
        std::fs::create_dir_all(d.path().join(".regente/saidas")).unwrap();
        std::fs::write(d.path().join(".regente/saidas/x.txt"), "x").unwrap();
        std::fs::write(d.path().join(".regente/projeto.json"), "{}").unwrap();
        let ignored = |f: &str| std::process::Command::new("git").args(["check-ignore", "-q", f]).current_dir(d.path()).status().unwrap().success();
        assert!(ignored(".regente/saidas/x.txt"));
        // abrir o repo não o deixa sujo: o projeto.json criado pelo Regente não aparece no git status
        assert!(ignored(".regente/projeto.json"));
        assert!(ignored(".regente/glossario.md"));
    }

    #[test]
    fn dirty_worktree_is_refused() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        std::fs::write(wt.path.join("a.txt"), "sujo\n").unwrap();
        assert_eq!(integrate(d.path(), &wt).unwrap_err().code(), "dirty_worktree");
    }

    #[test]
    fn conflict_aborts_merge() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        commit(&wt.path, "a.txt", "do worktree\n");
        commit(d.path(), "a.txt", "do main\n");
        assert_eq!(integrate(d.path(), &wt).unwrap_err().code(), "merge_conflict");
        assert!(!d.path().join(".git/MERGE_HEAD").exists(), "merge did not abort");
    }

    #[test]
    fn archive_keeps_dirty_work_on_branch() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        std::fs::write(wt.path.join("novo.txt"), "preciosa\n").unwrap();
        std::fs::write(wt.path.join("a.txt"), "editado\n").unwrap();
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
        assert_eq!(git(d.path(), &["show", &format!("{}:novo.txt", wt.branch)]).unwrap(), "preciosa");
    }

    #[test]
    fn integrate_refuses_switched_worktree_branch() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        git(&wt.path, &["checkout", "--detach"]).unwrap();
        assert_eq!(integrate(d.path(), &wt).unwrap_err().code(), "wrong_branch");
    }

    #[test]
    fn integrate_refuses_dirty_workspace() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        commit(&wt.path, "b.txt", "n\n");
        std::fs::write(d.path().join("a.txt"), "sujo\n").unwrap();
        assert_eq!(integrate(d.path(), &wt).unwrap_err().code(), "dirty_workspace");
    }

    #[test]
    fn broken_worktree_removal_is_retryable() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        std::fs::remove_file(wt.path.join(".git")).unwrap();
        // the MAIN workspace is dirty: without the worktree's `.git` file, git would walk up and commit it
        std::fs::write(d.path().join("a.txt"), "meu trabalho\n").unwrap();
        let head = git(d.path(), &["rev-parse", "HEAD"]).unwrap();
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
        assert_eq!(git(d.path(), &["rev-parse", "HEAD"]).unwrap(), head);
        assert!(!git(d.path(), &["status", "--porcelain"]).unwrap().is_empty());
        remove_worktree(d.path(), &wt.path).unwrap();
    }

    #[test]
    fn worktree_without_admin_dir_is_removed() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        std::fs::write(wt.path.join("a.txt"), "sujo\n").unwrap();
        let gitdir = std::fs::read_to_string(wt.path.join(".git")).unwrap();
        std::fs::remove_dir_all(wt.path.join(gitdir.trim().strip_prefix("gitdir:").unwrap().trim())).unwrap();
        let head = git(d.path(), &["rev-parse", "HEAD"]).unwrap();
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
        assert_eq!(git(d.path(), &["rev-parse", "HEAD"]).unwrap(), head);
    }

    #[test]
    fn archive_with_junction_keeps_the_base_node_modules() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        let nm = d.path().join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("pacote.js"), "x").unwrap();
        crate::worktree_cache::junction(&nm, &wt.path.join("node_modules")).unwrap();
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
        assert_eq!(std::fs::read_to_string(nm.join("pacote.js")).unwrap(), "x", "o node_modules do checkout principal sobreviveu");
    }

    #[test]
    fn archive_detached_keeps_work_on_new_branch() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        git(&wt.path, &["checkout", "--detach"]).unwrap();
        commit(&wt.path, "solto.txt", "só no HEAD\n");
        std::fs::write(wt.path.join("wip.txt"), "não commitado\n").unwrap();
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
        let wip = git(d.path(), &["branch", "--list", "regente/wip-*", "--format=%(refname:short)"]).unwrap();
        assert_eq!(wip.lines().count(), 1, "uma branch nova guarda o trabalho: {wip}");
        assert_eq!(git(d.path(), &["show", &format!("{wip}:solto.txt")]).unwrap(), "só no HEAD");
        assert_eq!(git(d.path(), &["show", &format!("{wip}:wip.txt")]).unwrap(), "não commitado");
    }

    #[test]
    fn archive_detached_clean_and_reachable_needs_no_branch() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        git(&wt.path, &["checkout", "--detach"]).unwrap();
        remove_worktree(d.path(), &wt.path).unwrap();
        assert!(!wt.path.exists());
        assert!(git(d.path(), &["branch", "--list", "regente/wip-*"]).unwrap().is_empty());
    }

    #[test]
    fn archive_refuses_plain_repo_at_worktree_path() {
        let d = repo();
        let nested = d.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-b", "main"]).unwrap();
        std::fs::write(nested.join("trabalho.txt"), "não commitado\n").unwrap();
        assert_eq!(remove_worktree(d.path(), &nested).unwrap_err().code(), "worktree_locked");
        assert!(nested.join("trabalho.txt").exists());
    }

    #[test]
    fn archive_refuses_locked_worktree_and_keeps_files() {
        let d = repo();
        let wt = create_worktree(d.path(), "x").unwrap();
        std::fs::write(wt.path.join("novo.txt"), "preciosa\n").unwrap();
        git(d.path(), &["worktree", "lock", &wt.path.to_string_lossy()]).unwrap();
        assert_eq!(remove_worktree(d.path(), &wt.path).unwrap_err().code(), "worktree_locked");
        assert!(wt.path.join("novo.txt").exists());
        assert!(wt.path.join(".git").is_file());
    }

    #[test]
    fn cwd_falls_back_to_root_when_subfolder_missing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("app").join("sub")).unwrap();
        assert_eq!(worktree_cwd(d.path(), "app/sub/"), d.path().join("app").join("sub"));
        assert_eq!(worktree_cwd(d.path(), "nao/existe/"), d.path());
        assert_eq!(worktree_cwd(d.path(), ""), d.path());
    }

    #[test]
    fn reserved_names_and_internal_slugify() {
        assert_eq!(slugify("Aux"), "aux-m");
        assert_eq!(slugify("com1"), "com1-m");
        assert_eq!(slugify("COM0"), "com0-m");
        assert_eq!(slugify("lpt0"), "lpt0-m");
        let d = repo();
        assert_eq!(create_worktree(d.path(), "Landing Página").unwrap().branch, "regente/landing-pagina");
    }

    #[test]
    fn workspace_inside_larger_repo_gets_subfolder_cwd() {
        let d = repo();
        let sub = d.path().join("app");
        std::fs::create_dir(&sub).unwrap();
        commit(d.path(), "app/f.txt", "x\n");
        let wt = create_worktree(&sub, "x").unwrap();
        assert!(wt.cwd.ends_with("app") && wt.cwd.join("f.txt").exists(), "{:?}", wt.cwd);
        let top = create_worktree(d.path(), "y").unwrap();
        assert_eq!(top.cwd, top.path);
    }

    #[test]
    fn mission_cwd_matches_create_worktree() {
        let d = repo();
        let sub = d.path().join("app");
        std::fs::create_dir(&sub).unwrap();
        commit(d.path(), "app/f.txt", "x\n");
        let wt = create_worktree(&sub, "x").unwrap();
        assert_eq!(mission_cwd(&sub, &wt.path), wt.cwd);
        let top = create_worktree(d.path(), "y").unwrap();
        assert_eq!(mission_cwd(d.path(), &top.path), top.path);
        // git fails outside a repo: the worktree root
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(mission_cwd(plain.path(), &top.path), top.path);
    }

    #[test]
    fn same_dir_ignores_case_and_separators() {
        assert!(same_dir(r"C:\Proj\App\", "c:/proj/app"));
        assert!(!same_dir(r"C:\proj\app2", r"C:\proj\app"));
    }

    #[test]
    fn non_repo_fails() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(create_worktree(d.path(), "x").unwrap_err().code(), "not_a_repo");
    }

    #[test]
    fn an_adopted_mission_reads_the_folder_branch_and_compares_with_origin_when_it_exists() {
        let d = repo();
        let ws = Workspace { id: "w".into(), path: d.path().to_string_lossy().to_string(), name: "r".into(), last_opened_at: 0, pinned: false };
        let amb = d.path().join("ambiente01");
        git(d.path(), &["worktree", "add", "--detach", &amb.to_string_lossy()]).unwrap();
        let m = new_adotada(&ws, &amb.to_string_lossy(), "main").unwrap();
        assert!(m.adopted);
        assert_eq!((m.name.as_str(), m.branch.as_deref(), m.base_branch.as_deref(), m.mode.as_str()), ("ambiente01", None, Some("main"), "livre"));
        // sem remoto: a comparação é contra a própria base
        assert_eq!(worktree_of(&m).unwrap().base_branch, "main");
        // com `origin/main`, a comparação usa a ref do remoto (é o que o ambiente.sh usa)
        git(d.path(), &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        assert_eq!(worktree_of(&m).unwrap().base_branch, "origin/main");
        assert_eq!(new_adotada(&ws, &d.path().join("nada").to_string_lossy(), "main").unwrap_err().code(), "no_worktree");
    }

    #[test]
    fn worktree_state_counts_ahead_behind_dirty_and_the_last_commit() {
        let d = repo();
        let amb = d.path().join("ambiente02");
        git(d.path(), &["worktree", "add", "-b", "spec/x", &amb.to_string_lossy()]).unwrap();
        let st0 = worktree_state(&amb, "main").unwrap();
        assert_eq!((st0.branch.as_deref(), st0.ahead, st0.behind, st0.dirty), (Some("spec/x"), 0, 0, 0));
        assert!(st0.last_commit_at.is_some() && !st0.head.is_empty());
        commit(&amb, "b.txt", "nova\n");
        commit(d.path(), "c.txt", "base avançou\n");
        std::fs::write(amb.join("d.txt"), "sujo").unwrap();
        let st = worktree_state(&amb, "main").unwrap();
        assert_eq!((st.ahead, st.behind, st.dirty), (1, 1, 1));
        // base inexistente: contagem zero, sem erro
        assert_eq!(worktree_state(&amb, "nao-existe").unwrap().ahead, 0);
    }
}
