use super::{blocking, caller_row, finish, note, ok_json, require_top, tool_err, RegenteMcp};
use crate::diag::redact;
use crate::error::{fail, AppError, AppResult};
use crate::runtime::{AppState, EventRec};
use crate::util::now_ms;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Log bytes read from the end of the newest log file (the tail is all `app_logs` needs).
const LOG_TAIL_BYTES: u64 = 256 * 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventsArgs {
    /// Devolve eventos com seq maior que este (padrão 0: os mais antigos guardados).
    pub since_seq: Option<u64>,
    /// Só eventos deste tipo (ex.: pane-status).
    pub kind: Option<String>,
    /// Quantos eventos (padrão 200, máximo 500).
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WatchArgs {
    /// Espera eventos com seq maior que este.
    pub since_seq: u64,
    /// Tempo máximo de espera em ms (padrão 15000, máximo 30000).
    pub timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct LogsArgs {
    /// Quantas linhas do fim do log (padrão 200, máximo 500).
    pub lines: Option<usize>,
}

fn cap_err(e: impl std::fmt::Display) -> AppError {
    AppError::new("capture_failed", e.to_string())
}

/// The newest `regente.log*` file in the logs folder.
fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("regente.log"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// The last `n` lines of a file, reading only its tail.
fn tail_lines(path: &Path, n: usize) -> AppResult<Vec<String>> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)))?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    Ok(lines[lines.len().saturating_sub(n)..].iter().map(|l| l.to_string()).collect())
}

/// Screenshots kept in `<data>/screens`; older ones are deleted after each capture.
const SCREENS_KEEP: usize = 10;

/// Screenshot of Regente's own window only: matched by process id (this process), falling back to an exact app
/// name when the pid is unavailable. Never another app and never the monitor. Saved as PNG under `<data>/screens`.
fn capture(dir: &Path) -> AppResult<PathBuf> {
    let me = std::process::id();
    let windows = xcap::Window::all().map_err(cap_err)?;
    let own = windows.into_iter().find(|w| match w.pid() {
        Ok(pid) => pid == me,
        Err(_) => w.app_name().is_ok_and(|n| n == "regente"),
    });
    let Some(w) = own else { return fail("no_window", "janela do Regente não encontrada") };
    if w.is_minimized().map_err(cap_err)? {
        return fail("minimized", "a janela do Regente está minimizada");
    }
    let img = w.capture_image().map_err(cap_err)?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.png", now_ms()));
    img.save(&path).map_err(cap_err)?;
    prune_screens(dir, SCREENS_KEEP);
    Ok(path)
}

/// Keeps the newest `keep` PNGs (names are capture timestamps).
fn prune_screens(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut shots: Vec<(i64, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.parse().ok()?, p)))
        .collect();
    shots.sort();
    let extra = shots.len().saturating_sub(keep);
    for (_, p) in shots.into_iter().take(extra) {
        let _ = std::fs::remove_file(p);
    }
}

/// Reply of the event feeds: the events, the latest seq, and `dropped` when `since` is older than the ring.
fn feed(s: &AppState, since: u64, events: Vec<EventRec>) -> Value {
    let oldest = s.events_since(0, None, 1).first().map(|e| e.seq);
    json!({"events": events, "latestSeq": s.latest_seq(), "dropped": oldest.is_some_and(|o| since + 1 < o)})
}

/// Workspace of an event, from its `workspaceId`, its mission, or its pane (looked up once per call).
fn event_workspace(s: &AppState, data: &Value, memo: &mut HashMap<String, Option<String>>) -> Option<String> {
    if let Some(ws) = data["workspaceId"].as_str() {
        return Some(ws.into());
    }
    if let Some(m) = data["missionId"].as_str() {
        return memo.entry(format!("m:{m}")).or_insert_with(|| s.db.get_mission(m).ok().map(|x| x.workspace_id)).clone();
    }
    let pane = ["paneId", "id", "pane", "parent"].iter().find_map(|k| data[*k].as_str())?;
    memo.entry(format!("p:{pane}")).or_insert_with(|| s.db.get_pane(pane).ok().map(|r| r.workspace_id)).clone()
}

/// The events of one workspace. An event whose workspace cannot be resolved is dropped (there are no app-level kinds yet).
fn scoped(s: &AppState, ws: &str, events: Vec<EventRec>) -> Vec<EventRec> {
    let mut memo = HashMap::new();
    events.into_iter().filter(|e| event_workspace(s, &e.data, &mut memo).as_deref() == Some(ws)).collect()
}

