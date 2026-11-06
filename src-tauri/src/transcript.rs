//! Conversa de um painel em Markdown (P1.6). Claude: turnos do usuário e do Claude a partir do JSONL da sessão,
//! com as chamadas de ferramenta recolhidas em `<details>`. Outros CLIs: o texto do terminal. Sempre passa por `diag::redact`.

use crate::diag::redact;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Resumo de uma chamada de ferramenta: no máximo este tanto de caracteres, em uma linha.
const SUMMARY_CHARS: usize = 120;
/// Campos da entrada que identificam a chamada (o primeiro que existir vira o resumo).
const KEYS: [&str; 8] = ["command", "file_path", "path", "pattern", "url", "query", "description", "prompt"];

enum Part {
    Text(String),
    Tool { id: String, name: String, input: Value },
}

struct Turn {
    user: bool,
    parts: Vec<Part>,
}

/// Turnos e resultados de ferramenta de um JSONL de sessão, na ordem em que aparecem.
/// `keep_sidechain`: o JSONL de um subagente inteiro é sidechain; a sessão principal não traz essas linhas.
fn parse(jsonl: &str, keep_sidechain: bool) -> (Vec<Turn>, HashMap<String, (bool, String)>) {
    let mut results: HashMap<String, (bool, String)> = HashMap::new();
    let mut turns: Vec<Turn> = Vec::new();
    let mut seen = HashSet::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if (v["isSidechain"] == true && !keep_sidechain) || v["isMeta"] == true {
            continue;
        }
        // o Claude grava uma linha por bloco da mesma resposta: a chave `uuid` tira as repetidas
        if let Some(uuid) = v["uuid"].as_str() {
            if !seen.insert(uuid.to_string()) {
                continue;
            }
        }
        let user = match v["type"].as_str() {
            Some("user") => true,
            Some("assistant") => false,
            _ => continue,
        };
        let content = &v["message"]["content"];
        let mut parts = Vec::new();
        match content {
            Value::String(s) if !s.trim().is_empty() => parts.push(Part::Text(s.clone())),
            Value::Array(blocks) => {
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = b["text"].as_str().filter(|t| !t.trim().is_empty()) {
                                parts.push(Part::Text(t.to_string()));
                            }
                        }
                        Some("tool_use") => parts.push(Part::Tool {
                            id: b["id"].as_str().unwrap_or_default().to_string(),
                            name: b["name"].as_str().unwrap_or("ferramenta").to_string(),
                            input: b["input"].clone(),
                        }),
                        Some("tool_result") => {
                            if let Some(id) = b["tool_use_id"].as_str() {
                                results.insert(id.to_string(), (b["is_error"] == true, block_text(&b["content"])));
                            }
                        }
                        _ => {} // thinking e demais blocos não fazem parte da conversa
                    }
                }
            }
            _ => {}
        }
        // linha que só traz resultados de ferramenta não abre turno
        if !parts.is_empty() {
            turns.push(Turn { user, parts });
        }
    }
    (turns, results)
}

/// Conversa do Claude (JSONL da sessão) como Markdown. `full_tools`: inclui a entrada e a saída completas das ferramentas.
pub fn claude_markdown(jsonl: &str, title: &str, full_tools: bool) -> String {
    let (turns, results) = parse(jsonl, false);
    let mut out = format!("# {title}\n\n");
    let mut last: Option<bool> = None;
    for t in &turns {
        if last != Some(t.user) {
            out.push_str(if t.user { "## Você\n\n" } else { "## Claude\n\n" });
            last = Some(t.user);
        }
        for p in &t.parts {
            match p {
                // texto que fecha o <details> ou traz cerca de crase quebraria a estrutura: vai em bloco de código
                Part::Text(text) if text.contains("</details>") || text.contains("```") => out.push_str(&format!("{}\n\n", fenced("text", text.trim_end()))),
                Part::Text(text) => out.push_str(&format!("{}\n\n", text.trim_end())),
                Part::Tool { id, name, input } => out.push_str(&tool_block(name, input, results.get(id), full_tools)),
            }
        }
    }
    redact(&out)
}

