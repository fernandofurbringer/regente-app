//! Worktrees do repositório do workspace (bloco A): o que `git worktree list --porcelain` devolve, sem o checkout
//! principal. Só leitura: o Regente nunca cria nem remove worktree do projeto por aqui (isso é do `ambiente.sh`).
use crate::error::AppResult;
use crate::git::git;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    /// Caminho como o git escreve (barras `/`, também no Windows).
    pub path: String,
    /// SHA curto do HEAD.
    pub head: String,
    /// Branch atual; None = HEAD destacado ("livre").
    pub branch: Option<String>,
}

/// Blocos separados por linha vazia: `worktree <path>`, `HEAD <sha>`, `branch refs/heads/<nome>` ou `detached`
/// (e `locked`/`prunable`, ignorados). O primeiro bloco é o checkout principal e fica de fora.
pub fn parse_porcelain(text: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    for (i, block) in text.replace("\r\n", "\n").split("\n\n").enumerate() {
        let mut path = None;
        let mut head = String::new();
        let mut branch = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(p.trim().to_string());
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                head = h.trim().chars().take(7).collect();
            } else if let Some(b) = line.strip_prefix("branch ") {
                let b = b.trim();
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            }
        }
        let Some(path) = path else { continue };
        if i == 0 {
            continue;
        }
        out.push(Worktree { path, head, branch });
    }
    out
}

/// Worktrees do repositório que contém `root`, sem o principal.
pub fn list(root: &Path) -> AppResult<Vec<Worktree>> {
    Ok(parse_porcelain(&git(root, &["worktree", "list", "--porcelain"])?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORCELAIN: &str = "worktree D:/repos/meu-app\nHEAD 19a301f1234567890\nbranch refs/heads/spec/ambientes-paralelos\n\nworktree D:/repos/meu-app/.claude/worktrees/ambiente01\nHEAD d9595bd0000000000\ndetached\n\nworktree C:/Users/Ana Silva/x/ambiente 02\nHEAD 7b9a1ed0000000000\nbranch refs/heads/spec/cadastro\nlocked\n";

    #[test]
    fn porcelain_skips_the_main_checkout_and_reads_branch_or_detached() {
        let w = parse_porcelain(PORCELAIN);
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!((w[0].path.as_str(), w[0].head.as_str(), w[0].branch.as_deref()), ("D:/repos/meu-app/.claude/worktrees/ambiente01", "d9595bd", None));
        assert_eq!((w[1].path.as_str(), w[1].branch.as_deref()), ("C:/Users/Ana Silva/x/ambiente 02", Some("spec/cadastro")));
    }

    #[test]
    fn porcelain_with_only_the_main_checkout_is_empty() {
        assert!(parse_porcelain("worktree D:/x\nHEAD abc\nbranch refs/heads/main\n").is_empty());
        assert!(parse_porcelain("").is_empty());
    }

    #[test]
    fn list_reads_a_real_repository() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for a in [vec!["init", "-b", "main"], vec!["config", "user.name", "T"], vec!["config", "user.email", "t@e.com"]] {
            git(&repo, &a).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        git(&repo, &["add", "."]).unwrap();
        git(&repo, &["commit", "-m", "base"]).unwrap();
        let livre = d.path().join("ambiente01");
        let spec = d.path().join("ambiente02");
        git(&repo, &["worktree", "add", "--detach", &livre.to_string_lossy()]).unwrap();
        git(&repo, &["worktree", "add", "-b", "spec/x", &spec.to_string_lossy()]).unwrap();
        let mut w = list(&repo).unwrap();
        w.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[0].branch, None);
        assert_eq!(w[1].branch.as_deref(), Some("spec/x"));
        // o %TEMP% pode vir em nome curto 8.3 (ANASIL~1); o git imprime o nome longo: compara pelo caminho canônico
        let longa = std::fs::canonicalize(&spec).unwrap().to_string_lossy().trim_start_matches(r"\\?\").to_string();
        assert!(crate::missions::same_dir(&w[1].path, &longa), "{} vs {longa}", w[1].path);
    }
}
