//! Revisão de uma missão antes de integrar (P2.1): arquivos mudados contra a base, as duas versões de um arquivo e
//! reverter um arquivo para a base. Todo git roda com argumentos separados; os caminhos passam por `checked`.

use crate::error::{fail, AppResult};
use crate::files;
use crate::git::{git, git_bytes};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Largest file (per side) whose text goes to the review editor.
pub const VERSION_MAX: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    /// A = novo na missão, M = alterado, D = apagado.
    pub status: String,
    /// Linhas acrescentadas e removidas contra a base (binário e apagado: 0 quando não dá para contar).
    pub add: u32,
    pub del: u32,
    /// Hash do conteúdo no disco (vazio se apagado): a revisão aceita some quando o arquivo muda.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileVersions {
    pub base: String,
    pub current: String,
    /// Binário, não UTF-8: os textos vêm vazios.
    pub binary: bool,
    /// Acima de `VERSION_MAX` em algum lado: os textos vêm vazios.
    pub too_big: bool,
}

/// A path from the UI, checked: relative, no `..` or drive part, and inside `root` (symlinks resolved).
pub(crate) fn checked(root: &Path, rel: &str) -> AppResult<PathBuf> {
    let bad = rel.is_empty() || rel.contains('\0') || rel.contains(':') || rel.starts_with(['/', '\\']) || rel.split(['/', '\\']).any(|c| c == "..");
    if bad {
        return fail("bad_path", "caminho de arquivo inválido");
    }
    let abs = root.join(rel);
    files::ensure_strictly_within(&[root.to_path_buf()], &abs)?;
    Ok(abs)
}

fn in_base(root: &Path, base: &str, rel: &str) -> bool {
    git(root, &["cat-file", "-e", &format!("{base}:{rel}")]).is_ok()
}

/// Quais destes caminhos existem na árvore de `base`: um `git ls-tree` por lote (a linha de comando do Windows tem
/// teto), em vez de um `cat-file -e` por arquivo.
fn in_base_all(root: &Path, base: &str, rels: &[&String]) -> AppResult<HashSet<String>> {
    const LOTE_CHARS: usize = 20_000;
    let mut out = HashSet::new();
    let mut i = 0;
    while i < rels.len() {
        let mut args: Vec<&str> = vec!["ls-tree", "-r", "-z", "--name-only", "--full-tree", base, "--"];
        let mut tam = 0;
        while i < rels.len() && (tam == 0 || tam + rels[i].len() < LOTE_CHARS) {
            tam += rels[i].len() + 1;
            args.push(rels[i].as_str());
            i += 1;
        }
        out.extend(split_nul(&git_bytes(root, &args)?));
    }
    Ok(out)
}

pub(crate) fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect()
}

/// Files the branch changes against `base`: committed (`base...HEAD`) plus uncommitted and untracked ones.
/// The status compares with the base tree and the disk: in base and present = M, not in base = A, gone = D.
pub fn changed_files(root: &Path, base: &str) -> AppResult<Vec<ChangedFile>> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(split_nul(&git_bytes(root, &["diff", "--no-renames", "-z", "--name-only", &format!("{base}...HEAD")])?));
    // porcelain -z: "XY <path>" per entry; with --no-renames there is no second path
    for entry in split_nul(&git_bytes(root, &["--no-optional-locks", "status", "--porcelain", "-z", "--untracked-files=all", "--no-renames"])?) {
        if entry.len() > 3 {
            paths.insert(entry[3..].to_string());
        }
    }
    let stats = numstat(root, base)?;
    let na_base = in_base_all(root, base, &paths.iter().collect::<Vec<_>>())?;
    let mut out = vec![];
    for p in paths {
        let abs = root.join(&p);
        // só arquivo regular: diretório e não-arquivo nunca são lidos
        let meta = std::fs::metadata(&abs).ok().filter(|m| m.is_file());
        let status = match (na_base.contains(&p), meta.is_some()) {
            (true, true) => "M",
            (false, true) => "A",
            (true, false) => "D",
            (false, false) => continue,
        };
        let (fingerprint, fallback) = match &meta {
            Some(m) => disk_state(&abs, m, status == "A"),
            None => (String::new(), 0),
        };
        // fora do numstat só vale a contagem de um arquivo novo; o resto fica em (0, 0)
        let (add, del) = stats.get(&p).copied().unwrap_or((fallback, 0));
        out.push(ChangedFile { path: p, status: status.into(), add, del, fingerprint });
    }
    Ok(out)
}

