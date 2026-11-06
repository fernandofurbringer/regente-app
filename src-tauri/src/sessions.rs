//! Sessões anteriores do Claude Code e do Codex ligadas a uma pasta, para "Retomar sessão anterior".
//!
//! Claude: `<config>/projects/<slug>/<session>.jsonl`; cada linha tem `cwd`, e o arquivo é ligado pelo nome (o id).
//! Codex: `<home>/sessions/AAAA/MM/DD/rollout-*.jsonl`; a primeira linha (`session_meta`) tem `id` e `cwd`.
//! Formato do primeiro pedido do Codex não verificado nesta máquina: sem prévia.

use crate::cost::codex;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Caracteres da prévia do primeiro pedido.
pub const PROMPT_CHARS: usize = 80;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub cli: String,
    pub id: String,
    pub modified_ms: i64,
    pub size: u64,
    pub prompt: Option<String>,
}

/// Primeiro pedido do usuário numa linha do `.jsonl` do Claude, numa linha só e cortado em `PROMPT_CHARS`.
pub fn first_prompt(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["type"] != "user" {
        return None;
    }
    let text = match &v["message"]["content"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items.iter().find(|i| i["type"] == "text")?["text"].as_str()?.to_string(),
        _ => return None,
    };
    let one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // mensagens do próprio CLI (comandos, lembretes) começam com "<": não são pedidos do usuário
    if one.is_empty() || one.starts_with('<') {
        return None;
    }
    if one.chars().count() > PROMPT_CHARS {
        return Some(format!("{}…", one.chars().take(PROMPT_CHARS - 1).collect::<String>()));
    }
    Some(one)
}

/// Pasta do workspace é a mesma da sessão (sem diferença de barra ou caixa, como o Windows trata).
pub fn same_dir(a: &str, b: &str) -> bool {
    codex::norm(a) == codex::norm(b)
}

/// Id de sessão seguro como argumento de CLI: letras, dígitos, `-` e `_`, sem começar com `-` (não vira flag).
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && !id.starts_with('-') && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Pastas padrão: `CLAUDE_CONFIG_DIR` (ou `~/.claude`) e `CODEX_HOME` (ou `~/.codex`).
pub fn default_homes(home: &Path) -> (PathBuf, PathBuf) {
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
    let codex = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".codex"));
    (claude, codex)
}

fn millis(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Bytes lidos do início de cada `.jsonl` ao listar sessões (a lista roda em toda abertura do picker).
const HEAD_BYTES: u64 = 256 * 1024;

/// (cwd, primeiro pedido) lidos do início do arquivo do Claude; para no primeiro pedido do usuário.
fn claude_head(path: &Path) -> Option<(String, Option<String>)> {
    let mut cwd = None;
    let mut prompt = None;
    let mut seen_user = false;
    let head = BufReader::new(std::fs::File::open(path).ok()?.take(HEAD_BYTES));
    for line in head.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(String::from);
        }
        if !seen_user && v["type"] == "user" {
            seen_user = true;
            prompt = first_prompt(&line);
        }
        if cwd.is_some() && seen_user {
            break;
        }
    }
    Some((cwd?, prompt))
}

/// Sessões do Claude cujo `cwd` é `cwd`. Só os `.jsonl` diretos de cada projeto (subagentes ficam mais fundo).
pub fn claude_sessions(config_dir: &Path, cwd: &str) -> Vec<SessionInfo> {
    let mut out = Vec::new();
    for proj in std::fs::read_dir(config_dir.join("projects")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        for f in std::fs::read_dir(&proj).into_iter().flatten().flatten().map(|e| e.path()) {
            if f.extension().is_none_or(|e| e != "jsonl") || !f.is_file() {
                continue;
            }
            let Some((fcwd, prompt)) = claude_head(&f) else { continue };
            let (Some(id), Ok(meta)) = (f.file_stem().map(|s| s.to_string_lossy().into_owned()), std::fs::metadata(&f)) else { continue };
            if !same_dir(&fcwd, cwd) || !valid_session_id(&id) {
                continue;
            }
            out.push(SessionInfo { cli: "claude".into(), id, modified_ms: millis(&meta), size: meta.len(), prompt });
        }
    }
    out
}

fn rollouts(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    for p in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if depth > 0 && p.is_dir() {
            rollouts(&p, depth - 1, out);
        } else if p.is_file() && p.extension().is_some_and(|e| e == "jsonl") && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-")) {
            out.push(p);
        }
    }
}

/// Sessões do Codex cujo `cwd` é `cwd` (id e cwd vêm da primeira linha do rollout).
pub fn codex_sessions(home: &Path, cwd: &str) -> Vec<SessionInfo> {
    let mut files = Vec::new();
    rollouts(&home.join("sessions"), 3, &mut files);
    let mut out = Vec::new();
    for f in files {
        let Some((id, fcwd)) = codex::first_meta(&f) else { continue };
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        if same_dir(&fcwd, cwd) && valid_session_id(&id) {
            out.push(SessionInfo { cli: "codex".into(), id, modified_ms: millis(&meta), size: meta.len(), prompt: None });
        }
    }
    out
}

