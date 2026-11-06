//! Checkpoints do trabalho dos agentes (A1.1). Cada um é um commit (sem branch) da árvore de trabalho sob
//! `refs/regente/checkpoints/<escopo>/<n>`. Nada toca HEAD, o índice do usuário nem a branch: a árvore sai de um
//! índice temporário (`GIT_INDEX_FILE` dentro de `.git`). Restaurar é `git restore --worktree` mais a remoção do que o
//! checkpoint não tinha; antes, um checkpoint de segurança guarda o estado atual.

use crate::error::{fail, AppError, AppResult};
use crate::git::{git, git_bytes, git_env};
use crate::review::{checked, split_nul, FileVersions, VERSION_MAX};
use crate::runtime::AppState;
use crate::util::{lk, new_id};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Checkpoints mantidos por escopo; os mais antigos saem.
pub const KEEP: usize = 50;
/// Arquivos listados por checkpoint (o total e as somas vêm inteiros).
const FILES_SHOWN: usize = 200;
const NS: &str = "refs/regente/checkpoints";
/// Identidade do commit: não depende do `user.name` do usuário.
const BOT: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Regente"),
    ("GIT_AUTHOR_EMAIL", "regente@localhost"),
    ("GIT_COMMITTER_NAME", "Regente"),
    ("GIT_COMMITTER_EMAIL", "regente@localhost"),
];
/// Uma operação de escrita por vez: a numeração `n` e a poda dependem uma da outra.
static LOCK: Mutex<()> = Mutex::new(());
/// Arquivo não ignorado acima disso fica fora do checkpoint (pesa muito e raramente é código).
const LARGE_MAX: u64 = 20 * 1024 * 1024;
/// Caminhos por chamada do git: fica abaixo do limite de linha de comando do Windows.
const PATHS_PER_CALL: usize = 64;
/// Rótulo do checkpoint de segurança (o assunto vira "checkpoint: antes de voltar <hora>").
const SAFETY_LABEL: &str = "antes de voltar";
/// Os últimos checkpoints de segurança que a poda nunca apaga: é deles que sai o "Desfazer".
const SAFETY_KEEP: usize = 5;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub n: u64,
    pub id: String,
    /// Hora do commit em ms (epoch).
    pub time: i64,
    /// Painel que disparou o checkpoint ("antes de voltar" no de segurança).
    pub pane: String,
    pub files: Vec<FileStat>,
    pub total_files: usize,
    pub added: u32,
    pub removed: u32,
    /// Arquivos não ignorados grandes (acima de 20 MB) que ficaram fora deste checkpoint. Só o `take` preenche.
    pub large_skipped: usize,
}

/// Resultado de `restore`: `safety` é o checkpoint "antes de voltar" (restaurá-lo desfaz); `falhas` são os arquivos
/// que não voltaram, cada um com o motivo. Uma falha por arquivo não para os outros.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub safety: u64,
    pub falhas: Vec<String>,
}

/// Uma ref de checkpoint: número, commit, hora (s) e assunto ("checkpoint: <painel> <hora>").
struct Ref {
    n: u64,
    sha: String,
    secs: i64,
    subject: String,
}

/// Escopo vira parte de um nome de ref: só letras, dígitos, `-` e `_`.
pub fn valid_scope(scope: &str) -> bool {
    !scope.is_empty() && scope.len() <= 80 && scope.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check_scope(scope: &str) -> AppResult<()> {
    if valid_scope(scope) { Ok(()) } else { fail("bad_scope", "escopo de checkpoint inválido") }
}

/// Caminho vindo da tela: relativo, sem `..`, sem unidade de disco nem `:` (vira `commit:caminho`).
pub fn rel_ok(rel: &str) -> AppResult<()> {
    let bad = rel.is_empty() || rel.contains('\0') || rel.contains(':') || rel.starts_with(['/', '\\']) || rel.split(['/', '\\']).any(|c| c == "..");
    if bad { fail("bad_path", "caminho de arquivo inválido") } else { Ok(()) }
}

fn ref_name(scope: &str, n: u64) -> String {
    format!("{NS}/{scope}/{n}")
}

/// Só a raiz do repositório: com o workspace numa subpasta, `add -A` pegaria o repositório inteiro.
fn require_top(root: &Path) -> AppResult<()> {
    if git(root, &["rev-parse", "--show-prefix"])?.is_empty() {
        return Ok(());
    }
    fail("checkpoint_subpasta", "o workspace está numa pasta dentro de um repositório maior; checkpoints precisam da raiz do repositório")
}

fn head(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "--verify", "-q", "HEAD"]).ok().filter(|s| !s.is_empty())
}

