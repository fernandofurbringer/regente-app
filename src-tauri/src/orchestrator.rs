use crate::db::{Handoff, HelperSession};
use crate::error::{fail, AppError, AppResult};
use crate::runtime::AppState;
use crate::status::PaneStatus;
use crate::util::{new_id, now_ms};
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Bracketed paste + Enter: lets the TUI (Claude/Codex/Gemini) receive multi-line text as one message.
pub fn bracketed(text: &str) -> Vec<u8> {
    let mut v = b"\x1b[200~".to_vec();
    v.extend_from_slice(clean_text(text).as_bytes());
    v.extend_from_slice(b"\x1b[201~\r");
    v
}

/// Bracketed paste without Enter: the text lands on the CLI's input line for the user to review.
pub fn paste_bytes(text: &str, enter: bool) -> Vec<u8> {
    if enter {
        return bracketed(text);
    }
    let mut v = b"\x1b[200~".to_vec();
    v.extend_from_slice(clean_text(text).as_bytes());
    v.extend_from_slice(b"\x1b[201~");
    v
}

/// Text that is typed or pasted into a pane: newlines and tabs stay, every other control char goes
/// (ESC above all, so no `ESC[201~` in the text can end the paste early).
pub fn clean_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect()
}

const STOPWORDS: &[&str] = &[
    "a", "o", "os", "as", "de", "da", "do", "das", "dos", "e", "em", "no", "na", "para", "por", "com", "que", "um", "uma", "se", "ao", "à", "é", "crie",
    "faça", "faca", "implemente", "adicione",
];

/// Short name (at most 4 meaningful words, 40 chars) from a request, for helpers.
pub fn auto_name(request: &str) -> String {
    let mut words: Vec<String> = request
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .take(4)
        .collect();
    if words.is_empty() {
        return "Ajudante".into();
    }
    // ponytail: 40-char cap (not 32: the plan's own example is 36 chars); drops trailing words, then hard-cuts one oversized word
    let mut name = words.join(" ");
    while name.chars().count() > 40 && words.len() > 1 {
        words.pop();
        name = words.join(" ");
    }
    let name: String = name.chars().take(40).collect();
    let mut chars = name.chars();
    let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    format!("{first}{}", chars.as_str())
}

/// A helper that exits with an error this soon after its start is reported to its parent.
pub const EARLY_DEATH: Duration = Duration::from_secs(20);
const CLOSE_AFTER_HANDOFF: Duration = Duration::from_secs(30);
/// A helper idle this long after its prompt, with no handoff, is reported as stalled.
const STALL_AFTER: Duration = Duration::from_secs(20);

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    // CSI, OSC (BEL or ST terminated) and two-byte escapes: the same set the frontend strips
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]").expect("valid regex")
});

pub fn strip_ansi(s: &str) -> String {
    ANSI.replace_all(s, "").into_owned()
}

pub fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// True while Claude's "trust this folder" dialog is the last thing on screen. A prompt typed then would
/// confirm the preselected "No, exit". Whitespace is ignored because the TUI may move the cursor instead of printing spaces.
pub fn trust_dialog_open(tail: &[u8]) -> bool {
    let t: String = last_lines(&strip_ansi(&String::from_utf8_lossy(tail)), 14).split_whitespace().collect();
    t.contains("Yes,Itrustthisfolder") && t.contains("Entertoconfirm")
}

/// Queues `text` for the child's parent (delivered when the parent is idle). Does nothing without a parent.
pub fn notify_parent(s: &AppState, child: &str, text: &str) {
    let Some(parent) = s.db.get_pane(child).ok().and_then(|r| r.parent_id) else { return };
    s.rt().notices.entry(parent.clone()).or_default().push_back(text.to_string());
    s.emit("pane-notice", json!({"parent": parent, "child": child, "text": text}));
    if s.status_of(&parent) == Some(PaneStatus::Hibernated) {
        s.wake(&parent);
    }
}

/// One queued message per idle parent, per tick.
/// Minimum gap between two notices to the same parent (the first one needs the parent to start working).
const NOTICE_GAP: Duration = Duration::from_secs(2);