/// Texto do terminal (outros CLIs) como Markdown, dentro de um bloco de código. Passa por `diag::redact`.
pub fn terminal_markdown(title: &str, text: &str) -> String {
    let body = text.trim_end_matches(['\n', ' ']);
    let fence = fence_for(body);
    redact(&format!("# {title}\n\n{fence}text\n{body}\n{fence}\n"))
}

/// Conversa do Claude para o celular, em ordem: falas (`kind` text, com `role` voce ou claude, redigidas) e chamadas
/// de ferramenta (`kind` tool) só com nome e estado (`ok`, `erro` ou `pendente`). Nenhuma entrada ou saída de
/// ferramenta sai daqui.
/// Marcadores que o Claude Code grava nas mensagens do usuário (comandos e lembretes internos): não são fala dele.
const CLAUDE_MARKERS: [&str; 5] = ["command-name", "command-message", "command-args", "system-reminder", "local-command-stdout"];

/// Teto de um item de texto no celular, em caracteres.
pub const TEXT_MAX_CHARS: usize = 4000;

/// Tira os blocos `<marcador>…</marcador>`; um marcador sem fechamento leva até o fim do texto.
pub fn strip_markers(text: &str) -> String {
    let mut s = text.to_string();
    for tag in CLAUDE_MARKERS {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(start) = s.find(&open) {
            let end = s[start..].find(&close).map_or(s.len(), |i| start + i + close.len());
            s.replace_range(start..end, "");
        }
    }
    s.trim().to_string()
}

/// Corta um texto no teto de caracteres (não de bytes, para não quebrar acentos).
pub fn cap_chars(text: &str) -> String {
    text.chars().take(TEXT_MAX_CHARS).collect()
}

pub fn claude_items(jsonl: &str) -> Vec<Value> {
    project(jsonl, false)
}

/// Conversa de um subagente (`subagents/agent-<id>.jsonl`), mesma projeção do celular (texto e ferramentas recolhidas).
pub fn subagent_items(jsonl: &str) -> Vec<Value> {
    project(jsonl, true)
}

/// Quanto do fim do transcrito a tela lê a cada tick: o painel mostra o fim da conversa.
const SUBAGENT_TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// Painel de subagente: as últimas linhas do transcrito e a descrição de `agent-<id>.meta.json` ao lado.
/// Só lê `.jsonl` dentro de uma pasta `subagents`; qualquer outro caminho dá vazio.
pub fn subagent_view(path: &std::path::Path) -> Value {
    let ours = path.extension().is_some_and(|e| e == "jsonl") && path.components().any(|c| c.as_os_str() == "subagents");
    if !ours {
        return json!({"items": [], "description": null});
    }
    let items = tail_text(path, SUBAGENT_TAIL_BYTES).map(|t| subagent_items(&t)).unwrap_or_default();
    let description = std::fs::read_to_string(path.with_extension("meta.json"))
        .ok()
        .and_then(|m| serde_json::from_str::<Value>(&m).ok())
        .and_then(|m| m["description"].as_str().map(str::to_string));
    json!({"items": items, "description": description})
}

fn project(jsonl: &str, keep_sidechain: bool) -> Vec<Value> {
    let (turns, results) = parse(jsonl, keep_sidechain);
    let mut out = Vec::new();
    for t in &turns {
        for p in &t.parts {
            match p {
                Part::Text(text) => {
                    // mensagem do usuário sem fala depois de tirar os marcadores não vira bolha vazia
                    let body = if t.user { strip_markers(text) } else { text.trim_end().to_string() };
                    if body.is_empty() {
                        continue;
                    }
                    out.push(json!({"kind": "text", "role": if t.user { "voce" } else { "claude" }, "text": cap_chars(&redact(&body))}));
                }
                Part::Tool { id, name, .. } => {
                    let status = match results.get(id) {
                        Some((true, _)) => "erro",
                        Some((false, _)) => "ok",
                        None => "pendente",
                    };
                    out.push(json!({"kind": "tool", "name": name, "status": status}));
                }
            }
        }
    }
    out
}