fn tree_of(root: &Path, rev: &str) -> AppResult<String> {
    git(root, &["rev-parse", &format!("{rev}^{{tree}}")])
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Árvore da pasta como está agora. HEAD semeia um índice temporário; `add -A` traz as mudanças (ignorados ficam de fora),
/// e `extra` (ignorados que o alvo de um restore tem) entra à força para o restore não sobrescrever sem cópia.
/// Devolve a árvore e quantos arquivos grandes ficaram de fora.
fn working_tree(root: &Path, extra: &[String]) -> AppResult<(String, usize)> {
    let gitdir = git(root, &["rev-parse", "--absolute-git-dir"])?;
    let idx = PathBuf::from(&gitdir).join(format!("regente-ckpt-{}.idx", new_id()));
    let _guard = RemoveOnDrop(idx.clone());
    let idx_s = idx.to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", idx_s.as_str())];
    if let Some(h) = head(root) {
        git_env(root, &["read-tree", &h], &env)?;
    }
    git_env(root, &["add", "-A"], &env)?;
    // grandes e não ignorados: saem do índice temporário (o do usuário não é tocado)
    let large = large_untracked(root)?;
    for chunk in large.chunks(PATHS_PER_CALL) {
        let mut args = vec!["update-index", "--force-remove", "--"];
        args.extend(chunk.iter().map(String::as_str));
        git_env(root, &args, &env)?;
    }
    if !extra.is_empty() {
        let mut args = vec!["add", "-f", "--"];
        args.extend(extra.iter().map(String::as_str));
        git_env(root, &args, &env)?;
    }
    Ok((git_env(root, &["write-tree"], &env)?, large.len()))
}

/// Arquivos não ignorados e não rastreados acima de `LARGE_MAX`.
fn large_untracked(root: &Path) -> AppResult<Vec<String>> {
    let names = split_nul(&git_bytes(root, &["ls-files", "-o", "--exclude-standard", "-z"])?);
    Ok(names.into_iter().filter(|p| std::fs::metadata(root.join(p)).is_ok_and(|m| m.len() > LARGE_MAX)).collect())
}

/// Dos `paths`, os que estão ignorados. Consulta por lotes com pathspec literal (nome com `*` ou `[` não vira glob).
fn ignored_among(root: &Path, paths: &[String]) -> AppResult<Vec<String>> {
    let mut out = Vec::new();
    for chunk in paths.chunks(PATHS_PER_CALL) {
        let literal: Vec<String> = chunk.iter().map(|p| format!(":(literal){p}")).collect();
        let mut args = vec!["ls-files", "-o", "-i", "--exclude-standard", "-z", "--"];
        args.extend(literal.iter().map(String::as_str));
        out.extend(split_nul(&git_bytes(root, &args)?));
    }
    Ok(out)
}

/// Todos os caminhos de uma árvore (com `-z`, sem aspas).
fn tree_paths(root: &Path, rev: &str) -> AppResult<Vec<String>> {
    Ok(split_nul(&git_bytes(root, &["ls-tree", "-r", "--name-only", "-z", rev])?))
}

fn list_refs(root: &Path, scope: &str) -> AppResult<Vec<Ref>> {
    let prefix = format!("{NS}/{scope}/");
    let out = git(root, &["for-each-ref", "--format=%(refname)%09%(objectname)%09%(committerdate:unix)%09%(contents:subject)", &format!("{NS}/{scope}")])?;
    let mut refs: Vec<Ref> = out
        .lines()
        .filter_map(|line| {
            let mut c = line.splitn(4, '\t');
            let (name, sha, secs, subject) = (c.next()?, c.next()?, c.next()?, c.next().unwrap_or(""));
            let n = name.strip_prefix(&prefix)?.parse().ok()?;
            Some(Ref { n, sha: sha.to_string(), secs: secs.parse().ok()?, subject: subject.to_string() })
        })
        .collect();
    refs.sort_by_key(|r| r.n);
    Ok(refs)
}

fn numstat(root: &Path, a: &str, b: &str) -> AppResult<Vec<FileStat>> {
    let bytes = git_bytes(root, &["diff-tree", "-r", "--numstat", "--no-renames", "-z", a, b])?;
    Ok(split_nul(&bytes)
        .into_iter()
        .filter_map(|rec| {
            let mut c = rec.splitn(3, '\t');
            let (x, y, p) = (c.next()?, c.next()?, c.next()?);
            // binário sai como "-": conta 0
            Some(FileStat { path: p.to_string(), added: x.parse().unwrap_or(0), removed: y.parse().unwrap_or(0) })
        })
        .collect())
}

/// Nome do painel no assunto: "checkpoint: <painel> <hora>" (a hora é a última palavra).
fn pane_of(subject: &str) -> String {
    let body = subject.strip_prefix("checkpoint: ").unwrap_or(subject);
    body.rsplit_once(' ').map_or(String::new(), |(pane, _hora)| pane.to_string())
}

/// Rótulo seguro para a mensagem do commit: sem quebras de linha, curto.
fn clean_label(pane: &str) -> String {
    let s: String = pane.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s: String = s.trim().chars().take(60).collect();
    if s.is_empty() { "agente".into() } else { s }
}

fn describe(root: &Path, r: &Ref, prev: &str) -> Checkpoint {
    let files = numstat(root, prev, &r.sha).unwrap_or_default();
    let added = files.iter().map(|f| f.added).sum();
    let removed = files.iter().map(|f| f.removed).sum();
    Checkpoint {
        n: r.n,
        id: r.sha.clone(),
        time: r.secs * 1000,
        pane: pane_of(&r.subject),
        total_files: files.len(),
        files: files.into_iter().take(FILES_SHOWN).collect(),
        added,
        removed,
        large_skipped: 0,
    }
}

fn take_locked(root: &Path, scope: &str, pane: &str, force: bool, extra: &[String]) -> AppResult<Option<Checkpoint>> {
    require_top(root)?;
    let refs = list_refs(root, scope)?;
    let last = refs.last();
    let head = head(root);
    let parent = last.map(|r| r.sha.clone()).or_else(|| head.clone());
    let (tree, large_skipped) = working_tree(root, extra)?;
    if !force {
        // sem mudança desde o último checkpoint (ou desde o HEAD, no primeiro): não cria nada
        let base = match (last, &head) {
            (Some(r), _) => Some(tree_of(root, &r.sha)?),
            (None, Some(h)) => Some(tree_of(root, h)?),
            _ => None,
        };
        if base.as_deref() == Some(tree.as_str()) {
            return Ok(None);
        }
    }
    let secs = chrono::Utc::now().timestamp();
    let msg = format!("checkpoint: {} {}", clean_label(pane), chrono::Local::now().format("%H:%M:%S"));
    let mut args = vec!["commit-tree", tree.as_str()];
    if let Some(p) = parent.as_deref() {
        args.extend(["-p", p]);
    }
    args.extend(["-m", msg.as_str()]);
    let sha = git_env(root, &args, &BOT)?;
    let n = last.map_or(1, |r| r.n + 1);
    git(root, &["update-ref", &ref_name(scope, n), &sha])?;
    let prev = parent.unwrap_or_else(|| format!("{sha}^"));
    let mut cp = describe(root, &Ref { n, sha, secs, subject: msg }, &prev);
    cp.large_skipped = large_skipped;
    prune_locked(root, scope, KEEP)?;
    Ok(Some(cp))
}

/// Remove os checkpoints além de `keep`, menos os `SAFETY_KEEP` últimos de segurança. Devolve quantos saíram.
fn prune_locked(root: &Path, scope: &str, keep: usize) -> AppResult<usize> {
    let refs = list_refs(root, scope)?;
    let drop = refs.len().saturating_sub(keep);
    let safety: HashSet<u64> = refs.iter().rev().filter(|r| r.subject.starts_with(&format!("checkpoint: {SAFETY_LABEL}"))).take(SAFETY_KEEP).map(|r| r.n).collect();
    let mut removed = 0;
    for r in &refs[..drop] {
        if safety.contains(&r.n) {
            continue;
        }
        git(root, &["update-ref", "-d", &ref_name(scope, r.n)])?;
        removed += 1;
    }
    Ok(removed)
}

/// Guarda o estado atual da pasta. `None` se nada mudou desde o último checkpoint (ou desde o HEAD).
pub fn take(root: &Path, scope: &str, pane: &str) -> AppResult<Option<Checkpoint>> {
    check_scope(scope)?;
    let _g = lk(&LOCK);
    take_locked(root, scope, pane, false, &[])
}

/// Checkpoints do escopo, mais recentes primeiro. Cada um traz os arquivos mudados contra o anterior.
pub fn list(root: &Path, scope: &str) -> AppResult<Vec<Checkpoint>> {
    check_scope(scope)?;
    let refs = list_refs(root, scope)?;
    let mut out: Vec<Checkpoint> = refs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let prev = if i > 0 { refs[i - 1].sha.clone() } else { format!("{}^", r.sha) };
            describe(root, r, &prev)
        })
        .collect();
    out.reverse();
    Ok(out)
}