pub fn deliver_notices(s: &AppState) {
    let now = Instant::now();
    // peek only: the message leaves the queue once the write went through
    let ready: Vec<(String, String)> = {
        let rt = s.rt();
        rt.notices
            .iter()
            .filter(|(p, q)| !q.is_empty() && rt.statuses.get(p.as_str()) == Some(&PaneStatus::Idle))
            .filter(|(p, _)| rt.last_notice_at.get(p.as_str()).is_none_or(|t| now.duration_since(*t) > NOTICE_GAP))
            .filter_map(|(p, q)| q.front().map(|m| (p.clone(), m.clone())))
            .collect()
    };
    for (p, msg) in ready {
        // never paste under a parent's typing
        if s.panes.typed_since_enter(&p) {
            continue;
        }
        if s.panes.write(&p, &bracketed(&format!("[Regente] {msg}"))).is_err() {
            continue;
        }
        let mut rt = s.rt();
        if let Some(q) = rt.notices.get_mut(&p) {
            q.pop_front();
        }
        rt.last_notice_at.insert(p, now);
    }
}

pub struct HandoffInput {
    pub summary: String,
    pub data: Option<Value>,
    pub files: Vec<String>,
}

/// Largest handoff payload (`data` + `files`), in bytes of JSON.
const MAX_HANDOFF_BYTES: usize = 256 * 1024;

/// The schema written at spawn (`panes/<id>/handoff.json`): it survives a respawn or a restart.
fn schema_from_disk(s: &AppState, pane: &str) -> Option<Value> {
    let text = std::fs::read_to_string(s.data.join("panes").join(pane).join("handoff.json")).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("validação de entrega pulada: schema de {pane} ilegível ({e})");
            None
        }
    }
}

/// Limites de uma entrega (RM-20): resumo em caracteres, dados em bytes JSON, arquivos relativos. O estouro
/// vira erro que o agente corrige numa tentativa; nada é cortado em silêncio.
pub const MAX_RESUMO: usize = 400;
pub const MAX_DADOS: usize = 2048;
pub const MAX_ARQUIVOS: usize = 20;

/// Como o pai lê o detalhe: pelo caminho do artefato, se a entrega tem um; senão pela entrega inteira.
pub fn detalhe_entrega(id: &str, data: Option<&Value>) -> String {
    match data.and_then(|d| d.get("artifact")).and_then(|a| a.get("path")).and_then(Value::as_str) {
        Some(p) => format!("artifact_read path={p}"),
        None => format!("handoff_get id={id} detail=true"),
    }
}

pub fn checar_limites(summary: &str, data: Option<&Value>, files: &[String]) -> AppResult<()> {
    let n = summary.chars().count();
    if n > MAX_RESUMO {
        return fail("summary_too_long", format!("Resumo com {n} caracteres; reduza para {MAX_RESUMO} e coloque o detalhe num artefato (campo artifact)."));
    }
    // o campo `artifact` (caminho e resumo do arquivo) não conta: o detalhe fica no arquivo
    let bytes = match data {
        Some(Value::Object(m)) => {
            let mut sem = m.clone();
            sem.remove("artifact");
            Value::Object(sem).to_string().len()
        }
        Some(d) => d.to_string().len(),
        None => 0,
    };
    if bytes > MAX_DADOS {
        return fail("data_too_long", format!("Dados com {bytes} bytes; o limite é {MAX_DADOS}. Reduza ou coloque o detalhe num artefato (campo artifact)."));
    }
    if files.len() > MAX_ARQUIVOS {
        return fail("files_too_many", format!("Arquivos: {}; o máximo é {MAX_ARQUIVOS}.", files.len()));
    }
    for f in files {
        if Path::new(f).components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            return fail("bad_file_path", format!("arquivo fora da pasta do painel: {f}; use caminho relativo"));
        }
    }
    Ok(())
}

