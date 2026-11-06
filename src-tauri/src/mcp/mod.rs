//! Servidor MCP embutido (rmcp, HTTP local) e rota `/hook` que recebe os hooks das IAs.
//! Cada painel tem o seu token (`Runtime::tokens`, token -> pane). O middleware resolve o painel pelo token;
//! o `X-Regente-Pane` é só conferência, nunca identidade.

pub mod inject;
pub mod tools_agentic;
pub mod tools_artifacts;
pub mod tools_catalog;
#[cfg(feature = "codeindex")]
pub mod tools_code;
pub mod tools_cron;
pub mod tools_diag;
pub mod tools_handoff;
pub mod tools_missions;
pub mod tools_panes;
pub mod tools_plan;
pub mod tools_media;
pub mod tools_memory;
pub mod tools_recipes;
pub mod tools_shot;
pub mod tools_squads;
pub mod tools_tasks;
pub mod user_servers;
// ▼ trilha A
// ▲ trilha A
// ▼ trilha B
// ▲ trilha B
// ▼ trilha C
// ▲ trilha C

use crate::db::{Mission, PaneRow};
use crate::error::{fail, AppError, AppResult};
use crate::providers::Permission;
use crate::runtime::AppState;
use crate::vault::{random, to_hex};
use axum::extract::{DefaultBodyLimit, Extension, Request};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use rmcp::transport::streamable_http_server::{session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool_handler, ServerHandler};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct McpInfo {
    pub port: u16,
}

impl McpInfo {
    pub fn mcp_url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
    pub fn hook_url(&self) -> String {
        format!("http://127.0.0.1:{}/hook", self.port)
    }
}

/// Pane that owns the bearer token of the request. Set by the auth middleware, read by tools and `/hook`.
#[derive(Clone, Debug, PartialEq)]
pub struct CallerPane(pub String);

#[derive(Clone)]
pub struct RegenteMcp {
    pub(crate) state: Arc<AppState>,
}

impl RegenteMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Composition of every router (each tools_*.rs file declares its own `#[tool_router(router = ..)]`).
    pub fn all() -> ToolRouter<Self> {
        Self::diag_router()
            + Self::panes_router()
            + Self::handoff_router()
            + Self::missions_router()
            + Self::tasks_router()
            + Self::memory_router()
            + Self::catalog_router()
            + Self::squads_router()
            + Self::recipes_router()
            + Self::agentic_router()
            + Self::cron_router()
            + Self::media_router()
            + Self::shot_router()
            // ▼ trilha A
            + Self::plan_router()
            // ▲ trilha A
            // ▼ trilha B
            // ▲ trilha B
            // ▼ trilha C
            + Self::artifacts_router()
            + Self::code_part()
            // ▲ trilha C
    }

    /// Índice de código (RM-24): some quando a feature `codeindex` está desligada.
    #[cfg(feature = "codeindex")]
    fn code_part() -> ToolRouter<Self> {
        Self::code_router()
    }

    #[cfg(not(feature = "codeindex"))]
    fn code_part() -> ToolRouter<Self> {
        ToolRouter::default()
    }
}

#[tool_handler(router = Self::all())]
impl ServerHandler for RegenteMcp {
    /// Recusa a ferramenta oculta para o papel do chamador (RM-18); o resto segue para o roteador.
    async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, ErrorData> {
        if let Some(parts) = context.extensions.get::<http::request::Parts>() {
            if let Err(e) = checar_papel(&self.state, parts, &request.name) {
                return Ok(CallToolResponse::from(tool_err(e)));
            }
        }
        Self::all().call(ToolCallContext::new(self, request, context)).await
    }
}

/// Pane of the calling request, from the token (never from the header).
pub fn caller(parts: &http::request::Parts) -> Option<String> {
    parts.extensions.get::<CallerPane>().map(|c| c.0.clone())
}

/// Runs a step that blocks (git, process waits) off the async runtime.
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(AppError::new("internal", e.to_string())))
}

/// Roles only the app creates (the pilot and the chiefs of an agentic mission, the maestro of a squad):
/// no MCP tool may open a pane with one of them.
pub(crate) fn protected_role(role: &str) -> bool {
    role == "piloto" || role == "maestro" || role.starts_with("chefe:")
}

fn perm_rank(p: &Permission) -> u8 {
    match p {
        Permission::Plan => 0,
        Permission::Pend => 1,
        Permission::Auto => 2,
        Permission::Danger => 3,
    }
}

/// The permission a pane was opened with. Old rows, or anything unknown, count as `pend`.
fn row_permission(row: &PaneRow) -> Permission {
    serde_json::from_value(json!(row.permission)).unwrap_or_default()
}

/// True when `target` ranks above `caller` (a pane may not write into one that can do more than it).
pub(crate) fn outranks(target: &PaneRow, caller: &PaneRow) -> bool {
    perm_rank(&row_permission(target)) > perm_rank(&row_permission(caller))
}

/// Permission of a pane opened over MCP: the requested one, but never above the caller's own; the caller's when none is asked.
/// `danger` is only for the UI.
pub(crate) fn cap_perm(caller: &PaneRow, requested: Option<Permission>) -> AppResult<Permission> {
    let mine = row_permission(caller);
    match requested {
        Some(Permission::Danger) => fail("forbidden", "permissão danger só pela interface"),
        Some(p) if perm_rank(&p) > perm_rank(&mine) => Ok(mine),
        Some(p) => Ok(p),
        // a danger caller's pane is not inherited: MCP never produces danger
        None => Ok(if mine == Permission::Danger { Permission::Auto } else { mine }),
    }
}

/// `target` is the caller itself or one of its descendants (walking `parent_id`).
pub(crate) fn is_self_or_descendant(s: &AppState, caller: &str, target: &str) -> bool {
    let mut cur = Some(target.to_string());
    for _ in 0..64 {
        let Some(id) = cur else { return false };
        if id == caller {
            return true;
        }
        cur = s.db.get_pane(&id).ok().and_then(|r| r.parent_id);
    }
    false
}

/// Mission-wide actions (integrate, status, brief, catalog and agent writes, squad spawn): the pilot, a chief, the maestro, or a pane with no parent.
pub(crate) fn require_top(caller: &PaneRow) -> AppResult<()> {
    if caller.parent_id.is_none() || caller.role.as_deref().is_some_and(protected_role) {
        return Ok(());
    }
    fail("forbidden", "só o piloto, um chefe, o maestro ou um painel sem pai pode fazer isso")
}

/// Open panes a workspace may hold when MCP opens them.
pub(crate) const MCP_PANE_MAX: usize = 24;

/// Refuses a spawn of `n` panes when the workspace would pass `MCP_PANE_MAX` open panes.
pub(crate) fn check_spawn_budget(s: &AppState, caller: &PaneRow, n: usize) -> AppResult<()> {
    if s.list_panes(&caller.workspace_id)?.len() + n > MCP_PANE_MAX {
        return fail("too_many_panes", format!("o workspace já tem {MCP_PANE_MAX} painéis abertos; feche algum antes"));
    }
    Ok(())
}

/// Refuses to spawn or open a gate for a caller whose mission is archived.
pub(crate) fn not_archived(s: &AppState, caller: &PaneRow) -> AppResult<()> {
    let archived = caller.mission_id.as_deref().and_then(|m| s.db.get_mission(m).ok()).is_some_and(|m| m.archived_at.is_some());
    if archived {
        return fail("archived", "a missão está arquivada");
    }
    Ok(())
}