fn blob_at(root: &Path, rev: &str, rel: &str) -> AppResult<Option<Vec<u8>>> {
    let spec = format!("{rev}:{rel}");
    if git(root, &["cat-file", "-e", &spec]).is_err() {
        return Ok(None);
    }
    Ok(Some(git_bytes(root, &["cat-file", "blob", &spec])?))
}

/// Texto de um arquivo antes (checkpoint anterior, ou o pai do primeiro) e no checkpoint `n`, para o diff do Monaco.
pub fn diff(root: &Path, scope: &str, n: u64, rel: &str) -> AppResult<FileVersions> {
    check_scope(scope)?;
    rel_ok(rel)?;
    let refs = list_refs(root, scope)?;
    let Some(i) = refs.iter().position(|r| r.n == n) else {
        return fail("checkpoint_not_found", "checkpoint não encontrado");
    };
    let cur = refs[i].sha.clone();
    let prev = if i > 0 { refs[i - 1].sha.clone() } else { format!("{cur}^") };
    let before = blob_at(root, &prev, rel)?.unwrap_or_default();
    let after = blob_at(root, &cur, rel)?.unwrap_or_default();
    if before.len() as u64 > VERSION_MAX || after.len() as u64 > VERSION_MAX {
        return Ok(FileVersions { base: String::new(), current: String::new(), binary: false, too_big: true });
    }
    let binary = before.contains(&0) || after.contains(&0);
    Ok(match (String::from_utf8(before), String::from_utf8(after)) {
        (Ok(base), Ok(current)) if !binary => FileVersions { base, current, binary: false, too_big: false },
        _ => FileVersions { base: String::new(), current: String::new(), binary: true, too_big: false },
    })
}