/// Schema de entrega de um painel: vai ao disco (sobrevive ao respawn) e ao cache da validação.
pub fn fixar_schema(s: &AppState, pane: &str, schema: Value) -> AppResult<()> {
    let dir = s.data.join("panes").join(pane);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("handoff.json"), schema.to_string())?;
    s.rt().handoff_schemas.insert(pane.into(), schema);
    Ok(())
}

pub fn submit_handoff(s: &AppState, caller: &str, input: HandoffInput) -> AppResult<Handoff> {
    checar_limites(&input.summary, input.data.as_ref(), &input.files)?;
    let row = s.db.get_pane(caller)?;
    let data = input.data.clone().unwrap_or(Value::Null);
    let data_json = input.data.as_ref().map(Value::to_string);
    let files_json = (!input.files.is_empty()).then(|| serde_json::to_string(&input.files)).transpose()?;
    if data_json.as_ref().map_or(0, String::len) + files_json.as_ref().map_or(0, String::len) > MAX_HANDOFF_BYTES {
        return fail("handoff_too_big", "entrega acima de 256 KB (dados + arquivos)");
    }
    // the runtime guard is released before the schema file is read
    let cached = s.rt().handoff_schemas.get(caller).cloned();
    let schema = cached.or_else(|| schema_from_disk(s, caller));
    if let Some(schema) = schema {
        let validator = jsonschema::validator_for(&schema).map_err(|e| AppError::new("handoff_invalid", format!("schema inválido: {e}")))?;
        let errors: Vec<String> = validator.iter_errors(&data).map(|e| e.to_string()).collect();
        if !errors.is_empty() {
            return fail("handoff_invalid", errors.join("; "));
        }
    }
    let n = s.db.handoffs(row.mission_id.as_deref(), None, 1000)?.iter().filter(|h| h.from_pane == caller).count() + 1;
    let h = Handoff {
        id: new_id(),
        mission_id: row.mission_id.clone(),
        from_pane: caller.into(),
        to_pane: row.parent_id.clone(),
        summary: input.summary,
        data_json,
        files_json,
        created_at: now_ms(),
        seen: false,
    };
    s.db.insert_handoff(&h)?;
    // memória: a entrega vira sugestão de fato (RM-32); a revisão fica em Memória → Sugestões
    crate::memory::suggest(s, &row.workspace_id, row.mission_id.as_deref(), "fato", &crate::memory::texto_entrega(&h.summary, &input.files));
    // o pai lê o detalhe pelo caminho do artefato, se houver; senão pela entrega inteira
    notify_parent(s, caller, &format!("Entrega de {} (#{n}): {} — detalhes: {}", row.name, h.summary, detalhe_entrega(&h.id, input.data.as_ref())));
    s.emit("handoff", json!({"missionId": h.mission_id, "handoff": h}));
    // a child closes itself after its handoff, unless it is a pilot, maestro or chief (they stay open)
    if row.parent_id.is_some() && !row.role.as_deref().is_some_and(crate::mcp::protected_role) {
        s.rt().close_at.insert(caller.into(), Instant::now() + CLOSE_AFTER_HANDOFF);
    }
    if let (Some(task_type), Some(mission)) = (row.task_type.clone(), row.mission_id.clone()) {
        if row.role.as_deref() == Some("ajudante") {
            s.db.put_helper_session(&HelperSession { task_type, mission_id: mission, pane_id: caller.into(), session_id: row.session_id.clone(), cli: row.cli.clone(), closed_at: now_ms() })?;
        }
    }
    Ok(h)
}