/// Últimos `max` bytes de um arquivo como texto, começando na primeira linha inteira (o arquivo de sessão pode
/// ser grande e o celular só mostra o fim).
pub fn tail_text(path: &std::path::Path, max: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let start = f.metadata()?.len().saturating_sub(max);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        if let Some(nl) = text.find('\n') {
            text.drain(..=nl);
        }
    }
    Ok(text)
}

/// Texto de um `tool_result`: string, ou a lista de blocos de texto dele.
fn block_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(bs) => bs.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn tool_block(name: &str, input: &Value, result: Option<&(bool, String)>, full: bool) -> String {
    let detail = KEYS.iter().find_map(|k| input[*k].as_str()).map(|s| one_line(s, SUMMARY_CHARS)).unwrap_or_default();
    let summary = if detail.is_empty() { name.to_string() } else { format!("{name}: {detail}") };
    let mut out = format!("<details>\n<summary>{}</summary>\n\n", html_escape(&summary));
    if full {
        out.push_str(&format!("**Entrada**\n\n{}\n\n", fenced("json", &serde_json::to_string_pretty(input).unwrap_or_default())));
        if let Some((is_err, text)) = result {
            let label = if *is_err { "**Erro**" } else { "**Saída**" };
            out.push_str(&format!("{label}\n\n{}\n\n", fenced("", text.trim_end())));
        }
    }
    out.push_str("</details>\n\n");
    out
}

/// Uma linha, cortada em `max` caracteres.
fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Cerca de crases maior que qualquer sequência dentro do texto, para o código não fechar cedo.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(longest.max(2) + 1)
}