/// Volta a pasta ao checkpoint `n`. Antes, cria o checkpoint de segurança ("antes de voltar") e o devolve: restaurá-lo desfaz.
/// Só a árvore de trabalho muda; o índice e o HEAD ficam como estão.
pub fn restore(root: &Path, scope: &str, n: u64) -> AppResult<Restored> {
    check_scope(scope)?;
    let _g = lk(&LOCK);
    require_top(root)?;
    let refs = list_refs(root, scope)?;
    let Some(target) = refs.iter().find(|r| r.n == n).map(|r| r.sha.clone()) else {
        return fail("checkpoint_not_found", "checkpoint não encontrado");
    };
    // ignorados que o alvo tem: o restore os sobrescreve, então entram no checkpoint de segurança à força
    let target_paths = tree_paths(root, &target)?;
    let guard = ignored_among(root, &target_paths)?;
    let Some(safety) = take_locked(root, scope, SAFETY_LABEL, true, &guard)? else {
        return fail("checkpoint_failed", "não foi possível criar o checkpoint de segurança");
    };
    // daqui em diante a pasta já pode estar mexida: todo erro leva o número do checkpoint de segurança
    let falhas = restore_files(root, &target, &safety.id, &target_paths).map_err(|e| {
        AppError::new(e.code(), format!("{e} (checkpoint de segurança n.º {}: restaurar por ele desfaz)", safety.n))
    })?;
    Ok(Restored { safety: safety.n, falhas })
}