/// Periodic work of the status ticker: auto-close after handoffs, stalled helpers, queued notices.
pub fn tick(s: &AppState) {
    crate::memory::expire_tick(s);
    let now = Instant::now();
    let due: Vec<String> = s.rt().close_at.iter().filter(|(_, t)| now >= **t).map(|(p, _)| p.clone()).collect();
    for pane in due {
        s.rt().close_at.remove(&pane);
        // still working, or the user is typing into it: not now, check again in 30 s
        let busy = s.status_of(&pane) != Some(PaneStatus::Idle) || s.panes.typed_since_enter(&pane);
        if busy {
            s.rt().close_at.insert(pane, now + CLOSE_AFTER_HANDOFF);
        } else {
            let _ = s.close_pane(&pane);
        }
    }
    let stalled: Vec<String> = {
        let mut rt = s.rt();
        let due: Vec<String> = rt
            .prompted_at
            .iter()
            .filter(|(p, t)| now.duration_since(**t) > STALL_AFTER && !rt.stalled_notified.contains(*p) && !rt.close_at.contains_key(*p) && rt.statuses.get(*p) == Some(&PaneStatus::Idle))
            .map(|(p, _)| p.clone())
            .collect();
        rt.stalled_notified.extend(due.iter().cloned());
        due
    };
    for pane in stalled {
        let name = s.db.get_pane(&pane).map(|r| r.name).unwrap_or_default();
        notify_parent(s, &pane, &format!("{name} está ocioso sem entregar. Veja com pane_read ou responda com pane_write."));
    }
    deliver_notices(s);
}

/// The CLI asked something (`Notification` hook). Repeats of the same question are not reported again.
pub fn on_question(s: &AppState, pane: &str, message: &str) {
    {
        let mut rt = s.rt();
        if rt.asked.get(pane).is_some_and(|m| m == message) {
            return;
        }
        rt.asked.insert(pane.into(), message.into());
    }
    let name = s.db.get_pane(pane).map(|r| r.name).unwrap_or_default();
    notify_parent(s, pane, &format!("{name} aguarda resposta: {message}"));
}

/// A helper died early with an error: the parent gets the code and the last lines of its output.
pub fn on_early_death(s: &AppState, pane: &str, code: u32, tail: &[u8]) {
    let row = s.db.get_pane(pane).ok();
    let name = row.as_ref().map(|r| r.name.clone()).unwrap_or_default();
    let plain = strip_ansi(&String::from_utf8_lossy(tail));
    // a CLI that refuses the model says so before it exits: the app offers the quick model switch.
    // Só painel de IA que saiu com erro (não um terminal, nem uma saída 0).
    if let (Some(r), true) = (row.as_ref(), code != 0) {
        // uma vez por painel: a vigília de modelo (runtime::tick_model_errors) avisa o mesmo erro
        if r.kind == "agent" && model_refused(&plain) && s.rt().model_warned.insert(pane.to_string()) {
            let label = r.account.clone().unwrap_or_else(|| "Principal".into());
            s.emit("model-unavailable", json!({"paneId": pane, "workspaceId": r.workspace_id, "model": r.model, "accountLabel": label}));
        }
    }
    let text = last_lines(&plain, 20);
    notify_parent(s, pane, &format!("{name} encerrou logo após abrir (código {code}). Últimas linhas:\n{text}"));
}

/// Wording a CLI uses when the account cannot use the requested model (checked lowercase, on the same line as "model").
const MODEL_REFUSED: [&str; 5] = ["model not found", "invalid model", "does not have access", "unknown model", "not available"];

