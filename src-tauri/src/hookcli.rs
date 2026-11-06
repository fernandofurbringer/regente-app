//! `Regente.exe --hook <Evento>`: chamado pelos hooks do Claude Code. Lê o JSON do evento em stdin e
//! repassa ao app pela rota `/hook`. Sempre sai com 0 e nunca escreve em stdout (não pode interferir na IA).
//! Tudo roda numa thread com prazo de 2 s: se o app não responder (ou o stdin não fechar), a IA não espera.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(2);

pub fn run(args: &[String]) -> i32 {
    let args = args.to_vec();
    let inicio = Instant::now();
    let (saida_tx, saida_rx) = mpsc::channel::<String>();
    let (fim_tx, fim_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        forward(&args, saida_tx);
        let _ = fim_tx.send(());
    });
    // a saída (resumo de ferramenta) sai antes do POST: o app pode demorar, a saída não espera por ele
    if let Ok(out) = saida_rx.recv_timeout(DEADLINE) {
        let _ = writeln!(std::io::stdout(), "{out}");
    }
    let _ = fim_rx.recv_timeout(DEADLINE.saturating_sub(inicio.elapsed()));
    0
}

/// Encaminha o evento ao app e, quando é o caso, manda a saída que o Claude Code deve usar (só stdout do JSON).
/// Qualquer falha fica em silêncio: sem saída, o Claude Code usa o resultado original.
fn forward(args: &[String], saida: mpsc::Sender<String>) -> Option<()> {
    let event = args.iter().position(|a| a == "--hook").and_then(|i| args.get(i + 1))?;
    let url = std::env::var("REGENTE_HOOK_URL").ok()?;
    let token = std::env::var("REGENTE_MCP_TOKEN").ok()?;
    let pane = std::env::var("REGENTE_PANE_ID").ok()?;
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let payload: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    if let Some(out) = saida_de(event, &payload) {
        let _ = saida.send(out);
    }
    let body = corpo(event, &pane, payload);
    post(&url, &token, &body, DEADLINE)
}

/// Saída do hook para o Claude Code, quando o perfil pede: resumo do `PostToolUse` de shell (`REGENTE_TRIM_CHARS`)
/// ou digest de falha (`REGENTE_DIGEST_FALHAS=1`, `PostToolUseFailure`). Nunca troca um campo que não entende.
fn saida_de(event: &str, payload: &Value) -> Option<String> {
    if !crate::trim::e_shell(payload["tool_name"].as_str()?) {
        return None;
    }
    let command = payload["tool_input"]["command"].as_str().unwrap_or_default();
    match event {
        "PostToolUse" => {
            let resumir_acima: usize = std::env::var("REGENTE_TRIM_CHARS").ok()?.parse().ok()?;
            let resposta = payload["tool_response"].as_object()?;
            // interrompido ou imagem: o resultado não é uma saída de texto comum, fica como veio
            if resposta.get("interrupted").and_then(Value::as_bool) == Some(true) || resposta.get("isImage").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            let stdout = resposta.get("stdout")?.as_str()?;
            let cfg = crate::trim::Cfg { resumir_acima, ..Default::default() };
            let mut t = crate::trim::trim(payload["tool_name"].as_str()?, command, stdout, &cfg)?;
            // a raiz vem do app (workspace, no spawn), não do `cwd` que o agente informa; sem arquivo, a linha do caminho some
            let id = payload["tool_use_id"].as_str()?;
            match std::env::var("REGENTE_SAIDAS_DIR").ok().and_then(|raiz| crate::trim::guardar(&raiz, id, stdout)) {
                Some(arquivo) => t.text.push_str(&format!("\n[saída completa: {}, {} linhas omitidas]", arquivo.display(), t.omitted_lines)),
                None => t.text.push_str(&format!("\n[{} linhas omitidas]", t.omitted_lines)),
            }
            let mut tr = payload["tool_response"].clone();
            tr["stdout"] = json!(t.text);
            Some(json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "updatedToolOutput": tr}}).to_string())
        }
        "PostToolUseFailure" if std::env::var("REGENTE_DIGEST_FALHAS").ok().as_deref() == Some("1") => {
            let erro = payload["error"].as_str()?;
            Some(json!({"hookSpecificOutput": {"hookEventName": "PostToolUseFailure", "additionalContext": crate::trim::digest_falha(command, erro)}}).to_string())
        }
        _ => None,
    }
}

/// POST JSON para o `/hook` do app (`http://127.0.0.1:PORTA/hook`), com o token do painel. Sem resposta no prazo = None.
pub fn post(url: &str, token: &str, body: &str, prazo: Duration) -> Option<()> {
    // http://127.0.0.1:PORT/hook -> address and path
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}")))?;
    let addr: SocketAddr = host.parse().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, prazo).ok()?;
    s.set_read_timeout(Some(prazo)).ok()?;
    s.set_write_timeout(Some(prazo)).ok()?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut resp = Vec::new();
    let _ = s.read_to_end(&mut resp);
    Some(())
}

/// Limites do payload de shell (o servidor recusa corpo acima de 256 KB): a saída fica com o fim, onde estão o
/// resultado e o marcador de saída; o comando e o erro ficam com o começo, onde está a primeira linha.
const SAIDA_MAX: usize = 96 * 1024;
const COMANDO_MAX: usize = 4 * 1024;
/// Teto do corpo já em JSON (o escape de `` e aspas ocupa bytes a mais que o texto).
const CORPO_MAX: usize = 200 * 1024;