/// Escreve o alvo na pasta e tira o que só existe no estado atual. Devolve a lista de arquivos que não voltaram.
fn restore_files(root: &Path, target: &str, safety_id: &str, target_paths: &[String]) -> AppResult<Vec<String>> {
    // arquivos que existem no estado atual e não existiam no alvo: saem depois do restore
    let created = split_nul(&git_bytes(root, &["diff", "--name-only", "--no-renames", "--diff-filter=A", "-z", target, safety_id])?);
    let mut falhas = Vec::new();
    let source = format!("--source={target}");
    if git(root, &["restore", &source, "--worktree", "--", "."]).is_err() {
        // um arquivo travado (aberto por outro programa, por exemplo) não pode parar os demais: um a um
        for rel in target_paths {
            let spec = format!(":(literal){rel}");
            if let Err(e) = git(root, &["restore", &source, "--worktree", "--", &spec]) {
                falhas.push(format!("{rel}: {e}"));
            }
        }
    }
    // no Windows, `readme.md` e `README.md` são o mesmo arquivo: o nome do alvo fica, não se apaga
    let lower: HashSet<String> = target_paths.iter().map(|p| p.to_lowercase()).collect();
    for rel in &created {
        if lower.contains(&rel.to_lowercase()) || !root.join(rel).is_file() {
            continue;
        }
        let removed = checked(root, rel).and_then(|p| std::fs::remove_file(p).map_err(AppError::from));
        if let Err(e) = removed {
            falhas.push(format!("{rel}: {e}"));
        }
    }
    Ok(falhas)
}

/// Mantém só os `keep` checkpoints mais recentes do escopo. Devolve quantos saíram.
pub fn prune(root: &Path, scope: &str, keep: usize) -> AppResult<usize> {
    check_scope(scope)?;
    let _g = lk(&LOCK);
    prune_locked(root, scope, keep)
}

type S<'a> = State<'a, Arc<AppState>>;

/// Raiz do git e chave do escopo: a missão com worktree (chave = id) ou o workspace (chave = `ws-<id>`).
fn scope_of(s: &AppState, scope: &str) -> AppResult<(PathBuf, String)> {
    if let Ok(m) = s.db.get_mission(scope) {
        let wt = crate::missions::live_worktree(&m)?;
        return Ok((wt.path, m.id));
    }
    let ws = s.db.get_workspace(scope)?;
    Ok((PathBuf::from(&ws.path), format!("ws-{}", ws.id)))
}

// async: roda git sobre a pasta inteira, fora da thread principal
#[tauri::command(async)]
pub fn checkpoint_take(s: S<'_>, scope: String, pane: Option<String>) -> AppResult<Option<Checkpoint>> {
    let (root, key) = scope_of(&s, &scope)?;
    take(&root, &key, pane.as_deref().unwrap_or("manual"))
}

#[tauri::command(async)]
pub fn checkpoint_list(s: S<'_>, scope: String) -> AppResult<Vec<Checkpoint>> {
    let (root, key) = scope_of(&s, &scope)?;
    list(&root, &key)
}

#[tauri::command(async)]
pub fn checkpoint_diff(s: S<'_>, scope: String, n: u64, path: String) -> AppResult<FileVersions> {
    let (root, key) = scope_of(&s, &scope)?;
    diff(&root, &key, n, &path)
}