/// True when a line of the early output says its model was refused: the line must name the model too,
/// so "does not have access to repo" is not a model refusal.
pub fn model_refused(text: &str) -> bool {
    text.lines().any(|line| {
        let l = line.to_lowercase();
        l.contains("model") && MODEL_REFUSED.iter().any(|p| l.contains(p))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PaneRow;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn state() -> (tempfile::TempDir, Arc<AppState>) {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        (d, s)
    }

    fn pane(id: &str, parent: Option<&str>, mission: Option<&str>, role: Option<&str>, task: Option<&str>) -> PaneRow {
        PaneRow {
            id: id.into(),
            workspace_id: "w".into(),
            mission_id: mission.map(Into::into),
            kind: "agent".into(),
            name: id.into(),
            permission: "pend".into(),
            cwd: "C:/".into(),
            status: "ocioso".into(),
            parent_id: parent.map(Into::into),
            role: role.map(Into::into),
            task_type: task.map(Into::into),
            ..Default::default()
        }
    }

    /// Parent `p` (idle) and child `c` in mission `m`.
    fn family(role: Option<&str>, task: Option<&str>) -> (tempfile::TempDir, Arc<AppState>) {
        let (d, s) = state();
        s.db.insert_pane(&pane("p", None, Some("m"), Some("piloto"), None)).unwrap();
        s.db.insert_pane(&pane("c", Some("p"), Some("m"), role, task)).unwrap();
        s.rt().statuses.insert("p".into(), PaneStatus::Idle);
        (d, s)
    }

    fn input(summary: &str, data: Option<Value>) -> HandoffInput {
        HandoffInput { summary: summary.into(), data, files: vec![] }
    }

    #[test]
    fn trust_dialog_is_detected_even_without_spaces() {
        let screen = ["{e}[1m> No, exit", "   Yes,{e}[1CI{e}[1Ctrust this folder", " Enter to confirm - Esc to cancel"].join("\r\n").replace("{e}", "\u{1b}");
        assert!(trust_dialog_open(screen.as_bytes()));
        assert!(!trust_dialog_open(b"> hello"));
    }

    #[test]
    fn bracketed_paste_normalizes_newlines() {
        assert_eq!(bracketed("a\r\nb"), b"\x1b[200~a\nb\x1b[201~\r".to_vec());
    }

    #[test]
    fn paste_without_enter_has_no_carriage_return() {
        assert_eq!(paste_bytes("a\r\nb", false), b"\x1b[200~a\nb\x1b[201~".to_vec());
        assert_eq!(paste_bytes("a", true), bracketed("a"));
    }

    #[test]
    fn auto_name_examples() {
        assert_eq!(auto_name("Crie a página de login com Google"), "Página login google");
        assert_eq!(auto_name("Revisar segurança do endpoint de pagamento"), "Revisar segurança endpoint pagamento");
        assert_eq!(auto_name(""), "Ajudante");
        assert!(auto_name(&"palavra ".repeat(20)).chars().count() <= 40);
    }

    #[test]
    fn strip_and_last_lines() {
        assert_eq!(strip_ansi("\x1b[31mA\x1b[0m\nB\nC"), "A\nB\nC");
        assert_eq!(strip_ansi("\x1b]0;título\x07ok\x1b]8;;x\x1b\\"), "ok");
        assert_eq!(last_lines("A\nB\nC", 2), "B\nC");
        assert_eq!(last_lines("A", 5), "A");
    }

    #[test]
    fn handoff_notifies_parent_and_schedules_close() {
        let (_d, s) = family(Some("ajudante"), None);
        let h = submit_handoff(&s, "c", input("pronto", None)).unwrap();
        assert_eq!(s.db.get_handoff(&h.id).unwrap().to_pane.as_deref(), Some("p"));
        let rt = s.rt();
        assert!(rt.notices["p"].iter().any(|m| m.contains("Entrega de")));
        assert!(rt.close_at.contains_key("c"));
    }

    #[test]
    fn schema_rejects_invalid() {
        let (_d, s) = family(None, None);
        s.rt().handoff_schemas.insert("c".into(), json!({"type": "object", "required": ["url"]}));
        assert_eq!(submit_handoff(&s, "c", input("x", Some(json!({})))).unwrap_err().code(), "handoff_invalid");
        // None counts as null, which is not an object either
        assert_eq!(submit_handoff(&s, "c", input("x", None)).unwrap_err().code(), "handoff_invalid");
        assert!(submit_handoff(&s, "c", input("x", Some(json!({"url": "x"})))).is_ok());
    }

    #[test]
    fn question_dedup() {
        let (_d, s) = family(None, None);
        on_question(&s, "c", "Permitir?");
        on_question(&s, "c", "Permitir?");
        assert_eq!(s.rt().notices["p"].len(), 1);
    }

    #[test]
    fn a_refused_model_is_recognised_in_the_early_output() {
        assert!(model_refused("Error: model not found: claude-opus-9\n"));
        assert!(model_refused("Invalid model 'x' for this request"));
        assert!(model_refused("Your account does not have access to this model"));
        assert!(!model_refused("Claude Code pronto, modelo padrão"));
        assert!(!model_refused("the model is ready"));
        assert!(model_refused("model claude-x is not available for this account"));
        // negativos: sem "model" na mesma linha, ou saída que não é de refusão
        assert!(!model_refused("Your account does not have access to repo acme"));
        assert!(!model_refused("model ok\nyou does not have access to repo"), "frases em linhas diferentes não contam");
    }

    #[test]
    fn only_an_agent_pane_that_exits_with_an_error_raises_the_model_warning() {
        let (_d, s) = state();
        let mut agent = pane("ag", None, None, None, None);
        agent.model = Some("claude-x".into());
        agent.account = Some("trabalho".into());
        s.db.insert_pane(&agent).unwrap();
        let mut term = pane("tm", None, None, None, None);
        term.kind = "terminal".into();
        s.db.insert_pane(&term).unwrap();
        let refusal = b"Error: model not found: claude-x\n";
        let count = |id: &str, code: u32| {
            let before = s.latest_seq();
            on_early_death(&s, id, code, refusal);
            s.events_since(before, Some("model-unavailable"), 10).len()
        };
        assert_eq!(count("tm", 1), 0, "terminal não avisa");
        assert_eq!(count("ag", 0), 0, "saída 0 não avisa");
        assert_eq!(count("ag", 1), 1, "agente com erro avisa");
    }

    #[test]
    fn pasted_text_cannot_end_the_paste_early() {
        let out = bracketed("a\x1b[201~b\x07\u{9b}\x7f\tc\r\nd");
        let s = String::from_utf8_lossy(&out).to_string();
        assert_eq!(s.matches("\x1b[200~").count(), 1, "{s:?}");
        assert_eq!(s.matches("\x1b[201~").count(), 1, "{s:?}");
        // between the markers: no ESC, no other control char but the tab and the newline
        let inner = &s[6..s.len() - 7];
        assert!(!inner.contains('\x1b') && !inner.contains('\x07') && !inner.contains('\u{9b}') && !inner.contains('\x7f'), "{inner:?}");
        assert!(inner.contains('\t') && inner.contains('\n'), "{inner:?}");
    }

    /// A real (idle) cmd process as pane `id`, so typing and writes reach a live pane.
    fn live(s: &AppState, id: &str) {
        let plan = crate::pty::SpawnPlan { program: "cmd.exe".into(), args: vec!["/k".into(), "rem".into()], cwd: std::env::temp_dir().to_string_lossy().into(), ..Default::default() };
        s.panes.spawn(id, &plan, 80, 24).unwrap();
    }

    #[test]
    fn notices_wait_while_the_parent_is_typing() {
        let (_d, s) = family(None, None);
        live(&s, "p");
        s.rt().statuses.insert("p".into(), PaneStatus::Idle);
        s.panes.write("p", b"abc").unwrap();
        on_question(&s, "c", "Permitir?");
        deliver_notices(&s);
        assert_eq!(s.rt().notices["p"].len(), 1, "typing: the notice stays queued");
        s.panes.write("p", b"\r").unwrap();
        deliver_notices(&s);
        assert!(s.rt().notices["p"].is_empty(), "after Enter it goes");
        s.panes.kill("p").unwrap();
    }

    #[test]
    fn notices_to_one_parent_are_spaced_out() {
        let (_d, s) = family(None, None);
        live(&s, "p");
        s.rt().statuses.insert("p".into(), PaneStatus::Idle);
        on_question(&s, "c", "um?");
        on_question(&s, "c", "dois?");
        deliver_notices(&s);
        assert_eq!(s.rt().notices["p"].len(), 1);
        deliver_notices(&s);
        assert_eq!(s.rt().notices["p"].len(), 1, "within the 2 s gap: nothing new");
        s.panes.kill("p").unwrap();
    }

    #[test]
    fn children_close_after_handoff_except_chiefs() {
        // a helper and an agent_invoke child (role = the agent's role) close
        for role in ["ajudante", "executor"] {
            let (_d, s) = family(Some(role), None);
            submit_handoff(&s, "c", input("pronto", None)).unwrap();
            assert!(s.rt().close_at.contains_key("c"), "{role} closes after its handoff");
        }
        // a chief stays open
        let (_d, s) = family(Some("chefe:builder"), None);
        submit_handoff(&s, "c", input("pronto", None)).unwrap();
        assert!(!s.rt().close_at.contains_key("c"), "chefe:builder stays open");
    }

    #[test]
    fn close_is_only_scheduled_for_helpers() {
        let (_d, s) = state();
        s.db.insert_pane(&pane("lone", None, Some("m"), None, None)).unwrap();
        submit_handoff(&s, "lone", input("x", None)).unwrap();
        assert!(!s.rt().close_at.contains_key("lone"));
    }

    #[test]
    fn tick_closes_an_idle_helper_and_rearms_a_busy_one() {
        let (_d, s) = family(None, None);
        let past = Instant::now() - Duration::from_secs(1);
        s.rt().close_at.insert("c".into(), past);
        s.rt().statuses.insert("c".into(), PaneStatus::Working);
        tick(&s);
        assert!(s.rt().close_at["c"] > Instant::now(), "busy: checked again later");
        assert!(s.db.get_pane("c").unwrap().closed_at.is_none());
        s.rt().statuses.insert("c".into(), PaneStatus::Idle);
        s.rt().close_at.insert("c".into(), past);
        tick(&s);
        assert!(s.db.get_pane("c").unwrap().closed_at.is_some(), "idle: closed");
    }

    #[test]
    fn helper_session_recorded() {
        let (_d, s) = family(Some("ajudante"), Some("testes"));
        submit_handoff(&s, "c", input("ok", None)).unwrap();
        assert!(s.db.helper_session("testes", "m").unwrap().is_some());
    }

    #[test]
    fn handoff_limits_refuse_over_and_accept_exact() {
        let ok = |s: &str| checar_limites(s, None, &[]);
        assert!(ok(&"a".repeat(MAX_RESUMO)).is_ok());
        let e = ok(&"é".repeat(612)).unwrap_err();
        assert_eq!(e.code(), "summary_too_long");
        assert!(e.to_string().starts_with("Resumo com 612 caracteres; reduza para 400"), "{e}");
        let grande = json!({"x": "a".repeat(MAX_DADOS)});
        assert_eq!(checar_limites("ok", Some(&grande), &[]).unwrap_err().code(), "data_too_long");
        assert!(checar_limites("ok", Some(&json!({"x": "a"})), &[]).is_ok());
        let muitos: Vec<String> = (0..21).map(|i| format!("f{i}.rs")).collect();
        assert_eq!(checar_limites("ok", None, &muitos).unwrap_err().code(), "files_too_many");
        assert!(checar_limites("ok", None, &muitos[..20]).is_ok());
        for bad in ["/etc/x", "../fora.rs", "C:/x.rs"] {
            assert_eq!(checar_limites("ok", None, &[bad.to_string()]).unwrap_err().code(), "bad_file_path", "{bad}");
        }
        assert!(checar_limites("ok", None, &["src/a.rs".to_string()]).is_ok());
        // o campo artifact não conta no limite de dados; o resto continua contando
        let so_artefato = json!({"artifact": {"path": ".regente/artefatos/m/x.md", "summary": "a".repeat(MAX_DADOS * 2)}});
        assert!(checar_limites("ok", Some(&so_artefato), &[]).is_ok());
        let misto = json!({"artifact": {"path": "p"}, "x": "a".repeat(MAX_DADOS)});
        assert_eq!(checar_limites("ok", Some(&misto), &[]).unwrap_err().code(), "data_too_long");
        // o detalhe do aviso ao pai: artefato quando há, senão handoff_get com detail=true
        assert_eq!(detalhe_entrega("h1", Some(&json!({"artifact": {"path": ".regente/artefatos/m/x.md"}}))), "artifact_read path=.regente/artefatos/m/x.md");
        assert_eq!(detalhe_entrega("h1", Some(&json!({"k": 1}))), "handoff_get id=h1 detail=true");
        assert_eq!(detalhe_entrega("h1", None), "handoff_get id=h1 detail=true");
    }
}