/// Estado no disco: SHA-256 do conteúdo até `VERSION_MAX`, `tamanho|mtime_ns` acima (sem ler o arquivo inteiro).
/// Linhas só se `count_lines` e o arquivo couber; acima do limite sai 0.
fn disk_state(abs: &Path, meta: &std::fs::Metadata, count_lines: bool) -> (String, u32) {
    if meta.len() > VERSION_MAX {
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        return (format!("{}|{mtime}", meta.len()), 0);
    }
    let Ok(bytes) = std::fs::read(abs) else { return (String::new(), 0) };
    let lines = if count_lines && !bytes.contains(&0) { bytes.iter().filter(|c| **c == b'\n').count() as u32 } else { 0 };
    (fingerprint(&bytes), lines)
}

/// Linhas +/− por arquivo entre o merge-base e o disco (comitado ou não). Binário sai como "-" e fica de fora.
fn numstat(root: &Path, base: &str) -> AppResult<HashMap<String, (u32, u32)>> {
    let mb = git(root, &["merge-base", base, "HEAD"])?.trim().to_string();
    let mut out = HashMap::new();
    for rec in split_nul(&git_bytes(root, &["diff", "--numstat", "--no-renames", "-z", &mb])?) {
        let mut cols = rec.splitn(3, '\t');
        if let (Some(a), Some(d), Some(p)) = (cols.next(), cols.next(), cols.next()) {
            if let (Ok(a), Ok(d)) = (a.parse(), d.parse()) {
                out.insert(p.to_string(), (a, d));
            }
        }
    }
    Ok(out)
}

/// 16 hex chars of SHA-256: stable across toolchains (DefaultHasher is not).
fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The base and current text of one file. Binary, non-UTF-8 or larger than `VERSION_MAX` come back as flags.
pub fn file_versions(root: &Path, base: &str, rel: &str) -> AppResult<FileVersions> {
    let abs = checked(root, rel)?;
    let base_bytes = if in_base(root, base, rel) { git_bytes(root, &["cat-file", "blob", &format!("{base}:{rel}")])? } else { Vec::new() };
    let cur_len = std::fs::metadata(&abs).ok().filter(|m| m.is_file()).map(|m| m.len());
    if base_bytes.len() as u64 > VERSION_MAX || cur_len.is_some_and(|n| n > VERSION_MAX) {
        return Ok(FileVersions { base: String::new(), current: String::new(), binary: false, too_big: true });
    }
    let cur_bytes = if cur_len.is_some() { std::fs::read(&abs)? } else { Vec::new() };
    let binary = base_bytes.contains(&0) || cur_bytes.contains(&0);
    match (String::from_utf8(base_bytes), String::from_utf8(cur_bytes)) {
        (Ok(base), Ok(current)) if !binary => Ok(FileVersions { base, current, binary: false, too_big: false }),
        _ => Ok(FileVersions { base: String::new(), current: String::new(), binary: true, too_big: false }),
    }
}