fn fenced(lang: &str, text: &str) -> String {
    let fence = fence_for(text);
    format!("{fence}{lang}\n{text}\n{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"rode os testes"}}
{"type":"assistant","uuid":"a1","message":{"id":"m1","content":[{"type":"text","text":"Vou rodar."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test","description":"roda a suíte"}}]}}
{"type":"user","uuid":"r1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"12 passed"}]}]}}
{"type":"assistant","uuid":"a2","message":{"id":"m2","content":[{"type":"thinking","thinking":"segredo interno"},{"type":"text","text":"Tudo verde."}]}}
{"type":"assistant","uuid":"a3","isSidechain":true,"message":{"id":"m3","content":[{"type":"text","text":"subagente"}]}}
{"type":"user","uuid":"m9","isMeta":true,"message":{"role":"user","content":"meta"}}
{"type":"assistant","uuid":"a1","message":{"id":"m1","content":[{"type":"text","text":"repetida"}]}}
not json
"#;

    #[test]
    fn phone_items_keep_texts_and_only_tool_name_and_status() {
        let items = claude_items(SESSION);
        let texts: Vec<&str> = items.iter().filter(|i| i["kind"] == "text").map(|i| i["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["rode os testes", "Vou rodar.", "Tudo verde."]);
        assert_eq!(items[0]["role"], "voce");
        assert_eq!(items[1]["role"], "claude");
        let tool = items.iter().find(|i| i["kind"] == "tool").unwrap();
        assert_eq!(tool, &json!({"kind": "tool", "name": "Bash", "status": "ok"}));
        let dump = serde_json::to_string(&items).unwrap();
        assert!(!dump.contains("npm test") && !dump.contains("12 passed"), "entrada e saída da ferramenta não saem: {dump}");
        for hidden in ["subagente", "repetida", "segredo interno"] {
            assert!(!dump.contains(hidden), "{hidden} apareceu: {dump}");
        }
    }

    #[test]
    fn phone_tool_status_is_error_or_pending_when_no_result() {
        let s = r#"{"type":"assistant","uuid":"a1","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Edit","input":{}},{"type":"tool_use","id":"t2","name":"Read","input":{}}]}}
{"type":"user","uuid":"r1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"falhou"}]}}"#;
        let items = claude_items(s);
        assert_eq!(items[0], json!({"kind": "tool", "name": "Edit", "status": "erro"}));
        assert_eq!(items[1], json!({"kind": "tool", "name": "Read", "status": "pendente"}));
    }

    #[test]
    fn user_turns_lose_claude_code_markers_and_long_text_is_capped() {
        assert_eq!(strip_markers("<command-name>/clear</command-name>\n<command-args></command-args>olá"), "olá");
        assert_eq!(strip_markers("oi <system-reminder>regra interna</system-reminder> tudo"), "oi  tudo");
        assert_eq!(strip_markers("a <local-command-stdout>sem fim"), "a", "sem fechamento, o marcador vai até o fim");
        let s = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"<command-name>/clear</command-name>"}}
{"type":"user","uuid":"u2","message":{"role":"user","content":"<system-reminder>x</system-reminder>pergunta"}}
{"type":"assistant","uuid":"a1","message":{"id":"m1","content":[{"type":"text","text":"<system-reminder>mantido</system-reminder>"}]}}"#;
        let items = claude_items(s);
        assert_eq!(items.len(), 2, "o turno só de marcador some");
        assert_eq!(items[0]["text"], "pergunta");
        assert_eq!(items[1]["text"], "<system-reminder>mantido</system-reminder>", "marcador só sai das falas do usuário");
        let long = "á".repeat(TEXT_MAX_CHARS + 10);
        assert_eq!(cap_chars(&long).chars().count(), TEXT_MAX_CHARS);
    }

    #[test]
    fn tail_starts_at_a_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::fs::write(&p, "linha-um\nlinha-dois\nlinha-tres\n").unwrap();
        assert_eq!(tail_text(&p, 1000).unwrap(), "linha-um\nlinha-dois\nlinha-tres\n");
        // 12 bytes pegam o fim de "linha-dois": a parte cortada some, a última linha inteira fica
        assert_eq!(tail_text(&p, 12).unwrap(), "linha-tres\n");
    }

    #[test]
    fn turns_are_headed_and_tools_collapsed() {
        let md = claude_markdown(SESSION, "agente", false);
        assert!(md.starts_with("# agente\n\n## Você\n\nrode os testes\n\n## Claude\n\nVou rodar.\n\n"), "{md}");
        assert!(md.contains("<summary>Bash: npm test</summary>"), "{md}");
        assert!(!md.contains("12 passed"), "sem ferramentas completas a saída some: {md}");
        assert!(md.contains("Tudo verde."));
        // sidechain, meta, linhas repetidas, thinking e linha inválida ficam de fora
        for hidden in ["subagente", "meta", "repetida", "segredo interno"] {
            assert!(!md.contains(hidden), "{hidden} apareceu: {md}");
        }
    }

    #[test]
    fn full_tools_include_input_and_output() {
        let md = claude_markdown(SESSION, "agente", true);
        assert!(md.contains("**Entrada**"), "{md}");
        assert!(md.contains("\"command\": \"npm test\""), "{md}");
        assert!(md.contains("**Saída**\n\n```\n12 passed\n```"), "{md}");
    }

    #[test]
    fn tool_error_is_labelled() {
        let s = r#"{"type":"assistant","uuid":"x","message":{"content":[{"type":"tool_use","id":"e","name":"Read","input":{"file_path":"C:/a.txt"}}]}}
{"type":"user","uuid":"y","message":{"content":[{"type":"tool_result","tool_use_id":"e","is_error":true,"content":"não existe"}]}}"#;
        let md = claude_markdown(s, "t", true);
        assert!(md.contains("<summary>Read: C:/a.txt</summary>"), "{md}");
        assert!(md.contains("**Erro**"), "{md}");
    }

    #[test]
    fn secrets_are_redacted_everywhere() {
        let s = r#"{"type":"user","uuid":"k","message":{"content":"minha chave é sk-abcdef123456789 e o email eu@x.com.br"}}
{"type":"assistant","uuid":"k2","message":{"content":[{"type":"tool_use","id":"b","name":"Bash","input":{"command":"curl -H 'Authorization: Bearer abc.def.ghi' x"}}]}}"#;
        let md = claude_markdown(s, "t", true);
        assert!(!md.contains("sk-abcdef123456789"), "{md}");
        assert!(!md.contains("eu@x.com.br"), "{md}");
        assert!(!md.contains("abc.def.ghi"), "{md}");
    }

    #[test]
    fn html_in_tool_summary_is_escaped() {
        let s = r#"{"type":"assistant","uuid":"h","message":{"content":[{"type":"tool_use","id":"h1","name":"Bash","input":{"command":"echo </details>"}}]}}"#;
        let md = claude_markdown(s, "t", false);
        assert!(md.contains("<summary>Bash: echo &lt;/details&gt;</summary>"), "{md}");
    }

    #[test]
    fn terminal_fallback_is_a_fenced_block_and_redacted() {
        let md = terminal_markdown("shell", "ls\n```\nsk-abcdef123456789\n");
        assert!(md.starts_with("# shell\n\n````text\n"), "{md}");
        assert!(md.ends_with("\n````\n"), "{md}");
        assert!(!md.contains("sk-abcdef123456789"));
    }

    #[test]
    fn long_summary_is_cut() {
        let long = "x".repeat(300);
        let s = format!(r#"{{"type":"assistant","uuid":"l","message":{{"content":[{{"type":"tool_use","id":"l1","name":"Bash","input":{{"command":"{long}"}}}}]}}}}"#);
        let md = claude_markdown(&s, "t", false);
        assert!(md.contains(&format!("Bash: {}…", "x".repeat(SUMMARY_CHARS))), "{md}");
    }

    #[test]
    fn message_with_details_tag_or_backticks_is_fenced() {
        let jsonl = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"veja </details> e ```x```"}}"#;
        let md = claude_markdown(jsonl, "T", false);
        assert!(md.contains("````text
veja </details> e ```x```
````"), "{md}");
    }

    #[test]
    fn subagent_transcript_projects_its_sidechain_lines() {
        // o arquivo do subagente é todo sidechain: a projeção mostra; a sessão principal não mostra
        let jsonl = r#"{"type":"user","isSidechain":true,"uuid":"u1","message":{"role":"user","content":"ache o bug"}}
{"type":"assistant","isSidechain":true,"uuid":"a1","message":{"id":"m1","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Grep","input":{"pattern":"login"}}]}}
{"type":"user","isSidechain":true,"uuid":"r1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}
{"type":"assistant","isSidechain":true,"uuid":"a2","message":{"id":"m2","role":"assistant","content":[{"type":"text","text":"Achei: login.rs:12"}]}}"#;
        assert_eq!(
            subagent_items(jsonl),
            vec![
                json!({"kind": "text", "role": "voce", "text": "ache o bug"}),
                json!({"kind": "tool", "name": "Grep", "status": "ok"}),
                json!({"kind": "text", "role": "claude", "text": "Achei: login.rs:12"}),
            ]
        );
        assert!(claude_items(jsonl).is_empty(), "a sessão principal não mostra linhas sidechain");
    }

    #[test]
    fn subagent_view_reads_only_subagent_transcripts_and_description() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sess").join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("agent-a1.jsonl");
        std::fs::write(&f, r#"{"type":"assistant","isSidechain":true,"uuid":"a","message":{"id":"m","role":"assistant","content":[{"type":"text","text":"oi"}]}}"#).unwrap();
        std::fs::write(dir.join("agent-a1.meta.json"), r#"{"agentType":"Explore","description":"ache o login"}"#).unwrap();
        let v = subagent_view(&f);
        assert_eq!(v["items"][0]["text"], "oi");
        assert_eq!(v["description"], "ache o login");
        // fora de subagents/ não lê nada
        let other = d.path().join("sess.jsonl");
        std::fs::write(&other, "{\"type\":\"assistant\",\"uuid\":\"x\",\"message\":{\"id\":\"x\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"segredo\"}]}}\n").unwrap();
        assert!(subagent_view(&other)["items"].as_array().unwrap().is_empty());
    }
}