/// Tools that change something: a pend or plan pane's Claude is not pre-allowed these, so it asks first.
pub(crate) const MUTATING_TOOLS: &[&str] = &[
    "pane_write",
    "pane_close",
    "pane_hibernate",
    "pane_spawn",
    "cron_create",
    "cron_delete",
    "cron_run_now",
    "catalog_install",
    "catalog_uninstall",
    "agent_save",
    "squad_save",
    "recipe_save",
    "mission_integrate",
    "mission_set_status",
    "squad_spawn",
    "mission_create",
    "pane_rename",
    "image_generate",
    "video_generate",
    "audio_generate",
    "shot_capture",
    // ▼ trilha A
    "plan_reply",
    // ▲ trilha A
    // ▼ trilha B
    "memory_pin",
    // ▲ trilha B
    // ▼ trilha C
    // ▲ trilha C
];


/// Ferramentas que um papel curto vê (RM-18). Nomes que não existem (ainda) são ignorados.
const NUCLEO: &[&str] = &[
    "handoff_submit",
    "handoff_get",
    "memory_read",
    "memory_search",
    "memory_write",
    "task_update",
    "task_list",
    "pane_status",
    "artifact_read",
    "code_outline",
    "code_symbols",
    "code_search",
];

/// Papel curto: ajudante, agente, executor, reviewer, scout e tester. Piloto, maestro, planner, chefes e painéis do usuário veem tudo.
pub(crate) fn papel_curto(role: Option<&str>) -> bool {
    matches!(role, Some("ajudante" | "agente" | "executor" | "reviewer" | "scout" | "tester"))
}

/// Ferramentas do Regente que o papel vê; None = todas. Filtra na abertura (R-10, `--disallowedTools` etc.) e recusa na chamada.
pub fn tools_for_role(role: Option<&str>) -> Option<Vec<&'static str>> {
    papel_curto(role).then(|| NUCLEO.to_vec())
}

/// Ferramentas do Regente ocultas para o papel (vazio para papel longo).
pub fn ocultas_para(role: Option<&str>) -> Vec<String> {
    let Some(vistas) = tools_for_role(role) else { return vec![] };
    RegenteMcp::all().list_all().iter().map(|t| t.name.to_string()).filter(|n| !vistas.iter().any(|v| v == n)).collect()
}

/// `_meta` anthropic/alwaysLoad: a ferramenta vem carregada no início, sem busca de ferramentas (só `handoff_submit`).
pub(crate) fn sempre_carregada() -> rmcp::model::MetaObject {
    let mut m = serde_json::Map::new();
    m.insert("anthropic/alwaysLoad".into(), json!(true));
    m.into()
}
/// Tools a pane's Claude may run without asking: the whole server for auto and danger; for pend and plan, every tool but `MUTATING_TOOLS`.
pub fn pre_allowed(p: Permission) -> Vec<String> {
    if matches!(p, Permission::Auto | Permission::Danger) {
        return vec!["mcp__regente__*".into()];
    }
    RegenteMcp::all().list_all().iter().map(|t| t.name.to_string()).filter(|n| !MUTATING_TOOLS.contains(&n.as_str())).map(|n| format!("mcp__regente__{n}")).collect()
}

/// Defesa em profundidade (RM-18): ferramenta oculta para o papel do chamador é recusada, mesmo que a CLI a chame.
pub(crate) fn checar_papel(s: &AppState, parts: &http::request::Parts, tool: &str) -> AppResult<()> {
    // sem linha do painel, não há papel a filtrar: cada ferramenta recusa por conta própria
    // sem cabeçalho de painel não há papel a filtrar; com cabeçalho e linha ilegível, recusa (fecha por padrão)
    let me = match caller_row(s, parts) {
        Ok(m) => m,
        Err(e) if e.code() == "no_caller" => return Ok(()),
        Err(e) => return Err(e),
    };
    if !crate::perfil::efetivo_de(s, &me.workspace_id, me.profile.as_deref()).0.mcp.filtrar_por_papel {
        return Ok(());
    }
    match tools_for_role(me.role.as_deref()) {
        Some(vistas) if !vistas.iter().any(|v| *v == tool) => Err(AppError::new("tool_unavailable", format!("ferramenta indisponível para o papel {}", me.role.as_deref().unwrap_or("")))),
        _ => Ok(()),
    }
}

/// Whether the caller may act on `target`: itself, a descendant, or (for a protected role) a non-protected pane of its own mission.
pub(crate) fn may_control(s: &AppState, caller: &PaneRow, target: &PaneRow) -> bool {
    if target.id == caller.id || is_self_or_descendant(s, &caller.id, &target.id) {
        return true;
    }
    let caller_protected = caller.role.as_deref().is_some_and(protected_role);
    let target_protected = target.role.as_deref().is_some_and(protected_role);
    caller_protected && caller.mission_id.is_some() && caller.mission_id == target.mission_id && (!target_protected || target.parent_id.as_deref() == Some(caller.id.as_str()))
}


/// Every tool call tells the UI which tool ran and for which pane (the caller from its token).
pub(crate) fn note(s: &AppState, tool: &str, parts: &http::request::Parts) {
    s.emit("mcp-call", json!({"tool": tool, "pane": caller(parts)}));
}

/// Tool result from a domain result: errors are `isError` results, never protocol errors.
pub(crate) fn finish(r: AppResult<Value>) -> CallToolResult {
    match r {
        Ok(v) => ok_json(&v),
        Err(e) => tool_err(e),
    }
}

/// The calling pane's row: its workspace, mission and cwd scope every tool.
pub(crate) fn caller_row(s: &AppState, parts: &http::request::Parts) -> AppResult<PaneRow> {
    let id = caller(parts).ok_or_else(|| AppError::new("no_caller", "chamada sem X-Regente-Pane"))?;
    s.db.get_pane(&id)
}

/// A pane given as an argument: it must be in the caller's workspace.
pub(crate) fn pane_in_ws(s: &AppState, ws: &str, pane: &str) -> AppResult<PaneRow> {
    match s.db.get_pane(pane) {
        Ok(r) if r.workspace_id == ws => Ok(r),
        _ => fail("pane_outside_workspace", "painel fora do workspace"),
    }
}

/// A mission given as an argument: it must be in the caller's workspace.
pub(crate) fn mission_in_ws(s: &AppState, ws: &str, id: &str) -> AppResult<Mission> {
    match s.db.get_mission(id) {
        Ok(m) if m.workspace_id == ws => Ok(m),
        _ => fail("mission_outside_workspace", "missão fora do workspace"),
    }
}

/// The caller's own mission id (tools that act on "the mission" need one).
pub(crate) fn own_mission(row: &PaneRow) -> AppResult<String> {
    row.mission_id.clone().ok_or_else(|| AppError::new("no_mission", "este painel não está em uma missão"))
}

/// A new per-pane token: 32 random bytes in hex.
pub fn new_token() -> AppResult<String> {
    Ok(to_hex(&random::<32>()?))
}

/// Caps on what the MCP may write: each text field up to 4000 chars, each document up to 64 KB.
pub(crate) const MCP_TEXT_MAX: usize = 4000;
pub(crate) const MCP_DOC_MAX: usize = 64 * 1024;

pub(crate) fn check_texts(texts: &[&str]) -> AppResult<()> {
    if texts.iter().any(|t| t.chars().count() > MCP_TEXT_MAX) {
        return crate::error::fail("too_big", "texto acima de 4000 caracteres");
    }
    Ok(())
}

pub(crate) fn check_doc<T: Serialize>(v: &T) -> AppResult<()> {
    if serde_json::to_string(v).map_or(true, |s| s.len() > MCP_DOC_MAX) {
        return crate::error::fail("too_big", "documento acima de 64 KB");
    }
    Ok(())
}

pub fn ok_text(s: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(s)])
}

pub fn ok_json<T: Serialize>(v: &T) -> CallToolResult {
    ok_text(serde_json::to_string_pretty(v).unwrap_or_default())
}