#[tauri::command(async)]
pub fn checkpoint_restore(s: S<'_>, scope: String, n: u64) -> AppResult<Restored> {
    // agente escrevendo na pasta agora teria o trabalho sobrescrito: só volta com os agentes parados
    let panes = match s.db.get_mission(&scope) {
        Ok(_) => s.db.mission_panes(&scope)?,
        Err(_) => s.db.open_panes(&scope)?,
    };
    if panes.iter().any(|p| p.closed_at.is_none() && p.status == "trabalhando") {
        return fail("agente_trabalhando", "um agente está trabalhando nesta pasta; espere ele parar para voltar");
    }
    let (root, key) = scope_of(&s, &scope)?;
    restore(&root, &key, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::git_env as genv;

    /// Repositório temporário com um commit (`a.txt` = "base").
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        git(r, &["init", "-q"]).unwrap();
        std::fs::write(r.join("a.txt"), "base").unwrap();
        git(r, &["add", "a.txt"]).unwrap();
        genv(r, &["commit", "-q", "-m", "inicial"], &BOT).unwrap();
        d
    }

    fn write(r: &Path, rel: &str, body: &str) {
        std::fs::write(r.join(rel), body).unwrap();
    }

    #[test]
    fn take_leaves_head_index_and_branch_alone() {
        let d = repo();
        let r = d.path();
        write(r, "a.txt", "mudou");
        write(r, "novo.txt", "x");
        let head_before = git(r, &["rev-parse", "HEAD"]).unwrap();
        let branch_before = git(r, &["branch", "--show-current"]).unwrap();
        let cached_before = git(r, &["diff", "--cached", "--name-only"]).unwrap();
        let cp = take(r, "m1", "agente").unwrap().expect("mudou, tem checkpoint");
        assert_eq!(cp.n, 1);
        assert_eq!(cp.pane, "agente");
        assert_eq!(cp.total_files, 2);
        assert_eq!(git(r, &["rev-parse", "HEAD"]).unwrap(), head_before);
        assert_eq!(git(r, &["branch", "--show-current"]).unwrap(), branch_before);
        assert_eq!(git(r, &["diff", "--cached", "--name-only"]).unwrap(), cached_before);
        assert!(git(r, &["status", "--porcelain"]).unwrap().contains("?? novo.txt"), "o índice do usuário não ganhou o arquivo novo");
        assert!(git(r, &["ls-files"]).unwrap() == "a.txt", "o índice do usuário não mudou");
        assert!(std::fs::read_dir(r.join(".git")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("regente-ckpt")), "índice temporário removido");
    }

    #[test]
    fn unchanged_tree_creates_nothing() {
        let d = repo();
        let r = d.path();
        assert!(take(r, "m1", "agente").unwrap().is_none(), "sem mudança desde o HEAD");
        write(r, "a.txt", "v1");
        assert!(take(r, "m1", "agente").unwrap().is_some());
        assert!(take(r, "m1", "agente").unwrap().is_none(), "sem mudança desde o último checkpoint");
        assert_eq!(list(r, "m1").unwrap().len(), 1);
    }

    #[test]
    fn restore_brings_files_back_removes_new_ones_and_safety_undoes_it() {
        let d = repo();
        let r = d.path();
        write(r, "a.txt", "v1");
        write(r, "c.txt", "criado antes");
        let ck = take(r, "m1", "agente").unwrap().unwrap();
        // o agente estraga: muda, apaga c.txt e cria d.txt
        write(r, "a.txt", "ruim");
        std::fs::remove_file(r.join("c.txt")).unwrap();
        write(r, "d.txt", "novo");
        let back = restore(r, "m1", ck.n).unwrap();
        assert!(back.falhas.is_empty(), "{:?}", back.falhas);
        assert_eq!(std::fs::read_to_string(r.join("a.txt")).unwrap(), "v1");
        assert_eq!(std::fs::read_to_string(r.join("c.txt")).unwrap(), "criado antes");
        assert!(!r.join("d.txt").exists(), "arquivo criado depois do checkpoint sai");
        let safety = list(r, "m1").unwrap().into_iter().find(|c| c.n == back.safety).unwrap();
        assert_eq!(safety.pane, "antes de voltar");
        restore(r, "m1", safety.n).unwrap();
        assert_eq!(std::fs::read_to_string(r.join("a.txt")).unwrap(), "ruim");
        assert!(!r.join("c.txt").exists());
        assert_eq!(std::fs::read_to_string(r.join("d.txt")).unwrap(), "novo");
    }

    #[test]
    fn restore_does_not_lose_a_file_that_became_ignored_after_the_checkpoint() {
        let d = repo();
        let r = d.path();
        write(r, ".env", "v1");
        let ck = take(r, "m1", "agente").unwrap().unwrap();
        // depois do checkpoint, a regra de ignorar entra e o usuário edita o arquivo
        write(r, ".gitignore", ".env\n");
        write(r, ".env", "editado depois");
        let back = restore(r, "m1", ck.n).unwrap();
        assert_eq!(std::fs::read_to_string(r.join(".env")).unwrap(), "v1");
        // o checkpoint de segurança guardou a edição: voltar para ele a traz de volta
        restore(r, "m1", back.safety).unwrap();
        assert_eq!(std::fs::read_to_string(r.join(".env")).unwrap(), "editado depois");
    }

    #[test]
    fn prune_keeps_the_last_fifty() {
        let d = repo();
        let r = d.path();
        for i in 0..53 {
            write(r, "a.txt", &format!("v{i}"));
            take(r, "m1", "agente").unwrap().unwrap();
        }
        let ns: Vec<u64> = list(r, "m1").unwrap().iter().map(|c| c.n).collect();
        assert_eq!(ns.len(), KEEP);
        assert_eq!(ns.first(), Some(&53));
        assert_eq!(ns.last(), Some(&4));
    }

    #[test]
    fn large_untracked_files_stay_out_and_are_counted() {
        let d = repo();
        let r = d.path();
        write(r, "a.txt", "v1");
        let big = std::fs::File::create(r.join("grande.bin")).unwrap();
        big.set_len(LARGE_MAX + 1).unwrap();
        let cp = take(r, "m1", "agente").unwrap().unwrap();
        assert_eq!(cp.large_skipped, 1);
        assert!(cp.files.iter().all(|f| f.path != "grande.bin"), "{:?}", cp.files);
        assert!(git(r, &["status", "--porcelain"]).unwrap().contains("?? grande.bin"), "o índice do usuário não muda");
    }

    #[test]
    fn ignored_files_are_found_by_literal_pathspec() {
        let d = repo();
        let r = d.path();
        write(r, ".gitignore", "*.log\n");
        write(r, "a[1].log", "x");
        write(r, "b.log", "y");
        let found = ignored_among(r, &["a[1].log".to_string(), "b.log".to_string(), "a.txt".to_string()]).unwrap();
        assert_eq!(found, vec!["a[1].log".to_string(), "b.log".to_string()]);
    }

    #[test]
    fn prune_never_removes_the_latest_safety_checkpoints() {
        let d = repo();
        let r = d.path();
        for i in 0..2 {
            write(r, "a.txt", &format!("s{i}"));
            take(r, "m1", SAFETY_LABEL).unwrap().unwrap();
        }
        for i in 0..52 {
            write(r, "a.txt", &format!("v{i}"));
            take(r, "m1", "agente").unwrap().unwrap();
        }
        let panes: Vec<String> = list(r, "m1").unwrap().into_iter().map(|c| c.pane).collect();
        assert_eq!(panes.iter().filter(|p| p.as_str() == SAFETY_LABEL).count(), 2, "{panes:?}");
        // ponytail: os de segurança ficam além do limite KEEP (são só dois aqui)
        assert_eq!(list(r, "m1").unwrap().len(), KEEP + 2, "a poda ainda corta o resto");
    }

    #[test]
    fn diff_gives_both_sides_and_paths_stay_inside_root() {
        let d = repo();
        let r = d.path();
        write(r, "a.txt", "v1");
        let ck = take(r, "m1", "agente").unwrap().unwrap();
        let v = diff(r, "m1", ck.n, "a.txt").unwrap();
        assert_eq!((v.base.as_str(), v.current.as_str()), ("base", "v1"));
        assert!(diff(r, "m1", ck.n, "../fora.txt").is_err());
        assert!(diff(r, "m1", ck.n, "/etc/passwd").is_err());
        assert!(diff(r, "m1", ck.n, "C:x").is_err());
        assert!(take(r, "../x", "agente").is_err(), "escopo com separador é recusado");
    }
}
