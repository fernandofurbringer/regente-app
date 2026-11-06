//! Issue do GitHub para o brief de uma missão (P1.9). `gh` roda só com o clique do usuário, na pasta do workspace,
//! com os argumentos separados (sem shell). A saída é lida como JSON: nada dela vai para um comando.

use crate::error::{AppError, AppResult};
use crate::util::cmd;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;

const TITLE_MAX: usize = 300;
const BODY_MAX: usize = 20_000;
/// Mensagem quando o `gh` não está instalado (usada também pelo PR, P2.2).
pub const GH_MISSING: &str = "GitHub CLI (gh) não encontrado: instale com winget install GitHub.cli (ou em cli.github.com)";

static ISSUE_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/issues/(\d{1,9})$").expect("valid regex"));

#[derive(Debug, Clone, PartialEq)]
pub enum IssueRef {
    Number(u64),
    /// URL já validada (sem barra final); o número vem dela.
    Url { url: String, number: u64 },
}

impl IssueRef {
    pub fn number(&self) -> u64 {
        match self {
            IssueRef::Number(n) | IssueRef::Url { number: n, .. } => *n,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// "123", "#123" ou a URL `https://github.com/dono/repo/issues/123`. Outra coisa é None.
pub fn parse_ref(text: &str) -> Option<IssueRef> {
    let t = text.trim();
    let digits = t.strip_prefix('#').unwrap_or(t);
    if !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|b| b.is_ascii_digit()) {
        let n: u64 = digits.parse().ok()?;
        return (n > 0).then_some(IssueRef::Number(n));
    }
    let url = t.trim_end_matches('/');
    let caps = ISSUE_URL.captures(url)?;
    let number: u64 = caps[1].parse().ok()?;
    (number > 0).then(|| IssueRef::Url { url: url.to_string(), number })
}

/// JSON de `gh issue view --json title,body,labels`. Título obrigatório; corpo e título com limite de tamanho.
pub fn parse_view(json: &[u8], number: u64) -> AppResult<Issue> {
    let v: Value = serde_json::from_slice(json).map_err(|_| AppError::new("gh_bad_json", "resposta do gh ilegível"))?;
    let title = v["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| AppError::new("gh_bad_json", "a issue não tem título"))?;
    let body = v["body"].as_str().unwrap_or_default();
    let labels = v["labels"].as_array().map(|ls| ls.iter().filter_map(|l| l["name"].as_str()).map(str::to_string).collect()).unwrap_or_default();
    Ok(Issue {
        number,
        title: title.chars().take(TITLE_MAX).collect(),
        body: body.chars().take(BODY_MAX).collect(),
        labels,
    })
}

/// Roda `gh` na pasta dada, com argumentos separados e sem prompts. Sem `gh`, ou sem login, a mensagem diz o que fazer.
pub fn run(dir: &Path, args: &[&str]) -> AppResult<String> {
    let out = cmd("gh")
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => AppError::new("gh_missing", GH_MISSING),
            _ => e.into(),
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let first = err.lines().next().unwrap_or("").to_string();
        let lower = err.to_lowercase();
        if lower.contains("auth login") || lower.contains("not logged") || lower.contains("authentication") {
            return Err(AppError::new("gh_auth", "gh não está autenticado: rode gh auth login no terminal"));
        }
        return Err(AppError::new("gh_failed", format!("gh {} falhou: {first}", args.first().unwrap_or(&""))));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Roda `gh issue view` na pasta dada.
pub fn view(dir: &Path, r: &IssueRef) -> AppResult<Issue> {
    let target = match r {
        IssueRef::Number(n) => n.to_string(),
        IssueRef::Url { url, .. } => url.clone(),
    };
    let json = run(dir, &["issue", "view", &target, "--json", "title,body,labels"])?;
    parse_view(json.as_bytes(), r.number())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_accept_number_hash_and_issue_url() {
        assert_eq!(parse_ref("123"), Some(IssueRef::Number(123)));
        assert_eq!(parse_ref("  #45 "), Some(IssueRef::Number(45)));
        assert_eq!(
            parse_ref("https://github.com/acme/app/issues/7/"),
            Some(IssueRef::Url { url: "https://github.com/acme/app/issues/7".into(), number: 7 })
        );
    }

    #[test]
    fn refs_refuse_anything_else() {
        for bad in ["", "#", "0", "abc", "12a", "-1", "1234567890", "http://github.com/a/b/issues/1", "https://github.com/a/b/pull/1", "https://evil.com/a/b/issues/1", "https://github.com/a/b/issues/1;rm -rf"] {
            assert_eq!(parse_ref(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn view_json_becomes_an_issue() {
        let json = r#"{"title":"  Botão quebrado  ","body":"Passos:\n1. clicar","labels":[{"name":"bug"},{"name":"ui"}]}"#;
        let i = parse_view(json.as_bytes(), 9).unwrap();
        assert_eq!(i, Issue { number: 9, title: "Botão quebrado".into(), body: "Passos:\n1. clicar".into(), labels: vec!["bug".into(), "ui".into()] });
    }

    #[test]
    fn view_json_needs_a_title_and_tolerates_empty_body() {
        assert_eq!(parse_view(br#"{"title":"   ","body":""}"#, 1).unwrap_err().code(), "gh_bad_json");
        assert_eq!(parse_view(b"<html>", 1).unwrap_err().code(), "gh_bad_json");
        let i = parse_view(br#"{"title":"x","body":null}"#, 1).unwrap();
        assert_eq!((i.body.as_str(), i.labels.len()), ("", 0));
    }

    #[test]
    fn view_json_is_capped() {
        let json = format!(r#"{{"title":"{}","body":"{}"}}"#, "t".repeat(400), "b".repeat(BODY_MAX + 10));
        let i = parse_view(json.as_bytes(), 1).unwrap();
        assert_eq!(i.title.chars().count(), TITLE_MAX);
        assert_eq!(i.body.chars().count(), BODY_MAX);
    }
}