/// Domain errors are results with `isError`, not protocol errors.
pub fn tool_err(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

#[derive(Deserialize)]
struct HookBody {
    event: String,
    pane: String,
    #[serde(default)]
    payload: Value,
}

fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Pane for the request's bearer token, or None. A present `X-Regente-Pane` must name that same pane.
pub fn authorize(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let token = headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")?;
    let pane = {
        let rt = state.rt();
        rt.tokens.iter().find(|(t, _)| same_bytes(t.as_bytes(), token.as_bytes())).map(|(_, p)| p.clone())?
    };
    if let Some(h) = headers.get("x-regente-pane") {
        if h.to_str().ok() != Some(pane.as_str()) {
            return None;
        }
    }
    Some(pane)
}

/// Router: `/mcp` (rmcp) + `POST /hook`. Host must be the loopback address of `port`; every request needs a pane token.
pub fn router(state: Arc<AppState>, port: u16) -> axum::Router {
    let hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let service = StreamableHttpService::new(
        {
            let st = state.clone();
            move || Ok(RegenteMcp::new(st.clone()))
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default(),
    );
    let hook_state = state.clone();
    let auth_state = state;
    axum::Router::new()
        .nest_service("/mcp", service)
        .route(
            "/hook",
            post(move |Extension(CallerPane(caller)): Extension<CallerPane>, Json(b): Json<HookBody>| {
                let st = hook_state.clone();
                async move {
                    if b.pane != caller {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    st.on_hook(&b.pane, &b.event, &b.payload);
                    Json(json!({})).into_response()
                }
            })
            .layer(DefaultBodyLimit::max(256 * 1024)),
        )
        .layer(middleware::from_fn(move |mut req: Request, next: Next| {
            let auth_state = auth_state.clone();
            let hosts = hosts.clone();
            async move {
                let host_ok = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).is_some_and(|h| hosts.iter().any(|x| x == h));
                if !host_ok {
                    return StatusCode::FORBIDDEN.into_response();
                }
                match authorize(&auth_state, req.headers()) {
                    Some(pane) => {
                        req.extensions_mut().insert(CallerPane(pane));
                        next.run(req).await
                    }
                    None => {
                        tracing::debug!("mcp: 401 em {} (token ausente, desconhecido ou de outro painel)", req.uri().path());
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                }
            }
        }))
}

/// Binds 127.0.0.1 on a free port (synchronously, so the port is known) and serves in the Tauri runtime.
pub fn start(state: Arc<AppState>) -> AppResult<McpInfo> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    tauri::async_runtime::spawn(serve(listener, router(state, port)));
    Ok(McpInfo { port })
}

pub async fn serve(listener: std::net::TcpListener, app: axum::Router) {
    let res = match listener.set_nonblocking(true).and_then(|()| tokio::net::TcpListener::from_std(listener)) {
        Ok(l) => axum::serve(l, app).await,
        Err(e) => Err(e),
    };
    if let Err(e) = res {
        tracing::error!("servidor MCP parou: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn state(dir: &std::path::Path) -> Arc<AppState> {
        AppState::new(dir.into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap()
    }

    /// A token for `pane`, as plan_for registers it.
    fn register(st: &AppState, pane: &str, token: &str) {
        st.rt().tokens.insert(token.into(), pane.into());
    }

    /// Serves on a free port; returns the base url.
    async fn spawn_server(st: Arc<AppState>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve(listener, router(st, port)));
        format!("http://127.0.0.1:{port}")
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    /// The `data:` line of an SSE response that carries a JSON-RPC answer.
    fn sse_result(body: &str) -> Value {
        let line = body.lines().filter_map(|l| l.strip_prefix("data:")).find(|d| d.contains("\"id\"")).expect("sse data line");
        serde_json::from_str(line.trim()).unwrap()
    }

    #[test]
    fn authorize_resolves_the_pane_from_the_token() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        register(&st, "A", "tokA");
        register(&st, "B", "tokB");
        assert_eq!(authorize(&st, &headers(&[("authorization", "Bearer tokA")])).as_deref(), Some("A"));
        assert_eq!(authorize(&st, &headers(&[("authorization", "Bearer tokA"), ("x-regente-pane", "A")])).as_deref(), Some("A"));
        // pane A's token with pane B's header: refused
        assert_eq!(authorize(&st, &headers(&[("authorization", "Bearer tokA"), ("x-regente-pane", "B")])), None);
        // unknown token, missing or non-bearer auth: refused
        assert_eq!(authorize(&st, &headers(&[("authorization", "Bearer nope")])), None);
        assert_eq!(authorize(&st, &headers(&[])), None);
        assert_eq!(authorize(&st, &headers(&[("authorization", "tokA")])), None);
    }

    #[test]
    fn caller_reads_the_extension_not_the_header() {
        let req = http::Request::builder().header("x-regente-pane", "B").body(()).unwrap();
        let (mut parts, _) = req.into_parts();
        assert_eq!(caller(&parts), None);
        parts.extensions.insert(CallerPane("A".into()));
        assert_eq!(caller(&parts).as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn rejects_without_or_with_unknown_token() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        register(&st, "A", "tokA");
        let base = spawn_server(st).await;
        let c = reqwest::Client::new();
        assert_eq!(c.post(format!("{base}/mcp")).body("{}").send().await.unwrap().status(), 401);
        assert_eq!(c.post(format!("{base}/mcp")).bearer_auth("nope").body("{}").send().await.unwrap().status(), 401);
        assert_eq!(c.post(format!("{base}/hook")).bearer_auth("nope").body("{}").send().await.unwrap().status(), 401);
    }

    #[tokio::test]
    async fn token_of_a_with_header_of_b_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        register(&st, "A", "tokA");
        let base = spawn_server(st).await;
        let body = json!({"event": "Stop", "pane": "B", "payload": {}});
        let r = reqwest::Client::new().post(format!("{base}/hook")).bearer_auth("tokA").header("X-Regente-Pane", "B").json(&body).send().await.unwrap();
        assert_eq!(r.status(), 401);
    }

    #[tokio::test]
    async fn hook_from_another_pane_is_refused_and_own_hook_applies() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        register(&st, "A", "tokA");
        st.rt().statuses.insert("A".into(), crate::status::PaneStatus::Working);
        st.rt().statuses.insert("B".into(), crate::status::PaneStatus::Working);
        let base = spawn_server(st.clone()).await;
        let c = reqwest::Client::new();
        // body names pane B while the token is A's: refused
        let r = c.post(format!("{base}/hook")).bearer_auth("tokA").json(&json!({"event": "Stop", "pane": "B", "payload": {}})).send().await.unwrap();
        assert_eq!(r.status(), 401);
        assert!(!st.rt().hook_state.contains_key("B"));
        let r = c.post(format!("{base}/hook")).bearer_auth("tokA").json(&json!({"event": "Stop", "pane": "A", "payload": {}})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(st.rt().hook_state["A"].0, crate::status::PaneStatus::Idle);
    }

    #[tokio::test]
    async fn foreign_host_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        register(&st, "A", "tokA");
        let base = spawn_server(st).await;
        let r = reqwest::Client::new().post(format!("{base}/mcp")).bearer_auth("tokA").header("Host", "evil.example:1").body("{}").send().await.unwrap();
        assert_eq!(r.status(), 403);
    }

    #[tokio::test]
    async fn initialize_list_call() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        // o painel do token precisa existir: sem linha, a checagem de papel recusa (fecha por padrão)
        st.db.insert_pane(&PaneRow { id: "A".into(), workspace_id: "w".into(), kind: "agent".into(), name: "a".into(), permission: "pend".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), ..Default::default() }).unwrap();
        register(&st, "A", "tokA");
        let base = spawn_server(st).await;
        let c = reqwest::Client::new();
        let mcp = format!("{base}/mcp");
        let accept = "application/json, text/event-stream";
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}});
        let r = c.post(&mcp).bearer_auth("tokA").header("X-Regente-Pane", "A").header("Accept", accept).json(&init).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let sid = r.headers()["mcp-session-id"].to_str().unwrap().to_string();
        let body = r.text().await.unwrap();
        assert!(sse_result(&body)["result"]["capabilities"].is_object());

        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        c.post(&mcp).bearer_auth("tokA").header("Accept", accept).header("mcp-session-id", &sid).json(&note).send().await.unwrap();

        let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
        let r = c.post(&mcp).bearer_auth("tokA").header("Accept", accept).header("mcp-session-id", &sid).json(&list).send().await.unwrap();
        let v = sse_result(&r.text().await.unwrap());
        assert!(v["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "app_health"), "{v}");

        let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "app_health", "arguments": {}}});
        let r = c.post(&mcp).bearer_auth("tokA").header("Accept", accept).header("mcp-session-id", &sid).json(&call).send().await.unwrap();
        let v = sse_result(&r.text().await.unwrap());
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"ok\": true"), "{text}");
    }

    const ACCEPT: &str = "application/json, text/event-stream";

    /// Panes of the tool tests: `p` (piloto, workspace w), its helper `c`, and `x` in another workspace.
    fn seed(st: &AppState, cwd: &std::path::Path) {
        let pane = |id: &str, ws: &str, parent: Option<&str>, role: &str| PaneRow {
            id: id.into(),
            workspace_id: ws.into(),
            mission_id: Some("m".into()),
            kind: "agent".into(),
            name: id.into(),
            permission: "pend".into(),
            cwd: cwd.to_string_lossy().into(),
            status: "ocioso".into(),
            parent_id: parent.map(Into::into),
            role: Some(role.into()),
            ..Default::default()
        };
        st.db.insert_pane(&pane("p", "w", None, "piloto")).unwrap();
        st.db.insert_pane(&pane("c", "w", Some("p"), "ajudante")).unwrap();
        st.db.insert_pane(&pane("x", "outro", None, "piloto")).unwrap();
        register(st, "p", "tokP");
        register(st, "c", "tokC");
    }

    /// An MCP session as the pane that owns `token`.
    struct Mcp {
        c: reqwest::Client,
        url: String,
        token: String,
        sid: String,
    }

    impl Mcp {
        async fn open(base: &str, token: &str) -> Mcp {
            let c = reqwest::Client::new();
            let url = format!("{base}/mcp");
            let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}});
            let r = c.post(&url).bearer_auth(token).header("Accept", ACCEPT).json(&init).send().await.unwrap();
            let sid = r.headers()["mcp-session-id"].to_str().unwrap().to_string();
            r.text().await.unwrap();
            let m = Mcp { c, url, token: token.into(), sid };
            let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
            m.c.post(&m.url).bearer_auth(&m.token).header("Accept", ACCEPT).header("mcp-session-id", &m.sid).json(&note).send().await.unwrap();
            m
        }

        /// The `result` of a JSON-RPC request.
        async fn request(&self, method: &str, params: Value) -> Value {
            let body = json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params});
            let r = self.c.post(&self.url).bearer_auth(&self.token).header("Accept", ACCEPT).header("mcp-session-id", &self.sid).json(&body).send().await.unwrap();
            sse_result(&r.text().await.unwrap())["result"].clone()
        }

        async fn call(&self, tool: &str, args: Value) -> Value {
            self.request("tools/call", json!({"name": tool, "arguments": args})).await
        }
    }

    /// The text of a tool result, parsed as JSON (errors are plain text, so this is for successes).
    fn body(r: &Value) -> Value {
        serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn tools_list_count() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.request("tools/list", json!({})).await;
        let names: Vec<&str> = r["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let esperado = if cfg!(feature = "codeindex") { 66 } else { 63 };
        assert_eq!(names.len(), esperado, "{names:?}");
    }

    /// Makes `p`'s mission agentic with `style`; claude runs as cmd.exe so no real CLI is needed.
    fn agentic(st: &AppState, style: &str) {
        st.db
            .insert_mission(&crate::db::Mission {
                id: "m".into(),
                workspace_id: "w".into(),
                name: "Ag".into(),
                mode: "agentico".into(),
                status: "ativa".into(),
                status_reason: None,
                worktree_path: None,
                branch: None,
                base_branch: None,
                pinned: false,
                created_at: 0,
                archived_at: None,
                squad_id: None,
                recipe_id: None,
                brief: None,
                style: Some(style.into()),
                profile: None, plan_json: None, verify_json: None, adopted: false,
            })
            .unwrap();
        let mut cfg = crate::util::lk(&st.providers);
        let claude = cfg.clis.iter_mut().find(|c| c.id == "claude").unwrap();
        claude.command = "cmd.exe".into();
        claude.prompt_args.clear();
    }

    #[tokio::test]
    async fn delegate_requires_type_without_decisor_and_only_the_pilot_delegates() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        agentic(&st, "chain");
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("delegate", json!({"request": "criar a tela"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("testes"), "the error lists the types: {r}");
        // a helper cannot delegate
        let helper = Mcp::open(&base, "tokC").await;
        let r = helper.call("delegate", json!({"request": "x", "taskType": "testes"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(st.panes.ids().is_empty());
    }

    #[tokio::test]
    async fn delegate_spawns_helper_and_receipt() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        agentic(&st, "chain");
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("delegate", json!({"request": "criar testes e2e do checkout", "taskType": "testes"})).await;
        assert_ne!(r["isError"], true, "{r}");
        let receipt = body(&r);
        assert!(receipt["receipt"].as_str().unwrap().contains("testes"), "{receipt}");
        let child = receipt["pane"]["id"].as_str().unwrap().to_string();
        let row = st.db.get_pane(&child).unwrap();
        assert_eq!(row.role.as_deref(), Some("ajudante"));
        assert_eq!(row.parent_id.as_deref(), Some("p"));
        assert_eq!(row.task_type.as_deref(), Some("testes"));
        assert_eq!(row.permission, "pend", "the helper of a pend pilot is pend");
        assert_eq!(row.agent.as_deref(), Some("sq-qa"));
        let decisions = st.db.decisions(0, 10).unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].pane_id.as_deref(), Some(child.as_str()));
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }
    }

    #[tokio::test]
    async fn delegate_resumes_within_30min() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        agentic(&st, "chain");
        let closed_ago = |min: i64| crate::db::HelperSession {
            task_type: "testes".into(),
            mission_id: "m".into(),
            pane_id: "old".into(),
            session_id: Some("S".into()),
            cli: Some("claude".into()),
            closed_at: crate::util::now_ms() - min * 60_000,
        };
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        st.db.put_helper_session(&closed_ago(5)).unwrap();
        let r = mcp.call("delegate", json!({"request": "mais testes", "taskType": "testes"})).await;
        let receipt = body(&r);
        assert_eq!(receipt["resumed"], true, "{receipt}");
        assert!(receipt["receipt"].as_str().unwrap().contains("retomou"), "{receipt}");
        let child = receipt["pane"]["id"].as_str().unwrap().to_string();
        assert_eq!(st.db.get_pane(&child).unwrap().session_id.as_deref(), Some("S"));
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }

        st.db.put_helper_session(&closed_ago(40)).unwrap();
        let receipt = body(&mcp.call("delegate", json!({"request": "outra vez", "taskType": "testes"})).await);
        assert_eq!(receipt["resumed"], false, "{receipt}");
        let child = receipt["pane"]["id"].as_str().unwrap().to_string();
        assert_ne!(st.db.get_pane(&child).unwrap().session_id.as_deref(), Some("S"));
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }
    }

    #[tokio::test]
    async fn chief_is_reused_and_torre_blocks_builder() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        agentic(&st, "chain");
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let first = body(&mcp.call("chief_invoke", json!({"type": "builder", "request": "primeiro"})).await);
        assert_eq!(first["reused"], false, "{first}");
        assert_eq!(st.db.get_pane(first["pane"]["id"].as_str().unwrap()).unwrap().permission, "pend", "chief is capped to the caller");
        let second = body(&mcp.call("chief_invoke", json!({"type": "builder", "request": "segundo"})).await);
        assert_eq!(second["reused"], true, "{second}");
        assert_eq!(first["pane"]["id"], second["pane"]["id"]);
        let chiefs = st.db.mission_panes("m").unwrap().into_iter().filter(|p| p.role.as_deref() == Some("chefe:builder")).count();
        assert_eq!(chiefs, 1);
        // the controller is a torre-style chief only
        assert_eq!(mcp.call("chief_invoke", json!({"type": "controller", "request": "x"})).await["isError"], true);
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }

        st.db.update_mission(&crate::db::Mission { style: Some("torre".into()), ..st.db.get_mission("m").unwrap() }).unwrap();
        let r = mcp.call("chief_invoke", json!({"type": "builder", "request": "x"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("controller"), "{r}");
    }

    #[tokio::test]
    async fn gate_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        // the request stays open until the decision; the decision comes from the UI's command path
        let decide = async {
            let t0 = std::time::Instant::now();
            let id = loop {
                if let Some(g) = st.db.open_gates(None).unwrap().first() {
                    break g.id.clone();
                }
                assert!(t0.elapsed() < std::time::Duration::from_secs(10), "gate never opened");
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            };
            crate::squads::answer_gate(&st, &id, "aprovar", Some("ok")).unwrap();
        };
        let (r, ()) = tokio::join!(mcp.call("gate_request", json!({"title": "Aprovar", "summary": "tudo pronto"})), decide);
        assert_ne!(r["isError"], true, "{r}");
        assert_eq!(body(&r), json!({"decision": "aprovar", "note": "ok"}));
    }

    #[tokio::test]
    async fn mcp_writes_only_create_and_stay_capped() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let refused = |r: &Value| r["isError"] == true;
        let squad = |id: &str, phases: usize| {
            let ph: Vec<Value> = (0..phases).map(|_| json!({"name": "f", "steps": [], "gate": null})).collect();
            json!({"id": id, "name": "S", "description": "d", "phases": ph})
        };
        // originals and names taken from any source are refused
        assert!(refused(&mcp.call("squad_save", json!({"squad": squad("jogo-web", 1)})).await));
        assert!(refused(&mcp.call("recipe_save", json!({"recipe": {"id": "livre-3-claude", "name": "x", "description": "", "mode": "livre"}})).await));
        assert!(refused(&mcp.call("agent_save", json!({"name": "sq-qa", "description": "d", "body": "b"})).await));
        // a new id is created once
        assert!(!refused(&mcp.call("squad_save", json!({"squad": squad("meu-time", 1)})).await));
        assert!(refused(&mcp.call("squad_save", json!({"squad": squad("meu-time", 1)})).await));
        // caps
        assert!(refused(&mcp.call("squad_save", json!({"squad": squad("grande", 13)})).await));
        assert!(refused(&mcp.call("squad_save", json!({"squad": {"id": "texto", "name": "x".repeat(4001), "description": ""}})).await));
        let slots: Vec<Value> = vec![json!({}); 9];
        assert!(refused(&mcp.call("recipe_save", json!({"recipe": {"id": "muitos", "name": "x", "description": "", "mode": "livre", "slots": slots}})).await));
        // the body may be longer than the 4000-char text cap, up to 20000; the description keeps the cap
        assert!(!refused(&mcp.call("agent_save", json!({"name": "corpo-longo", "description": "d", "body": "x".repeat(4001)})).await));
        assert!(refused(&mcp.call("agent_save", json!({"name": "corpo-enorme", "description": "d", "body": "x".repeat(20001)})).await));
        assert!(refused(&mcp.call("agent_save", json!({"name": "descricao-longa", "description": "x".repeat(4001), "body": "b"})).await));
        // an empty brief does not open a squad
        assert!(refused(&mcp.call("squad_spawn", json!({"squadId": "jogo-web", "brief": "   "})).await));
    }

    #[test]
    fn text_and_doc_caps() {
        assert!(check_texts(&["x".repeat(4000).as_str()]).is_ok());
        assert!(check_texts(&["x".repeat(4001).as_str()]).is_err());
        assert!(check_doc(&"x".repeat(MCP_DOC_MAX)).is_err(), "a JSON string adds its quotes");
        assert!(check_doc(&json!({"a": 1})).is_ok());
    }

    #[tokio::test]
    async fn agent_invoke_uses_the_squad_roster() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        // the caller's mission is an app-web squad mission: its roster sets sq-frontend's model
        st.db.insert_mission(&crate::db::Mission {
            id: "m".into(),
            workspace_id: "w".into(),
            name: "App".into(),
            mode: "squad".into(),
            status: "ativa".into(),
            status_reason: None,
            worktree_path: None,
            branch: None,
            base_branch: None,
            pinned: false,
            created_at: 0,
            archived_at: None,
            squad_id: Some("app-web".into()),
            recipe_id: None,
            brief: None,
            style: None,
            profile: None, plan_json: None, verify_json: None, adopted: false,
        })
        .unwrap();
        {
            let mut cfg = crate::util::lk(&st.providers);
            let claude = cfg.clis.iter_mut().find(|c| c.id == "claude").unwrap();
            claude.command = "cmd.exe".into();
            claude.prompt_args.clear();
        }
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("agent_invoke", json!({"name": "sq-frontend", "request": "tela de login"})).await;
        assert_ne!(r["isError"], true, "{r}");
        let receipt = body(&r);
        assert_eq!(receipt["harness"]["model"], "claude-opus-5-5", "{receipt}");
        let child = receipt["pane"]["id"].as_str().unwrap().to_string();
        assert_eq!(st.db.get_pane(&child).unwrap().model.as_deref(), Some("claude-opus-5-5"));
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }
    }

    #[tokio::test]
    async fn task_flow() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("task_create", json!({"title": "Login"})).await;
        assert_ne!(r["isError"], true, "{r}");
        let id = body(&r)["id"].as_str().unwrap().to_string();
        let list = body(&mcp.call("task_list", json!({})).await);
        assert_eq!(list["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(st.db.get_pane("p").unwrap().name, "Login", "pilot takes the first task titles");

        let r = mcp.call("task_update", json!({"id": id, "status": "feito"})).await;
        assert_ne!(r["isError"], true, "{r}");
        let list = body(&mcp.call("task_list", json!({})).await);
        assert_eq!(list["tasks"][0]["status"], "feito");
    }

    #[tokio::test]
    async fn pane_spawn_unknown_cli_is_a_tool_error() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("pane_spawn", json!({"cli": "cmd-fake"})).await;
        assert_eq!(r["isError"], true, "{r}");
    }

    #[tokio::test]
    async fn pane_write_outside_workspace_is_denied() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("pane_write", json!({"pane": "x", "text": "oi"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("painel fora do workspace"), "{r}");
    }

    #[tokio::test]
    async fn handoff_flow() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st).await;
        let child = Mcp::open(&base, "tokC").await;
        let r = child.call("handoff_submit", json!({"summary": "ok"})).await;
        assert_ne!(r["isError"], true, "{r}");
        assert_eq!(body(&r)["deliveredTo"], "p");
        let parent = Mcp::open(&base, "tokP").await;
        let list = body(&parent.call("handoff_list", json!({})).await);
        assert_eq!(list["handoffs"].as_array().unwrap().len(), 1, "{list}");
    }

    #[tokio::test]
    async fn pane_spawn_refuses_risky_command_line_values() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        for args in [json!({"cli": "claude", "model": "a%b"}), json!({"cli": "claude", "effort": "x\"y"}), json!({"cli": "claude", "allowedSkills": ["ok", "z\u{7}"]})] {
            let r = mcp.call("pane_spawn", args).await;
            assert_eq!(r["isError"], true, "{r}");
        }
        assert_eq!(st.panes.ids().len(), 0, "nothing was spawned");
    }

    #[tokio::test]
    async fn memory_global_writes_come_only_from_the_top_and_wait_for_review() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let pilot = Mcp::open(&base, "tokP").await;
        let r = helper.call("memory_write", json!({"kind": "fato", "text": "use pnpm", "escopo": "global"})).await;
        assert_eq!(r["isError"], true, "ajudante não escreve no global: {r}");
        let r = pilot.call("memory_write", json!({"kind": "fato", "text": "use pnpm", "escopo": "global"})).await;
        assert_ne!(r["isError"], true, "{r}");
        assert_eq!(st.db.memories_in(crate::memory::GLOBAL, "sugerida", 10).unwrap().len(), 1, "vira sugestão, não preferência ativa");
        assert!(st.db.memories(crate::memory::GLOBAL, None, 10).unwrap().is_empty());
        assert!(st.db.memories("w", None, 10).unwrap().is_empty(), "o workspace não recebe a escrita global");
    }

    #[tokio::test]
    async fn memory_pin_is_top_only_and_search_reads_global_prefs() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let id = st.db.insert_memory(&crate::db::Memory { workspace_id: "w".into(), kind: "decisao".into(), text: "usar postgres".into(), created_at: 1, ..Default::default() }).unwrap();
        st.db.insert_memory(&crate::db::Memory { workspace_id: crate::memory::GLOBAL.into(), kind: "fato".into(), text: "use pnpm".into(), created_at: 2, ..Default::default() }).unwrap();
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let pilot = Mcp::open(&base, "tokP").await;
        assert_eq!(helper.call("memory_pin", json!({"id": id, "pinned": true})).await["isError"], true, "ajudante não fixa");
        let r = pilot.call("memory_pin", json!({"id": id, "pinned": true})).await;
        assert_ne!(r["isError"], true, "{r}");
        assert!(st.db.get_memory(id).unwrap().unwrap().pinned);
        let r = body(&pilot.call("memory_search", json!({"q": "pnpm", "mode": "significado"})).await);
        assert_eq!(r["memories"][0]["text"], "use pnpm", "lê a preferência global: {r}");
        assert!(r["nota"].as_str().unwrap().contains("palavra"), "{r}");
        let r = pilot.call("memory_search", json!({"q": "pnpm", "mode": "tudo"})).await;
        assert_eq!(r["isError"], true, "modo inválido é recusado");
    }

    #[tokio::test]
    async fn memory_write_then_search() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("memory_write", json!({"kind": "decisao", "text": "usar postgres 16"})).await;
        assert_ne!(r["isError"], true, "{r}");
        let found = body(&mcp.call("memory_search", json!({"q": "postgres"})).await);
        assert_eq!(found["memories"][0]["text"], "usar postgres 16", "{found}");
    }

    #[tokio::test]
    async fn catalog_writes_from_mcp_only_reach_regente_and_skip_originals() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let install = |args: Value| mcp.call("catalog_install", args);
        let r = install(json!({"source": "anthropics/skills", "name": "pdf", "target": "claude"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("UI do Regente"), "{r}");
        let r = install(json!({"source": "anthropics/skills", "name": "sq-qa", "target": "regente"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("reservado"), "{r}");
        let r = mcp.call("catalog_uninstall", json!({"name": "pdf", "kind": "skill", "target": "codex"})).await;
        assert_eq!(r["isError"], true, "{r}");
        let r = mcp.call("catalog_uninstall", json!({"name": "executar", "kind": "skill", "target": "regente"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("reservado"), "{r}");
    }

    #[tokio::test]
    async fn agent_invoke_unknown_agent_is_a_tool_error() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("agent_invoke", json!({"name": "nao-existe", "request": "oi"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(st.panes.ids().len(), 0);
    }

    #[tokio::test]
    async fn agent_invoke_spawns_child_with_prompt() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        // claude stands in for cmd.exe; no prompt argument, so the prompt is pasted
        {
            let mut cfg = crate::util::lk(&st.providers);
            let claude = cfg.clis.iter_mut().find(|c| c.id == "claude").unwrap();
            claude.command = "cmd.exe".into();
            claude.prompt_args.clear();
        }
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("agent_invoke", json!({"name": "sq-qa", "request": "testar a home"})).await;
        assert_ne!(r["isError"], true, "{r}");
        let receipt = body(&r);
        let child = receipt["pane"]["id"].as_str().unwrap().to_string();
        let row = st.db.get_pane(&child).unwrap();
        assert_eq!(row.agent.as_deref(), Some("sq-qa"));
        assert_eq!(row.parent_id.as_deref(), Some("p"));
        assert_eq!(receipt["skills"], json!(["revisar"]), "{receipt}");
        assert_eq!(row.permission, "pend", "agent_invoke is capped to the caller");
        let pasted = String::from_utf8_lossy(&st.rt().initial_input.get(&child).cloned().unwrap_or_default()).to_string();
        assert!(pasted.contains("## Pedido"), "{pasted}");
        assert!(pasted.contains("testar a home"), "{pasted}");
    }

    #[tokio::test]
    async fn two_delegates_resume_a_helper_session_at_most_once() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        agentic(&st, "chain");
        st.db.put_helper_session(&crate::db::HelperSession {
            task_type: "testes".into(),
            mission_id: "m".into(),
            pane_id: "old".into(),
            session_id: Some("S".into()),
            cli: Some("claude".into()),
            closed_at: crate::util::now_ms() - 5 * 60_000,
        })
        .unwrap();
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let first = body(&mcp.call("delegate", json!({"request": "um", "taskType": "testes"})).await);
        let second = body(&mcp.call("delegate", json!({"request": "dois", "taskType": "testes"})).await);
        assert_eq!(first["resumed"], true, "{first}");
        assert_eq!(second["resumed"], false, "{second}");
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }
    }

    #[tokio::test]
    async fn pane_spawn_opens_only_helpers_and_agents_cannot_be_protected_roles() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        for role in ["piloto", "maestro", "chefe:builder"] {
            let r = mcp.call("pane_spawn", json!({"cli": "claude", "role": role})).await;
            assert_eq!(r["isError"], true, "{role}: {r}");
        }
        assert!(st.panes.ids().is_empty(), "nothing was spawned");
        let r = mcp.call("agent_save", json!({"name": "pilotinho", "description": "d", "body": "b", "role": "piloto"})).await;
        assert_eq!(r["isError"], true, "{r}");
    }

    #[tokio::test]
    async fn helper_cap_and_empty_chief_request_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        agentic(&st, "chain");
        // twelve open helpers in the mission: no room for another (rows only, no processes)
        for i in 0..12 {
            st.db.insert_pane(&PaneRow { id: format!("h{i}"), workspace_id: "w".into(), mission_id: Some("m".into()), kind: "agent".into(), name: "h".into(), permission: "pend".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), parent_id: Some("p".into()), role: Some("ajudante".into()), ..Default::default() }).unwrap();
        }
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("delegate", json!({"request": "mais um", "taskType": "testes"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("ajudantes"), "{r}");
        let r = mcp.call("chief_invoke", json!({"type": "builder", "request": "  "})).await;
        assert_eq!(r["isError"], true, "{r}");
    }

    /// `claude` runs as cmd.exe with no prompt argument, so spawns in these tests need no real CLI.
    fn fake_claude(st: &AppState) {
        let mut cfg = crate::util::lk(&st.providers);
        let claude = cfg.clis.iter_mut().find(|c| c.id == "claude").unwrap();
        claude.command = "cmd.exe".into();
        claude.prompt_args.clear();
    }

    /// A helper row in the tool tests' workspace and mission, under `parent`.
    fn child_row(id: &str, parent: &str, kind: &str, permission: &str) -> PaneRow {
        PaneRow {
            id: id.into(),
            workspace_id: "w".into(),
            mission_id: Some("m".into()),
            kind: kind.into(),
            name: id.into(),
            permission: permission.into(),
            cwd: "C:/".into(),
            status: "ocioso".into(),
            parent_id: Some(parent.into()),
            role: Some("ajudante".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn spawn_permission_is_capped_to_the_callers() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        fake_claude(&st);
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        // p is pend: danger is refused outright
        let r = mcp.call("pane_spawn", json!({"cli": "claude", "permission": "danger"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("danger"), "{r}");
        assert!(st.panes.ids().is_empty(), "nothing was spawned");
        // auto is capped to the caller's pend
        let spawned = body(&mcp.call("pane_spawn", json!({"cli": "claude", "permission": "auto"})).await);
        let child = spawned["panes"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(st.db.get_pane(&child).unwrap().permission, "pend");
        for id in st.panes.ids() {
            st.close_pane(&id).unwrap();
        }
    }

    #[tokio::test]
    async fn pane_targets_follow_the_tree_and_the_kind() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        st.db.insert_pane(&child_row("s", "p", "agent", "pend")).unwrap(); // sibling of the helper c
        st.db.insert_pane(&child_row("t", "p", "terminal", "pend")).unwrap(); // a terminal under the pilot
        st.db.insert_pane(&child_row("a", "c", "agent", "auto")).unwrap(); // a stronger child of the helper
        // Waking keeps writes queued in pending_input, so no process is needed
        st.rt().statuses.insert("c".into(), crate::status::PaneStatus::Waking);
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let pilot = Mcp::open(&base, "tokP").await;
        let refused = |r: &Value| r["isError"] == true;
        assert!(refused(&helper.call("pane_write", json!({"pane": "s", "text": "oi"})).await), "helper into its sibling");
        assert!(refused(&helper.call("pane_write", json!({"pane": "p", "text": "oi"})).await), "helper into the pilot");
        assert!(refused(&helper.call("pane_write", json!({"pane": "a", "text": "oi"})).await), "pend caller into an auto child");
        assert!(refused(&pilot.call("pane_write", json!({"pane": "t", "text": "oi"})).await), "write into a terminal");
        assert!(refused(&pilot.call("pane_read", json!({"pane": "t"})).await), "read of a terminal");
        assert!(!refused(&pilot.call("pane_write", json!({"pane": "c", "text": "oi"})).await), "pilot into its helper");
        assert!(st.rt().pending_input.contains_key("c"));
    }


    #[test]
    fn papel_curto_ve_so_o_nucleo_e_papel_longo_ve_tudo() {
        assert_eq!(tools_for_role(Some("ajudante")).unwrap(), NUCLEO.to_vec());
        assert!(tools_for_role(Some("reviewer")).is_some());
        assert!(tools_for_role(Some("piloto")).is_none());
        assert!(tools_for_role(None).is_none());
        let ocultas = ocultas_para(Some("ajudante"));
        assert!(ocultas.contains(&"pane_spawn".to_string()));
        assert!(!ocultas.contains(&"handoff_submit".to_string()));
        assert!(ocultas_para(Some("piloto")).is_empty());
    }

    #[test]
    fn handoff_submit_is_always_loaded_and_descriptions_fit() {
        let tools = RegenteMcp::all().list_all();
        let submit = tools.iter().find(|t| t.name == "handoff_submit").unwrap();
        assert_eq!(submit.meta.as_ref().and_then(|m| m.get("anthropic/alwaysLoad")), Some(&json!(true)));
        for t in &tools {
            let n = t.description.as_deref().unwrap_or_default().chars().count();
            assert!(n <= 200, "{}: descrição com {n} caracteres", t.name);
        }
    }

    #[tokio::test]
    async fn ferramenta_oculta_do_ajudante_e_recusada() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let r = helper.call("pane_spawn", json!({"cli": "claude", "count": 1})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("ferramenta indisponível para o papel ajudante"), "{r}");
        assert!(st.panes.ids().is_empty());
        // a ferramenta do núcleo segue para o roteador (erro de argumento, não de papel)
        let r = helper.call("handoff_get", json!({"id": "inexistente"})).await;
        assert!(!r.to_string().contains("indisponível"), "{r}");
    }

    #[tokio::test]
    async fn handoff_artifact_stays_on_disk_and_reads_are_sliced_and_scoped() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let r = helper.call("handoff_submit", json!({"summary": "ok", "artifact": {"content": [{"id": 1, "nome": "alfa"}, {"id": 2, "nome": "beta"}], "format": "json"}})).await;
        assert!(r["isError"] != true, "{r}");
        let h = st.db.handoffs(Some("m"), None, 10).unwrap().into_iter().find(|h| h.from_pane == "c").unwrap();
        let data = h.data_json.clone().unwrap_or_default();
        assert!(!data.contains("beta"), "o conteúdo não vai para a entrega: {data}");
        let v: Value = serde_json::from_str(&data).unwrap();
        let path = v["artifact"]["path"].as_str().unwrap().to_string();
        assert_eq!(v["artifact"]["format"], "json");
        assert_eq!(v["artifact"]["rows"], 2);
        // o piloto lê só o campo pedido, com limite
        let pilot = Mcp::open(&base, "tokP").await;
        let r = pilot.call("artifact_read", json!({"path": path, "fields": ["nome"], "limit": 1, "format": "json"})).await;
        assert_eq!(r["content"][0]["text"], r#"[{"nome":"alfa"}]"#, "{r}");
        // fora da pasta, de outra missão e com `..`: recusados
        for bad in ["../segredo.txt", ".regente/artefatos/outra/x.json", ".regente/artefatos/m/../../x.json"] {
            let r = pilot.call("artifact_read", json!({"path": bad})).await;
            assert_eq!(r["isError"], true, "{bad}: {r}");
        }
    }

    #[tokio::test]
    async fn handoff_list_is_concise_and_get_has_detail_on_request() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let longo = "r".repeat(300);
        let r = helper.call("handoff_submit", json!({"summary": longo, "data": {"ok": true, "extra": 1}})).await;
        assert!(r["isError"] != true, "{r}");
        let r = helper.call("handoff_submit", json!({"summary": "x".repeat(401)})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("Resumo com 401 caracteres"), "{r}");
        let pilot = Mcp::open(&base, "tokP").await;
        let lista = body(&pilot.call("handoff_list", json!({})).await);
        let item = &lista["handoffs"][0];
        let mut chaves: Vec<&str> = item.as_object().unwrap().keys().map(String::as_str).collect();
        chaves.sort();
        assert_eq!(chaves, ["from", "id", "ok", "summary", "taskType"]);
        assert_eq!(item["summary"].as_str().unwrap().chars().count(), 120);
        let id = item["id"].as_str().unwrap().to_string();
        let curta = body(&pilot.call("handoff_get", json!({"id": id})).await);
        assert_eq!(curta["dataKeys"], json!(["extra", "ok"]));
        assert!(curta.get("dataJson").is_none());
        let cheia = body(&pilot.call("handoff_get", json!({"id": id, "detail": true})).await);
        assert!(cheia.get("dataJson").is_some(), "{cheia}");
    }
    #[tokio::test]
    async fn helper_cannot_run_mission_squad_or_catalog_writes() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let refused = |r: &Value| r["isError"] == true;
        assert!(refused(&helper.call("mission_set_status", json!({"status": "concluida"})).await), "status");
        assert!(refused(&helper.call("mission_integrate", json!({})).await), "integrate");
        assert!(refused(&helper.call("mission_brief", json!({"set": "novo"})).await), "brief");
        assert!(refused(&helper.call("squad_spawn", json!({"squadId": "jogo-web", "brief": "um jogo"})).await), "squad_spawn");
        assert!(refused(&helper.call("agent_save", json!({"name": "novo-agente", "description": "d", "body": "b"})).await), "agent_save");
        assert!(refused(&helper.call("catalog_install", json!({"source": "anthropics/skills", "name": "pdf", "target": "regente"})).await), "catalog_install");
        assert!(refused(&helper.call("catalog_uninstall", json!({"name": "pdf", "kind": "skill", "target": "regente"})).await), "catalog_uninstall");
        // hidden for a helper: the role filter refuses it before the tree rule runs
        let r = helper.call("mission_set_status", json!({"status": "concluida"})).await;
        assert!(r.to_string().contains("indisponível para o papel ajudante"), "{r}");
        assert!(st.panes.ids().is_empty());
    }

    #[tokio::test]
    async fn rename_and_saves_need_the_right_standing() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        st.db.insert_pane(&child_row("s", "p", "agent", "pend")).unwrap(); // sibling of the helper c
        let base = spawn_server(st.clone()).await;
        let helper = Mcp::open(&base, "tokC").await;
        let pilot = Mcp::open(&base, "tokP").await;
        let refused = |r: &Value| r["isError"] == true;
        // renaming types into the pane: only over a pane the caller controls
        assert!(refused(&helper.call("pane_rename", json!({"pane": "s", "name": "x"})).await), "helper renames its sibling");
        assert!(refused(&helper.call("pane_rename", json!({"pane": "p", "name": "x"})).await), "helper renames the pilot");
        assert!(!refused(&pilot.call("pane_rename", json!({"pane": "c", "name": "ajudante-1"})).await), "pilot renames its helper");
        // saving a recipe or a squad over MCP is the top role's job
        let recipe = json!({"recipe": {"id": "nova-receita", "name": "x", "description": "", "mode": "livre"}});
        assert!(refused(&helper.call("recipe_save", recipe).await), "helper saves a recipe");
        let squad = json!({"squad": {"id": "novo-time", "name": "S", "description": "d", "phases": []}});
        assert!(refused(&helper.call("squad_save", squad).await), "helper saves a squad");
    }

    #[test]
    fn danger_caller_never_gets_danger_by_default() {
        let danger = PaneRow { permission: "danger".into(), ..Default::default() };
        assert_eq!(cap_perm(&danger, None).unwrap(), Permission::Auto);
        assert_eq!(cap_perm(&danger, Some(Permission::Pend)).unwrap(), Permission::Pend);
        assert_eq!(cap_perm(&danger, Some(Permission::Danger)).unwrap_err().code(), "forbidden");
    }

    #[tokio::test]
    async fn recipe_slots_are_capped_to_the_callers_permission() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        let base = spawn_server(st.clone()).await;
        // p is pend: a slot that asks for auto is stored as pend
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("recipe_save", json!({"recipe": {"id": "slot-auto", "name": "x", "description": "", "mode": "livre", "slots": [{"cli": "claude", "permission": "auto"}]}})).await;
        assert_ne!(r["isError"], true, "{r}");
        let saved = crate::recipes::get(d.path(), "slot-auto").unwrap();
        assert_eq!(saved.slots[0]["permission"], "pend");
    }

    #[tokio::test]
    async fn archived_mission_refuses_new_panes() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        fake_claude(&st);
        st.db
            .insert_mission(&crate::db::Mission {
                id: "m".into(),
                workspace_id: "w".into(),
                name: "Velha".into(),
                mode: "livre".into(),
                status: "arquivada".into(),
                status_reason: None,
                worktree_path: None,
                branch: None,
                base_branch: None,
                pinned: false,
                created_at: 0,
                archived_at: Some(1),
                squad_id: None,
                recipe_id: None,
                brief: None,
                style: None,
                profile: None, plan_json: None, verify_json: None, adopted: false,
            })
            .unwrap();
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("pane_spawn", json!({"cli": "claude"})).await;
        assert!(r.to_string().contains("arquivada"), "{r}");
        let r = mcp.call("agent_invoke", json!({"name": "sq-qa", "request": "oi"})).await;
        assert!(r.to_string().contains("arquivada"), "{r}");
        assert!(st.panes.ids().is_empty(), "nothing was spawned");
    }

    #[tokio::test]
    async fn too_many_open_panes_refuse_a_spawn() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        fake_claude(&st);
        // p and c are open already: 22 more make 24, and one more spawn is 25
        for i in 0..22 {
            st.db.insert_pane(&child_row(&format!("f{i}"), "p", "agent", "pend")).unwrap();
        }
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("pane_spawn", json!({"cli": "claude"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("24 painéis"), "{r}");
    }

    #[tokio::test]
    async fn roster_max_refuses_another_of_the_same_agent() {
        let d = tempfile::tempdir().unwrap();
        let st = state(d.path());
        seed(&st, d.path());
        fake_claude(&st);
        // a squad whose roster allows one sq-qa at a time
        std::fs::create_dir_all(d.path().join("squads")).unwrap();
        std::fs::write(d.path().join("squads").join("time-teste.json"), r#"{"id":"time-teste","name":"Time","roster":[{"agent":"sq-qa","max":1}]}"#).unwrap();
        st.db
            .insert_mission(&crate::db::Mission {
                id: "m".into(),
                workspace_id: "w".into(),
                name: "Time".into(),
                mode: "squad".into(),
                status: "ativa".into(),
                status_reason: None,
                worktree_path: None,
                branch: None,
                base_branch: None,
                pinned: false,
                created_at: 0,
                archived_at: None,
                squad_id: Some("time-teste".into()),
                recipe_id: None,
                brief: None,
                style: None,
                profile: None, plan_json: None, verify_json: None, adopted: false,
            })
            .unwrap();
        st.db.insert_pane(&PaneRow { agent: Some("sq-qa".into()), ..child_row("q1", "p", "agent", "pend") }).unwrap();
        let base = spawn_server(st.clone()).await;
        let mcp = Mcp::open(&base, "tokP").await;
        let r = mcp.call("agent_invoke", json!({"name": "sq-qa", "request": "outra vez"})).await;
        assert_eq!(r["isError"], true, "{r}");
        assert!(r.to_string().contains("já tem 1"), "{r}");
        assert_eq!(st.panes.ids().len(), 0, "the refused call spawned nothing");
    }

    #[test]
    fn pre_allow_is_the_whole_server_for_auto_and_skips_mutations_for_pend() {
        assert_eq!(pre_allowed(Permission::Auto), vec!["mcp__regente__*"]);
        assert_eq!(pre_allowed(Permission::Danger), vec!["mcp__regente__*"]);
        let pend = pre_allowed(Permission::Pend);
        assert!(pend.contains(&"mcp__regente__pane_read".to_string()));
        assert!(pend.contains(&"mcp__regente__delegate".to_string()));
        for m in MUTATING_TOOLS {
            assert!(!pend.contains(&format!("mcp__regente__{m}")), "{m} must stay on the ask list");
        }
        assert_eq!(pre_allowed(Permission::Plan), pend);
    }

    #[test]
    fn mutating_tools_exist_in_the_router() {
        let names: Vec<String> = RegenteMcp::all().list_all().iter().map(|t| t.name.to_string()).collect();
        for m in MUTATING_TOOLS {
            assert!(names.iter().any(|n| n == m), "{m} is not a tool of the router");
        }
    }

    #[test]
    fn hookcli_without_env_is_noop() {
        let args: Vec<String> = ["x", "--hook", "Stop"].iter().map(|s| s.to_string()).collect();
        assert_eq!(crate::hookcli::run(&args), 0);
    }
}