/// Corpo do POST: o evento com o payload já encolhido. Se o JSON ainda passar de `CORPO_MAX` (escape), os tetos de
/// saída são divididos ao meio até caber.
fn corpo(event: &str, pane: &str, payload: Value) -> String {
    let mut teto = SAIDA_MAX;
    loop {
        let mut p = payload.clone();
        encolhe(&mut p, teto);
        let body = json!({"event": event, "pane": pane, "payload": p}).to_string();
        if body.len() <= CORPO_MAX {
            return body;
        }
        if teto <= 1024 {
            // ainda grande no fim da divisão: nenhum campo longo passa, nem os que `encolhe` não conhece
            let mut p = payload.clone();
            encolhe(&mut p, teto);
            omite_grandes(&mut p);
            return json!({"event": event, "pane": pane, "payload": p}).to_string();
        }
        teto /= 2;
    }
}

/// Campo de texto acima disto vira `[omitido: N bytes]` (só no último recurso de `corpo`).
const CAMPO_MAX: usize = 16 * 1024;

fn omite_grandes(v: &mut Value) {
    if let Some(n) = v.as_str().map(str::len).filter(|n| *n > CAMPO_MAX) {
        *v = json!(format!("[omitido: {n} bytes]"));
        return;
    }
    match v {
        Value::Object(m) => m.values_mut().for_each(omite_grandes),
        Value::Array(a) => a.iter_mut().for_each(omite_grandes),
        _ => {}
    }
}

fn encolhe(payload: &mut Value, teto: usize) {
    if let Some(s) = payload["tool_input"]["command"].as_str().map(|s| primeiros(s, COMANDO_MAX)) {
        payload["tool_input"]["command"] = json!(s);
    }
    for campo in ["stdout", "stderr"] {
        if let Some(s) = payload["tool_response"][campo].as_str().map(|s| ultimos(s, teto)) {
            payload["tool_response"][campo] = json!(s);
        }
    }
    if let Some(s) = payload["tool_response"].as_str().map(|s| ultimos(s, teto)) {
        payload["tool_response"] = json!(s);
    }
    if let Some(s) = payload["error"].as_str().map(|s| primeiros(s, teto)) {
        payload["error"] = json!(s);
    }
}

/// Os primeiros `max` bytes (em fronteira de caractere).
fn primeiros(s: &str, max: usize) -> String {
    let mut fim = max.min(s.len());
    while !s.is_char_boundary(fim) {
        fim -= 1;
    }
    s[..fim].to_string()
}

/// Os últimos `max` bytes (em fronteira de caractere).
fn ultimos(s: &str, max: usize) -> String {
    let mut ini = s.len().saturating_sub(max);
    while !s.is_char_boundary(ini) {
        ini += 1;
    }
    s[ini..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_megabyte_of_output_is_cut_to_its_end_and_the_body_stays_under_the_limit() {
        let grande = format!("{}FIM\n[exited with code 0]", "x".repeat(1024 * 1024));
        let p = json!({"tool_name": "Bash", "tool_input": {"command": "c".repeat(10_000)}, "tool_response": {"stdout": grande, "stderr": "e".repeat(1024 * 1024)}, "session_id": "s"});
        let body = corpo("PostToolUse", "pane", p);
        assert!(body.len() < 256 * 1024, "{}", body.len());
        let v: Value = serde_json::from_str(&body).unwrap();
        let out = v["payload"]["tool_response"]["stdout"].as_str().unwrap();
        assert_eq!(out.len(), SAIDA_MAX);
        assert!(out.ends_with("[exited with code 0]"), "o marcador de saída fica");
        assert_eq!(v["payload"]["tool_input"]["command"].as_str().unwrap().len(), COMANDO_MAX);
        assert_eq!(v["payload"]["session_id"], "s", "o resto do payload fica igual");
    }

    #[test]
    fn a_long_error_keeps_its_first_line_with_the_exit_code() {
        let p = json!({"tool_name": "Bash", "error": format!("Exit code 3\n{}", "z".repeat(1024 * 1024))});
        let v: Value = serde_json::from_str(&corpo("PostToolUseFailure", "pane", p)).unwrap();
        assert!(v["payload"]["error"].as_str().unwrap().starts_with("Exit code 3\n"));
    }

    #[test]
    fn escape_heavy_output_still_fits_the_body_limit() {
        // ESC vira \u001b (6 bytes) e a quebra de linha vira \n (2 bytes): o corpo passa do teto de texto
        let ansi = "\x1b[31m".repeat(1024 * 1024 / 5);
        let nl = "\n\"".repeat(1024 * 1024 / 2);
        let p = json!({"tool_name": "Bash", "tool_response": {"stdout": ansi, "stderr": nl.clone()}, "error": nl});
        let body = corpo("PostToolUseFailure", "pane", p);
        assert!(body.len() <= CORPO_MAX, "{}", body.len());
        assert!(serde_json::from_str::<Value>(&body).is_ok());
    }

    #[test]
    fn cuts_never_split_a_character() {
        let s = "é".repeat(10);
        assert_eq!(ultimos(&s, 5).chars().count(), 2);
        assert_eq!(primeiros(&s, 5).chars().count(), 2);
    }

    #[test]
    fn an_unknown_huge_field_is_dropped_so_the_body_fits() {
        let body = corpo("PostToolUse", "P1", json!({"tool_name": "Bash", "extra": "x".repeat(300_000)}));
        assert!(body.len() <= CORPO_MAX, "{}", body.len());
        assert!(body.contains("[omitido: 300000 bytes]"));
    }
}