#[tool_router(router = diag_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Estado do Regente: versão e quantidade de painéis abertos.
    #[tool(description = "Estado do Regente: versão e quantidade de painéis abertos")]
    async fn app_health(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "app_health", &parts);
        ok_json(&json!({"ok": true, "version": env!("CARGO_PKG_VERSION"), "panes": self.state.panes.ids().len()}))
    }

    /// Estado geral: painéis por status, missões ativas, gates abertos e agendamentos.
    #[tool(description = "Estado geral do Regente: painéis por status, missões ativas, gates abertos e agendamentos")]
    async fn app_status(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "app_status", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            require_top(&me)?;
            let ws = me.workspace_id.as_str();
            let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
            for p in s.list_panes(ws)? {
                *by_status.entry(p.status).or_default() += 1;
            }
            let active = s.db.list_missions(ws)?.iter().filter(|m| m.status == "ativa").count();
            Ok(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "uptimeSec": s.uptime().as_secs(),
                "mcpPort": s.mcp.get().map(|m| m.port),
                "panes": {"byStatus": by_status},
                "missions": {"active": active},
                "gatesOpen": s.db.open_gates(Some(ws))?.len(),
                "cronJobs": s.db.cron_jobs(Some(ws))?.len(),
            }))
        })())
    }

    /// Eventos recentes do app (guardados os últimos 2000), a partir de um seq.
    #[tool(description = "Eventos recentes do seu workspace (os últimos 2000 guardados), a partir de um seq; filtro opcional por tipo")]
    async fn app_events(&self, Parameters(a): Parameters<EventsArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "app_events", &parts);
        let me = match caller_row(&self.state, &parts) {
            Ok(r) => r,
            Err(e) => return tool_err(e),
        };
        let since = a.since_seq.unwrap_or(0);
        let events = self.state.events_since(since, a.kind.as_deref(), usize::MAX);
        let mut events = scoped(&self.state, &me.workspace_id, events);
        events.truncate(a.limit.unwrap_or(200).clamp(1, 500));
        ok_json(&feed(&self.state, since, events))
    }

    /// Espera eventos novos (até timeoutMs). Devolve assim que houver algum, ou uma lista vazia no fim do prazo.
    #[tool(description = "Espera eventos novos do seu workspace a partir de um seq (long-poll): volta assim que houver, ou vazio no fim do prazo")]
    async fn app_watch(&self, Parameters(a): Parameters<WatchArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "app_watch", &parts);
        let me = match caller_row(&self.state, &parts) {
            Ok(r) => r,
            Err(e) => return tool_err(e),
        };
        let timeout = Duration::from_millis(a.timeout_ms.unwrap_or(15_000).min(30_000));
        let started = Instant::now();
        // the cursor moves past events of other workspaces, so a long wait never rescans them
        let mut since = a.since_seq;
        loop {
            let raw = self.state.events_since(since, None, 200);
            if let Some(last) = raw.last() {
                since = last.seq;
            }
            let events = scoped(&self.state, &me.workspace_id, raw);
            if !events.is_empty() || started.elapsed() >= timeout {
                return ok_json(&feed(&self.state, a.since_seq, events));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Últimas linhas do log do app, sem segredos.
    #[tool(description = "Últimas linhas do log do Regente, com segredos ocultados")]
    async fn app_logs(&self, Parameters(a): Parameters<LogsArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "app_logs", &parts);
        let dir = self.state.data.join("logs");
        let n = a.lines.unwrap_or(200).clamp(1, 500);
        let r = blocking(move || -> AppResult<Value> {
            let Some(path) = newest_log(&dir) else { return fail("no_log", "nenhum log encontrado") };
            let lines = tail_lines(&path, n)?;
            Ok(json!({"file": path.file_name().map(|f| f.to_string_lossy().to_string()), "lines": redact(&lines.join("\n"))}))
        })
        .await;
        match r {
            Ok(v) => ok_json(&v),
            Err(e) => tool_err(e),
        }
    }

    /// Captura a janela do Regente em PNG e devolve o caminho. Erro se a janela não for encontrada.
    #[tool(description = "Captura a janela do Regente em PNG (só ela, nunca outros apps) e devolve o caminho; erro se a janela não existir")]
    async fn app_screenshot(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "app_screenshot", &parts);
        if let Err(e) = caller_row(&self.state, &parts).and_then(|me| require_top(&me)) {
            return tool_err(e);
        }
        let dir = self.state.data.join("screens");
        match blocking(move || capture(&dir)).await {
            Ok(path) => ok_json(&json!({"path": path.to_string_lossy()})),
            Err(e) => tool_err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn state(dir: &Path) -> Arc<AppState> {
        AppState::new(dir.into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap()
    }

    fn parts() -> http::request::Parts {
        http::Request::new(()).into_parts().0
    }

    /// Request parts of the pane `id`, as the auth middleware leaves them.
    fn as_pane(id: &str) -> http::request::Parts {
        let mut p = parts();
        p.extensions.insert(crate::mcp::CallerPane(id.into()));
        p
    }

    fn pane_in(id: &str, ws: &str) -> crate::db::PaneRow {
        crate::db::PaneRow { id: id.into(), workspace_id: ws.into(), kind: "agent".into(), name: id.into(), permission: "pend".into(), cwd: "C:/".into(), status: "ocioso".into(), ..Default::default() }
    }

    fn body(r: &CallToolResult) -> Value {
        serde_json::from_str(r.content[0].as_text().unwrap().text.as_str()).unwrap()
    }

    #[tokio::test]
    async fn app_watch_returns_new_event() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        st.db.insert_pane(&pane_in("p", "w")).unwrap();
        let since = st.events_since(0, None, usize::MAX).last().map_or(0, |e| e.seq);
        let m = RegenteMcp::new(st.clone());
        let t0 = Instant::now();
        let call = tokio::spawn(async move { m.app_watch(Parameters(WatchArgs { since_seq: since, timeout_ms: None }), Extension(as_pane("p"))).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        st.emit("pane-status", json!({"id": "p"}));
        let r = call.await.unwrap();
        assert!(t0.elapsed() < Duration::from_secs(2));
        assert_eq!(body(&r)["events"].as_array().unwrap().len(), 1);
    }


    #[tokio::test]
    async fn events_report_latest_seq_and_dropped() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        st.db.insert_pane(&pane_in("p", "w")).unwrap();
        let shared = st.clone();
        for _ in 0..2100 {
            st.emit("x", json!({"paneId": "p"}));
        }
        let m = RegenteMcp::new(st);
        let old = m.app_events(Parameters(EventsArgs { since_seq: Some(0), kind: None, limit: None }), Extension(as_pane("p"))).await;
        assert_eq!(body(&old)["latestSeq"], shared.latest_seq());
        assert_eq!(body(&old)["dropped"], true);
        let inside = shared.latest_seq() - 1900;
        let fresh = m.app_events(Parameters(EventsArgs { since_seq: Some(inside), kind: None, limit: None }), Extension(as_pane("p"))).await;
        assert_eq!(body(&fresh)["dropped"], false);
    }

    #[tokio::test]
    async fn events_of_another_workspace_are_invisible() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        st.db.insert_pane(&pane_in("pa", "A")).unwrap();
        st.db.insert_pane(&pane_in("pb", "B")).unwrap();
        let since = st.latest_seq();
        st.emit("pane-status", json!({"id": "pb"}));
        st.emit("pane-status", json!({"id": "pa"}));
        st.emit("handoff", json!({"missionId": "missao-sem-dono", "handoff": {}}));
        let m = RegenteMcp::new(st.clone());
        // the call's own mcp-call event is pa's too, so the feed is narrowed to pane-status
        let r = m.app_events(Parameters(EventsArgs { since_seq: Some(since), kind: Some("pane-status".into()), limit: None }), Extension(as_pane("pa"))).await;
        let events = body(&r)["events"].as_array().unwrap().clone();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["data"]["id"], "pa");
    }

    #[tokio::test]
    async fn app_status_and_screenshot_need_the_top_role() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        st.db.insert_pane(&crate::db::PaneRow { parent_id: Some("p".into()), role: Some("ajudante".into()), ..pane_in("h", "w") }).unwrap();
        let m = RegenteMcp::new(st);
        assert_eq!(m.app_status(Extension(as_pane("h"))).await.is_error, Some(true));
        assert_eq!(m.app_screenshot(Extension(as_pane("h"))).await.is_error, Some(true));
    }

    #[tokio::test]
    async fn app_logs_redacts() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        std::fs::create_dir_all(d.path().join("logs")).unwrap();
        std::fs::write(d.path().join("logs").join("regente.log.2026-10-15"), "ok\nconfig token=abc\n").unwrap();
        let m = RegenteMcp::new(st);
        let r = m.app_logs(Parameters(LogsArgs { lines: None }), Extension(parts())).await;
        let text = r.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("config"), "{text}");
        assert!(!text.contains("abc"), "{text}");
    }
}

#[cfg(test)]
mod screen_tests {
    use super::*;

    /// Redline's `limits` events reach the windows and the tray, never the agent's feed.
    #[test]
    fn app_events_never_return_limits() {
        let d = tempfile::tempdir().unwrap();
        let st = AppState::new(d.path().into(), PathBuf::from("C:/x/regente.exe"), std::sync::Arc::new(|_: &str, _: Value| {})).unwrap();
        st.emit("limits", json!({"windows": [], "stale": [], "topo": []}));
        st.emit("pane-status", json!({"id": "p"}));
        let kinds: Vec<String> = st.events_since(0, None, 100).into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec!["pane-status".to_string()]);
    }

    #[test]
    fn prune_keeps_the_newest_ten() {
        let d = tempfile::tempdir().unwrap();
        for i in 1..=12 {
            std::fs::write(d.path().join(format!("{i}.png")), b"x").unwrap();
        }
        prune_screens(d.path(), SCREENS_KEEP);
        let mut left: Vec<String> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left.len(), 10);
        assert!(!left.contains(&"1.png".to_string()) && !left.contains(&"2.png".to_string()));
    }
}
