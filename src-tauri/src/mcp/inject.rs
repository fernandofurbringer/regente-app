//! Como cada CLI aponta para o servidor MCP do Regente (args, env e arquivos de configuração do painel).
//! O token é do painel (não do app): cada painel recebe o seu, e ele nunca vai em arquivo para o Claude
//! (o Claude expande `${REGENTE_MCP_TOKEN}` a partir do ambiente).

use super::McpInfo;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Injection {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Files to write before the spawn: (path, content).
    pub files: Vec<(PathBuf, String)>,
}

/// Files that carry MCP configuration (boot removes them; pane close removes the whole folder).
pub const MCP_FILES: [&str; 2] = ["mcp.json", "gemini-defaults.json"];

/// Junta `tools.truncateToolOutputThreshold` (limite de saída do perfil de trabalho) ao `gemini-defaults.json` gerado.
pub fn with_tool_output_threshold(text: &str, chars: u64) -> String {
    let mut v: Value = serde_json::from_str(text).unwrap_or_else(|_| json!({}));
    v["tools"]["truncateToolOutputThreshold"] = json!(chars);
    v.to_string()
}

/// kind: "claude" | "codex" | "gemini". profile: "full" | "minimal" | "custom". `token`: the pane's own token.
/// `servers`: (nome, configuração) dos servidores do usuário mantidos no perfil `custom` (só o Claude filtra).
pub fn mcp_injection(kind: &str, info: &McpInfo, pane_id: &str, token: &str, profile: &str, pane_dir: &Path, servers: &[(String, Value)], vistas: Option<&[&str]>) -> Injection {
    let mut env = vec![
        ("REGENTE_PANE_ID".to_string(), pane_id.to_string()),
        ("REGENTE_HOOK_URL".to_string(), info.hook_url()),
        ("REGENTE_MCP_TOKEN".to_string(), token.to_string()),
    ];
    let mut args = vec![];
    let mut files = vec![];
    match kind {
        "claude" => {
            let path = pane_dir.join("mcp.json");
            // literal `${REGENTE_MCP_TOKEN}`: Claude Code expands it from the pane's env, so the file holds no secret
            let mut all = serde_json::Map::new();
            all.insert("regente".into(), json!({"type": "http", "url": info.mcp_url(), "headers": {"X-Regente-Pane": pane_id, "Authorization": "Bearer ${REGENTE_MCP_TOKEN}"}}));
            // perfil custom: os servidores escolhidos do usuário entram junto (com a config que o Claude já tinha)
            for (name, cfg) in servers {
                all.insert(name.clone(), cfg.clone());
            }
            let cfg = json!({"mcpServers": all});
            args.extend(["--mcp-config".to_string(), path.to_string_lossy().to_string()]);
            // minimal e custom: só o que está no arquivo (--strict-mcp-config ignora o resto da configuração do usuário)
            if profile != "full" {
                args.push("--strict-mcp-config".to_string());
            }
            env.push(("MCP_TOOL_TIMEOUT".to_string(), "86400000".to_string()));
            files.push((path, cfg.to_string()));
        }
        // quote-free on purpose: `.cmd` shims reject `"` in arguments. Codex reads the token from its env var.
        "codex" => {
            args.extend(["-c".to_string(), format!("mcp_servers.regente.url={}", info.mcp_url())]);
            args.extend(["-c".to_string(), "mcp_servers.regente.bearer_token_env_var=REGENTE_MCP_TOKEN".to_string()]);
            args.extend(["-c".to_string(), "mcp_servers.regente.env_http_headers.X-Regente-Pane=REGENTE_PANE_ID".to_string()]);
            // filtro por papel: lista TOML de aspas simples (nada de `"` na linha de comando)
            if let Some(v) = vistas {
                let lista: Vec<String> = v.iter().map(|t| format!("'{t}'")).collect();
                args.extend(["-c".to_string(), format!("mcp_servers.regente.enabled_tools=[{}]", lista.join(","))]);
            }
        }
        "gemini" => {
            // literal token (Gemini's `$VAR` expansion in settings is not confirmed): the file is deleted on close and at boot
            let path = pane_dir.join("gemini-defaults.json");
            let mut cfg = json!({"mcpServers": {"regente": {"httpUrl": info.mcp_url(), "headers": {"X-Regente-Pane": pane_id, "Authorization": format!("Bearer {token}")}, "timeout": 86400000}}});
            if let Some(v) = vistas {
                cfg["mcpServers"]["regente"]["includeTools"] = json!(v);
            }
            env.push(("GEMINI_CLI_SYSTEM_DEFAULTS_PATH".to_string(), path.to_string_lossy().to_string()));
            files.push((path, cfg.to_string()));
        }
        _ => {}
    }
    Injection { args, env, files }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "tok123";

    fn info() -> McpInfo {
        McpInfo { port: 4321 }
    }

    fn env_of(inj: &Injection, k: &str) -> Option<String> {
        inj.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn claude_full() {
        let d = tempfile::tempdir().unwrap();
        let inj = mcp_injection("claude", &info(), "P1", TOKEN, "full", d.path(), &[], None);
        let path = d.path().join("mcp.json");
        assert_eq!(inj.args, ["--mcp-config".to_string(), path.to_string_lossy().to_string()]);
        assert_eq!(inj.files.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&inj.files[0].1).unwrap();
        assert_eq!(v["mcpServers"]["regente"]["headers"]["X-Regente-Pane"], "P1");
        assert_eq!(v["mcpServers"]["regente"]["headers"]["Authorization"], "Bearer ${REGENTE_MCP_TOKEN}");
        assert!(!inj.files[0].1.contains(TOKEN), "the token must not be written to the file");
        assert_eq!(env_of(&inj, "MCP_TOOL_TIMEOUT").as_deref(), Some("86400000"));
        assert_eq!(env_of(&inj, "REGENTE_PANE_ID").as_deref(), Some("P1"));
        assert_eq!(env_of(&inj, "REGENTE_MCP_TOKEN").as_deref(), Some(TOKEN));
        assert_eq!(env_of(&inj, "REGENTE_HOOK_URL").as_deref(), Some("http://127.0.0.1:4321/hook"));
    }

    #[test]
    fn claude_minimal() {
        let d = tempfile::tempdir().unwrap();
        let inj = mcp_injection("claude", &info(), "P1", TOKEN, "minimal", d.path(), &[], None);
        assert!(inj.args.contains(&"--strict-mcp-config".to_string()));
    }

    #[test]
    fn claude_custom_keeps_the_chosen_servers_and_is_strict() {
        let d = tempfile::tempdir().unwrap();
        let servers = vec![("github".to_string(), json!({"command": "gh-mcp", "env": {"K": "v"}}))];
        let inj = mcp_injection("claude", &info(), "P1", TOKEN, "custom", d.path(), &servers, None);
        assert!(inj.args.contains(&"--strict-mcp-config".to_string()));
        let v: serde_json::Value = serde_json::from_str(&inj.files[0].1).unwrap();
        assert_eq!(v["mcpServers"]["github"]["command"], "gh-mcp");
        assert!(v["mcpServers"]["regente"].is_object());
    }

    #[test]
    fn codex() {
        let d = tempfile::tempdir().unwrap();
        let inj = mcp_injection("codex", &info(), "P1", TOKEN, "full", d.path(), &[], None);
        assert_eq!(inj.args.len(), 6);
        assert_eq!(inj.args[1], "mcp_servers.regente.url=http://127.0.0.1:4321/mcp");
        assert_eq!(inj.args[3], "mcp_servers.regente.bearer_token_env_var=REGENTE_MCP_TOKEN");
        assert_eq!(inj.args[5], "mcp_servers.regente.env_http_headers.X-Regente-Pane=REGENTE_PANE_ID");
        assert!(inj.files.is_empty());
        assert_eq!(env_of(&inj, "REGENTE_MCP_TOKEN").as_deref(), Some(TOKEN));
    }

    #[test]
    fn gemini() {
        let d = tempfile::tempdir().unwrap();
        let inj = mcp_injection("gemini", &info(), "P1", TOKEN, "full", d.path(), &[], None);
        let path = d.path().join("gemini-defaults.json");
        assert_eq!(env_of(&inj, "GEMINI_CLI_SYSTEM_DEFAULTS_PATH"), Some(path.to_string_lossy().to_string()));
        let v: serde_json::Value = serde_json::from_str(&inj.files[0].1).unwrap();
        assert_eq!(v["mcpServers"]["regente"]["httpUrl"], "http://127.0.0.1:4321/mcp");
        assert_eq!(v["mcpServers"]["regente"]["headers"]["Authorization"], format!("Bearer {TOKEN}"));
    }

    #[test]
    fn no_injected_arg_has_quotes() {
        let d = tempfile::tempdir().unwrap();
        for kind in ["claude", "codex", "gemini"] {
            let inj = mcp_injection(kind, &info(), "P1", TOKEN, "minimal", d.path(), &[], None);
            for a in &inj.args {
                assert!(!a.contains('"'), "{kind}: arg with quotes: {a}");
            }
        }
    }

    #[test]
    fn unknown_kind_only_common_env() {
        let d = tempfile::tempdir().unwrap();
        let inj = mcp_injection("outro", &info(), "P1", TOKEN, "full", d.path(), &[], None);
        assert!(inj.args.is_empty() && inj.files.is_empty());
        assert_eq!(inj.env.len(), 3);
    }

    #[test]
    fn role_filter_reaches_codex_and_gemini_without_quotes() {
        let d = tempfile::tempdir().unwrap();
        let vistas = ["handoff_submit", "handoff_get"];
        let codex = mcp_injection("codex", &info(), "P1", TOKEN, "minimal", d.path(), &[], Some(&vistas));
        assert!(codex.args.contains(&"mcp_servers.regente.enabled_tools=['handoff_submit','handoff_get']".to_string()));
        assert!(codex.args.iter().all(|a| !a.contains('"')));
        let gemini = mcp_injection("gemini", &info(), "P1", TOKEN, "minimal", d.path(), &[], Some(&vistas));
        let v: serde_json::Value = serde_json::from_str(&gemini.files[0].1).unwrap();
        assert_eq!(v["mcpServers"]["regente"]["includeTools"], json!(["handoff_submit", "handoff_get"]));
        // sem filtro (papel longo) nada muda
        let full = mcp_injection("gemini", &info(), "P1", TOKEN, "full", d.path(), &[], None);
        let v: serde_json::Value = serde_json::from_str(&full.files[0].1).unwrap();
        assert!(v["mcpServers"]["regente"].get("includeTools").is_none());
    }

    #[test]
    fn gemini_threshold_is_merged_into_the_defaults_file() {
        let v: serde_json::Value = serde_json::from_str(&with_tool_output_threshold("{\"mcpServers\":{}}", 20_000)).unwrap();
        assert_eq!(v["tools"]["truncateToolOutputThreshold"], 20_000);
        assert!(v.get("mcpServers").is_some());
    }
}