/// Todas as sessões da pasta, mais recentes primeiro.
pub fn for_workspace(claude_dir: &Path, codex_home: &Path, cwd: &str) -> Vec<SessionInfo> {
    let mut v = claude_sessions(claude_dir, cwd);
    v.extend(codex_sessions(codex_home, cwd));
    v.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.id.cmp(&b.id)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_head_takes_cwd_and_the_first_user_prompt() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.jsonl");
        std::fs::write(&f, "{\"type\":\"summary\"}\n{\"type\":\"user\",\"cwd\":\"C:/p\",\"message\":{\"content\":\"revisar o login\"}}\n{\"type\":\"user\",\"message\":{\"content\":\"outro\"}}\n").unwrap();
        assert_eq!(claude_head(&f), Some(("C:/p".into(), Some("revisar o login".into()))));
    }

    #[test]
    fn claude_head_does_not_read_past_the_head_limit() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("s.jsonl");
        let filler = format!("{{\"type\":\"assistant\",\"pad\":\"{}\"}}\n", "x".repeat(1000));
        let mut body = String::from("{\"type\":\"assistant\",\"cwd\":\"C:/p\"}\n");
        for _ in 0..300 {
            body.push_str(&filler); // ~300 KB antes do primeiro pedido: fora do limite de leitura
        }
        body.push_str("{\"type\":\"user\",\"message\":{\"content\":\"tarde demais\"}}\n");
        std::fs::write(&f, body).unwrap();
        assert_eq!(claude_head(&f), Some(("C:/p".into(), None)));
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn first_prompt_skips_cli_noise_and_clips_to_80_chars() {
        assert_eq!(first_prompt(r#"{"type":"user","message":{"role":"user","content":"  oi\n  tudo   bem "}}"#).as_deref(), Some("oi tudo bem"));
        assert_eq!(first_prompt(r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#), None);
        assert_eq!(first_prompt(r#"{"type":"assistant","message":{"content":"oi"}}"#), None);
        let arr = r#"{"type":"user","message":{"content":[{"type":"text","text":"revise o diff"}]}}"#;
        assert_eq!(first_prompt(arr).as_deref(), Some("revise o diff"));
        let long = format!(r#"{{"type":"user","message":{{"content":"{}"}}}}"#, "a".repeat(200));
        let p = first_prompt(&long).unwrap();
        assert_eq!(p.chars().count(), PROMPT_CHARS);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn claude_sessions_match_by_cwd_and_ignore_subagents() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join(".claude");
        let ours = cfg.join("projects").join("D--pessoal-REA");
        write(&ours.join("aaa-111.jsonl"), "{\"type\":\"user\",\"cwd\":\"D:\\\\pessoal\\\\REA\",\"message\":{\"content\":\"primeiro\"}}\n");
        write(&ours.join("aaa-111").join("subagents").join("sub.jsonl"), "{\"cwd\":\"D:\\\\pessoal\\\\REA\"}\n");
        write(&cfg.join("projects").join("outra").join("bbb-222.jsonl"), "{\"cwd\":\"C:\\\\outro\",\"message\":{}}\n");
        let found = claude_sessions(&cfg, "d:/pessoal/rea");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, "aaa-111");
        assert_eq!(found[0].cli, "claude");
        assert_eq!(found[0].prompt.as_deref(), Some("primeiro"));
        assert!(found[0].size > 0);
    }

    #[test]
    fn codex_rollouts_match_by_session_meta_cwd() {
        let d = tempfile::tempdir().unwrap();
        let day = d.path().join("sessions").join("2026").join("10").join("14");
        write(&day.join("rollout-2026-10-14T10-00-00-cod-1.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"cod-1\",\"cwd\":\"D:\\\\pessoal\\\\REA\"}}\n");
        write(&day.join("rollout-2026-10-14T11-00-00-cod-2.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"cod-2\",\"cwd\":\"D:\\\\outro\"}}\n");
        let found = codex_sessions(d.path(), "D:\\pessoal\\REA");
        assert_eq!(found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["cod-1"]);
        assert_eq!(found[0].cli, "codex");
        assert!(found[0].prompt.is_none(), "formato do pedido do Codex não verificado: sem prévia");
    }

    #[test]
    fn combined_list_is_newest_first_and_ids_are_safe() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join(".claude");
        write(&cfg.join("projects").join("p").join("old.jsonl"), "{\"cwd\":\"X:\\\\w\"}\n");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&cfg.join("projects").join("p").join("new.jsonl"), "{\"cwd\":\"X:\\\\w\"}\n");
        let all = for_workspace(&cfg, &d.path().join(".codex"), "X:\\w");
        assert_eq!(all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["new", "old"]);
        assert!(valid_session_id("abc-123_x"));
        assert!(!valid_session_id("a b"));
        assert!(!valid_session_id("--dangerously-skip"), "um id com espaço ou ponto não vira argumento");
        assert!(!valid_session_id("a;rm"));
    }
}