/// Puts one file back as in `base`. A file the base does not have is deleted. A change already committed on the
/// branch is committed back too ("revert: <arquivo>"), or the worktree stays dirty and `integrate` refuses it.
pub fn revert_file(root: &Path, base: &str, rel: &str) -> AppResult<()> {
    let abs = checked(root, rel)?;
    if in_base(root, base, rel) {
        git(root, &["checkout", base, "--", rel])?;
    } else if !git(root, &["ls-files", "--", rel])?.is_empty() {
        git(root, &["rm", "-f", "-q", "--", rel])?;
    } else if abs.is_file() {
        std::fs::remove_file(&abs)?;
    }
    // checkout and rm already staged the path, so the commit only takes it
    if !git(root, &["status", "--porcelain", "--", rel])?.is_empty() {
        git(root, &["commit", "-m", &format!("revert: {rel}"), "--", rel])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// main: a.txt, d.txt, e.txt. Branch `feat` is checked out.
    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-b", "main"]).unwrap();
        git(p, &["config", "user.name", "Teste"]).unwrap();
        git(p, &["config", "user.email", "teste@example.com"]).unwrap();
        git(p, &["config", "core.autocrlf", "false"]).unwrap();
        for f in ["a.txt", "d.txt", "e.txt"] {
            std::fs::write(p.join(f), format!("base {f}\n")).unwrap();
        }
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-m", "base"]).unwrap();
        git(p, &["checkout", "-b", "feat"]).unwrap();
        d
    }

    /// A branch with committed changes (a modified, b added, d removed) and uncommitted ones (c untracked, e modified).
    fn changed_repo() -> tempfile::TempDir {
        let d = repo();
        let p = d.path();
        std::fs::write(p.join("a.txt"), "mudou\n").unwrap();
        std::fs::write(p.join("b.txt"), "novo\n").unwrap();
        git(p, &["rm", "-q", "d.txt"]).unwrap();
        git(p, &["add", "."]).unwrap();
        git(p, &["commit", "-m", "mudancas"]).unwrap();
        std::fs::write(p.join("c.txt"), "sem commit\n").unwrap();
        std::fs::write(p.join("e.txt"), "editado\n").unwrap();
        d
    }

    fn statuses(files: &[ChangedFile]) -> Vec<(String, String)> {
        files.iter().map(|f| (f.path.clone(), f.status.clone())).collect()
    }

    fn s(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn counts_lines_per_file_and_fingerprints_content() {
        let d = changed_repo();
        let files = changed_files(d.path(), "main").unwrap();
        let get = |p: &str| files.iter().find(|f| f.path == p).unwrap();
        assert_eq!((get("a.txt").add, get("a.txt").del), (1, 1));
        assert_eq!((get("b.txt").add, get("b.txt").del), (1, 0));
        assert_eq!((get("c.txt").add, get("c.txt").del), (1, 0));
        assert_eq!((get("d.txt").add, get("d.txt").del), (0, 1));
        assert_eq!((get("e.txt").add, get("e.txt").del), (1, 1));
        assert!(get("d.txt").fingerprint.is_empty());
        let before = get("e.txt").fingerprint.clone();
        std::fs::write(d.path().join("e.txt"), "outro\n").unwrap();
        let after = changed_files(d.path(), "main").unwrap().into_iter().find(|f| f.path == "e.txt").unwrap().fingerprint;
        assert_ne!(before, after);
        assert_eq!(after.len(), 16);
        assert_eq!(super::fingerprint(b"outro
"), after);
        assert_eq!(super::fingerprint(b""), "e3b0c44298fc1c14");
    }

    #[test]
    fn changed_files_join_committed_and_uncommitted() {
        let d = changed_repo();
        let files = changed_files(d.path(), "main").unwrap();
        assert_eq!(statuses(&files), s(&[("a.txt", "M"), ("b.txt", "A"), ("c.txt", "A"), ("d.txt", "D"), ("e.txt", "M")]));
    }

    #[test]
    fn big_untracked_file_is_fingerprinted_by_size_without_lines() {
        let d = changed_repo();
        let p = d.path();
        std::fs::write(p.join("grande.txt"), "x\n".repeat(VERSION_MAX as usize)).unwrap();
        let f = changed_files(p, "main").unwrap().into_iter().find(|f| f.path == "grande.txt").unwrap();
        assert_eq!((f.status.as_str(), f.add, f.del), ("A", 0, 0));
        assert!(f.fingerprint.starts_with(&format!("{}|", 2 * VERSION_MAX)), "{}", f.fingerprint);
    }

    #[test]
    fn clean_branch_has_no_changes() {
        let d = repo();
        assert!(changed_files(d.path(), "main").unwrap().is_empty());
    }

    #[test]
    fn versions_of_text_binary_and_big_files() {
        let d = changed_repo();
        let p = d.path();
        let v = file_versions(p, "main", "a.txt").unwrap();
        assert_eq!((v.base.as_str(), v.current.as_str(), v.binary, v.too_big), ("base a.txt\n", "mudou\n", false, false));
        let added = file_versions(p, "main", "c.txt").unwrap();
        assert_eq!((added.base.as_str(), added.current.as_str()), ("", "sem commit\n"));
        let gone = file_versions(p, "main", "d.txt").unwrap();
        assert_eq!((gone.base.as_str(), gone.current.as_str()), ("base d.txt\n", ""));
        std::fs::write(p.join("f.bin"), [0u8, 1, 2, 255]).unwrap();
        assert!(file_versions(p, "main", "f.bin").unwrap().binary);
        std::fs::write(p.join("grande.txt"), vec![b'x'; VERSION_MAX as usize + 1]).unwrap();
        assert!(file_versions(p, "main", "grande.txt").unwrap().too_big);
    }

    #[test]
    fn revert_uncommitted_edit_restores_base() {
        let d = changed_repo();
        let p = d.path();
        revert_file(p, "main", "e.txt").unwrap();
        assert_eq!(std::fs::read_to_string(p.join("e.txt")).unwrap(), "base e.txt\n");
        assert!(!changed_files(p, "main").unwrap().iter().any(|f| f.path == "e.txt"));
    }

    #[test]
    fn revert_committed_edit_commits_the_restore() {
        let d = changed_repo();
        let p = d.path();
        revert_file(p, "main", "a.txt").unwrap();
        assert_eq!(std::fs::read_to_string(p.join("a.txt")).unwrap(), "base a.txt\n");
        assert_eq!(git(p, &["log", "-1", "--format=%s"]).unwrap(), "revert: a.txt");
        assert!(git(p, &["status", "--porcelain"]).unwrap().lines().all(|l| l.ends_with("c.txt") || l.ends_with("e.txt")));
        assert!(!changed_files(p, "main").unwrap().iter().any(|f| f.path == "a.txt"));
    }

    #[test]
    fn revert_added_files_deletes_them() {
        let d = changed_repo();
        let p = d.path();
        revert_file(p, "main", "c.txt").unwrap();
        assert!(!p.join("c.txt").exists());
        revert_file(p, "main", "b.txt").unwrap();
        assert!(!p.join("b.txt").exists());
        assert_eq!(git(p, &["log", "-1", "--format=%s"]).unwrap(), "revert: b.txt");
        assert!(!changed_files(p, "main").unwrap().iter().any(|f| f.path == "b.txt" || f.path == "c.txt"));
    }

    #[test]
    fn reverting_a_deleted_file_brings_it_back() {
        let d = changed_repo();
        let p = d.path();
        revert_file(p, "main", "d.txt").unwrap();
        assert_eq!(std::fs::read_to_string(p.join("d.txt")).unwrap(), "base d.txt\n");
        assert!(!changed_files(p, "main").unwrap().iter().any(|f| f.path == "d.txt"));
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused() {
        let d = changed_repo();
        let p = d.path();
        for bad in ["../fora.txt", "a/../../fora.txt", "..\\fora.txt", "/etc/passwd", "C:/x.txt", ""] {
            assert_eq!(file_versions(p, "main", bad).unwrap_err().code(), "bad_path", "{bad:?}");
            assert_eq!(revert_file(p, "main", bad).unwrap_err().code(), "bad_path", "{bad:?}");
        }
        assert!(!d.path().parent().unwrap().join("fora.txt").exists());
    }
}
