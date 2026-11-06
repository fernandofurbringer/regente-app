//! Controle pelo celular (Configurações → Celular): servidor HTTPS na rede local, pareamento, API e WebSocket.
//! O celular só faz o que esta página lista: ler workspaces, missões, painéis e gates; abrir painel (nunca em
//! danger); digitar em painel de IA; acordar um hibernado; responder gate; propor missão (confirmada no
//! computador). Texto para painel passa por `clean_text`/`bracketed` (via `jarvis::write_bytes`).

use crate::db::{MobileDevice, PaneRow};
use crate::error::{fail, AppError, AppResult};
use crate::jarvis;
use crate::mcp::tools_panes::plain_arg;
use crate::orchestrator::{last_lines, strip_ansi};
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::squads;
use crate::status::PaneStatus;
use crate::util::{lk, new_id, now_ms};
use crate::vault::{random, to_hex};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use axum_server::tls_rustls::{from_tcp_rustls, RustlsConfig};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path as FsPath;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, State as TState};
use tokio::sync::{broadcast, mpsc};

pub const PORT: u16 = 8789;
const PAIR_TTL_MS: i64 = 10 * 60_000;
/// A code shown on the desktop works this long.
const CODE_TTL_MS: i64 = 5 * 60_000;
/// Failed codes before the code rotates (a new epoch, same deadline).
const CODE_FAILS_ROTATE: u32 = 5;
/// Pairing failures from everyone, within the window, lock `/api/pair` for the lock time.
const GLOBAL_PAIR_FAILS: usize = 30;
const GLOBAL_WINDOW_MS: i64 = 5 * 60_000;
const GLOBAL_LOCK_MS: i64 = 5 * 60_000;
const MAX_FAILS: u32 = 10;
const LOCK_MS: i64 = 60_000;
/// Failed sign-in counts reset after this long without a failure.
const FAIL_FORGET_MS: i64 = 10 * 60_000;
const BODY_MAX: usize = 16 * 1024;
const REQ_TIMEOUT: Duration = Duration::from_secs(30);
const WS_AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// Envio de um aviso ao celular: passou disso, a conexão é cortada.
const WS_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const WS_MSG_MAX: usize = 16 * 1024;
const WS_INPUT_PER_MIN: usize = 60;
const WS_PER_DEVICE: usize = 4;
const WS_TOTAL: usize = 16;
const WS_OUT_BUFFER: usize = 256;
/// A live socket re-checks its device this often (and on each input).
const WS_RECHECK: Duration = Duration::from_secs(30);
/// Confirms asked of the desktop (texts, missions) per device per minute; one pending at a time per device.
const CONFIRM_PER_MIN: usize = 5;
const CONFIRM_PENDING_MS: i64 = 60_000;
/// Requests in flight at once, across every route.
const REQ_SLOTS: usize = 64;
/// Header read timeout of a connection (slow-loris).
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_PER_MIN: usize = 3;
const WAKE_PER_MIN: usize = 6;
const CONVERSATION_LINES: usize = 300;
/// Turns of a Claude conversation sent to the phone, and the bytes read from the end of its session file.
const CONVERSATION_ITEMS: usize = 200;
const ITEMS_READ_BYTES: u64 = 1 << 20;
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; font-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// Static files of the PWA (`mobile.html`, assets, manifest): path -> (bytes, mime type). Tauri's asset resolver in the app.
pub type Assets = Arc<dyn Fn(&str) -> Option<(Vec<u8>, String)> + Send + Sync>;

/// SHA-256 (hex) of a device token. The token itself is never stored.
pub fn token_hash(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

/// Constant-time equality: no early exit on the first differing byte.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether `token` hashes to the stored `hash`.
pub fn verify_token(hash: &str, token: &str) -> bool {
    same(hash.as_bytes(), token_hash(token).as_bytes())
}

/// Pairing secrets of one server run. The QR token: single use, 10 min. The code: inactive until the user
/// shows it on the desktop, works 5 min, is burned by a success, and rotates after 5 failures.
pub struct Pairing {
    token: Option<(String, i64)>,
    secret: [u8; 32],
    /// The shown code and its deadline. None = inactive.
    code: Option<(String, i64)>,
    epoch: u32,
    code_fails: u32,
}

impl Pairing {
    pub fn new() -> AppResult<Self> {
        Ok(Self { token: None, secret: random::<32>()?, code: None, epoch: 0, code_fails: 0 })
    }

    #[cfg(test)]
    fn with_secret(secret: [u8; 32]) -> Self {
        Self { token: None, secret, code: None, epoch: 0, code_fails: 0 }
    }

    /// A new token (32 random bytes, hex). It replaces the previous one.
    pub fn issue_token(&mut self, now: i64) -> AppResult<String> {
        let token = to_hex(&random::<32>()?);
        self.token = Some((token.clone(), now + PAIR_TTL_MS));
        Ok(token)
    }

    /// Accepts the token once, only before it expires. A wrong guess does not spend the real token.
    pub fn take_token(&mut self, presented: &str, now: i64) -> bool {
        match &self.token {
            Some((t, until)) if now < *until && same(t.as_bytes(), presented.as_bytes()) => {
                self.token = None;
                true
            }
            _ => false,
        }
    }

    fn derive(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.secret);
        h.update(self.epoch.to_be_bytes());
        let d = h.finalize();
        let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
        format!("{n:06}")
    }

    /// Shows a new code for 5 minutes (the user clicked "Mostrar código"). The previous code is gone.
    pub fn show_code(&mut self, now: i64) -> String {
        self.epoch = self.epoch.wrapping_add(1);
        self.code_fails = 0;
        let code = self.derive();
        self.code = Some((code.clone(), now + CODE_TTL_MS));
        code
    }

    /// The code while it is shown and not expired.
    pub fn code_now(&self, now: i64) -> Option<String> {
        self.code.as_ref().filter(|(_, until)| now < *until).map(|(c, _)| c.clone())
    }

    /// Checks a code. A success burns it. A failure counts; the 5th rotates the code (new epoch, same deadline).
    pub fn check_code(&mut self, presented: &str, now: i64) -> bool {
        let Some((code, until)) = self.code.clone() else { return false };
        if now >= until {
            self.code = None;
            return false;
        }
        if same(code.as_bytes(), presented.trim().as_bytes()) {
            self.code = None;
            self.code_fails = 0;
            return true;
        }
        self.code_fails += 1;
        if self.code_fails >= CODE_FAILS_ROTATE {
            self.epoch = self.epoch.wrapping_add(1);
            self.code_fails = 0;
            self.code = Some((self.derive(), until));
        }
        false
    }
}

#[derive(Clone, Copy)]
struct Fail {
    n: u32,
    until: i64,
    last: i64,
}

/// Failed sign-ins per IP (10 failures lock the IP for 60 s), pairing failures from everyone, and sliding
/// windows of actions per device.
#[derive(Default)]
pub struct Limits {
    fails: HashMap<IpAddr, Fail>,
    hits: HashMap<(String, &'static str), VecDeque<i64>>,
    pair_fails: VecDeque<i64>,
    pair_locked_until: i64,
}

impl Limits {
    pub fn locked(&self, ip: IpAddr, now: i64) -> bool {
        self.fails.get(&ip).is_some_and(|f| now < f.until)
    }

    pub fn failed(&mut self, ip: IpAddr, now: i64) {
        let f = self.fails.entry(ip).or_insert(Fail { n: 0, until: 0, last: now });
        if now < f.until {
            return;
        }
        if now - f.last > FAIL_FORGET_MS {
            f.n = 0;
        }
        f.n += 1;
        f.last = now;
        if f.n >= MAX_FAILS {
            f.n = 0;
            f.until = now + LOCK_MS;
        }
    }

    pub fn succeeded(&mut self, ip: IpAddr) {
        self.fails.remove(&ip);
    }

    /// Pairing is locked for everyone after 30 failures within 5 min.
    pub fn pair_locked(&self, now: i64) -> bool {
        now < self.pair_locked_until
    }

    pub fn pair_failed(&mut self, now: i64) {
        self.pair_fails.retain(|t| now - t < GLOBAL_WINDOW_MS);
        self.pair_fails.push_back(now);
        if self.pair_fails.len() >= GLOBAL_PAIR_FAILS {
            self.pair_locked_until = now + GLOBAL_LOCK_MS;
            self.pair_fails.clear();
        }
    }

    /// At most `max` actions of this kind per `window` ms for this key. Entries that went quiet are pruned here.
    pub fn allow(&mut self, key: &str, action: &'static str, max: usize, window: i64, now: i64) -> bool {
        // every window in use is at most 60 s, so a queue whose newest hit is older than that is dead
        self.hits.retain(|_, q| q.back().is_some_and(|t| now - t < 60_000));
        self.fails.retain(|_, f| now < f.until || now - f.last < FAIL_FORGET_MS);
        let q = self.hits.entry((key.to_string(), action)).or_default();
        while q.front().is_some_and(|t| now - t >= window) {
            q.pop_front();
        }
        if q.len() >= max {
            return false;
        }
        q.push_back(now);
        true
    }
}

/// Host header allowed: `localhost` or an IPv4 literal of loopback, private (RFC 1918) or CGNAT range, on the
/// server's port. DNS names and IPv6 are refused (DNS rebinding).
pub fn host_allowed(host: &str, port: u16) -> bool {
    let Some((name, p)) = host.rsplit_once(':') else { return false };
    if p.parse::<u16>().ok() != Some(port) {
        return false;
    }
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match name.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || cgnat(v4),
        _ => false,
    }
}

fn cgnat(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

/// Whether a TCP peer may talk to the server: this machine, the LAN (RFC 1918), CGNAT 100.64/10, or IPv6
/// loopback, link-local (fe80::/10) and ULA (fc00::/7). Everything else is refused at the door.
pub fn peer_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || cgnat(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => peer_allowed(IpAddr::V4(v4)),
            None => {
                let first = v6.segments()[0];
                v6.is_loopback() || (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00
            }
        },
    }
}

/// Whether the phone's text goes straight to the pane: an AI pane that is not in danger and not asking.
/// Terminals, danger panes and a pane waiting for an answer need the user's confirmation on the desktop.
pub fn input_is_direct(kind: &str, permission: &str, asking: bool) -> bool {
    kind == "agent" && permission != "danger" && !asking
}

/// Keys the phone may send to an AI pane: fixed sequences only, no free-form escapes.
fn key_bytes(key: &str) -> Option<&'static [u8]> {
    let seq = match key {
        "enter" => "\r",
        "esc" => "\x1b",
        "ctrl-c" => "\x03",
        "tab" => "\t",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        _ => return None,
    };
    Some(seq.as_bytes())
}

/// Name of a phone: no control chars, at most 40 chars.
fn device_name(raw: Option<&str>) -> String {
    let n: String = raw.unwrap_or("").chars().filter(|c| !c.is_control()).take(40).collect();
    let n = n.trim();
    if n.is_empty() { "Celular".into() } else { n.into() }
}

/// Endereço que o painel mostra: só neste computador no modo HTTP; senão o IP da rede local (HTTPS).
pub fn base_url(http_only: bool, lan: Option<IpAddr>, port: u16) -> String {
    if http_only {
        return format!("http://127.0.0.1:{port}");
    }
    let host = lan.map_or("localhost".to_string(), |ip| ip.to_string());
    format!("https://{host}:{port}")
}

/// The address this machine uses on the LAN: a UDP socket picks the route, and `connect` sends nothing.
pub fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("10.255.255.255:1").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// Shared by the router and the handlers: the app state, limits, pairing and the live WebSocket counts.
pub struct Shared {
    state: Arc<AppState>,
    port: u16,
    tls: bool,
    pairing: Mutex<Pairing>,
    limits: Mutex<Limits>,
    /// Revoked device ids (`"*"` = all): a WebSocket of that device closes.
    revoked: broadcast::Sender<String>,
    assets: Assets,
    /// Open WebSockets per device.
    ws_open: Mutex<HashMap<String, usize>>,
    /// Device -> until when its confirm (asked of the desktop) counts as pending.
    confirm_pending: Mutex<HashMap<String, i64>>,
    /// Requests in flight (see `REQ_SLOTS`).
    req_slots: Arc<tokio::sync::Semaphore>,
}

type Ctx = Arc<Shared>;

#[derive(Clone)]
struct DeviceId(String);

/// One open WebSocket of a device; dropping it frees the slot.
struct WsSlot {
    ctx: Ctx,
    dev: String,
}

impl Drop for WsSlot {
    fn drop(&mut self) {
        let mut open = lk(&self.ctx.ws_open);
        if let Some(n) = open.get_mut(&self.dev) {
            *n -= 1;
            if *n == 0 {
                open.remove(&self.dev);
            }
        }
    }
}

fn take_ws_slot(c: &Ctx, dev: &str) -> Option<WsSlot> {
    let mut open = lk(&c.ws_open);
    let total: usize = open.values().sum();
    if total >= WS_TOTAL || open.get(dev).copied().unwrap_or(0) >= WS_PER_DEVICE {
        return None;
    }
    *open.entry(dev.to_string()).or_insert(0) += 1;
    Some(WsSlot { ctx: c.clone(), dev: dev.to_string() })
}

impl Shared {
    /// The device of a bearer token. Every active device is compared, and a miss counts toward the IP's lock.
    /// A device still waiting for approval gets 403 `pending_approval`.
    fn sign_in(&self, token: &str, ip: IpAddr) -> Result<String, Response> {
        let now = now_ms();
        if lk(&self.limits).locked(ip, now) {
            return Err(api_err(StatusCode::TOO_MANY_REQUESTS, "locked", "muitas tentativas; espere 1 minuto"));
        }
        let hash = token_hash(token);
        let devices = self.state.db.mobile_active_hashes().map_err(app_err)?;
        let mut hit = None;
        for (id, h, pending) in devices {
            if same(h.as_bytes(), hash.as_bytes()) {
                hit = Some((id, pending));
            }
        }
        match hit {
            Some((id, false)) => {
                lk(&self.limits).succeeded(ip);
                let _ = self.state.db.mobile_touch(&id, now);
                Ok(id)
            }
            Some((_, true)) => Err(api_err(StatusCode::FORBIDDEN, "pending_approval", "aguardando aprovação no computador")),
            None => {
                lk(&self.limits).failed(ip, now);
                Err(api_err(StatusCode::UNAUTHORIZED, "unauthorized", "token inválido"))
            }
        }
    }

    /// Name of a paired phone, as the confirm modal shows it.
    fn device_name(&self, id: &str) -> String {
        self.state.db.mobile_devices().ok().and_then(|list| list.into_iter().find(|d| d.id == id)).map_or_else(|| "Celular".to_string(), |d| d.name)
    }

    /// Still paired and approved (checked on each input and every 30 s by a live socket).
    fn device_ok(&self, id: &str) -> bool {
        self.state.db.mobile_device_ok(id).unwrap_or(false)
    }

    fn allow(&self, device: &str, action: &'static str, max: usize) -> bool {
        lk(&self.limits).allow(device, action, max, 60_000, now_ms())
    }

    /// A confirm asked of the desktop: 5 a minute per device, and only one pending at a time per device.
    fn confirm_slot(&self, device: &str) -> AppResult<()> {
        let now = now_ms();
        let mut pending = lk(&self.confirm_pending);
        if pending.get(device).is_some_and(|until| now < *until) {
            return Err(AppError::new("rate", "já há uma confirmação pendente no computador"));
        }
        if !lk(&self.limits).allow(device, "confirm", CONFIRM_PER_MIN, 60_000, now) {
            return Err(AppError::new("rate", "limite de 5 confirmações por minuto"));
        }
        pending.insert(device.to_string(), now + CONFIRM_PENDING_MS);
        Ok(())
    }
}

fn bearer(headers: &HeaderMap) -> &str {
    headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok()).and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("")
}

fn api_err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"error": code, "message": message}))).into_response()
}

/// Domain error -> HTTP status with the stable code.
fn app_err(e: AppError) -> Response {
    let status = match e.code() {
        "not_found" => StatusCode::NOT_FOUND,
        "forbidden" => StatusCode::FORBIDDEN,
        "gate_closed" | "bad_state" | "desktop_only" | "not_running" => StatusCode::CONFLICT,
        "too_big" | "bad_input" | "bad_arg" | "bad_decision" | "bad_squad" => StatusCode::BAD_REQUEST,
        "rate" => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    api_err(status, e.code(), &e.to_string())
}

fn res<T: Serialize>(r: AppResult<T>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => app_err(e),
    }
}

/// The request's host: the Host header (HTTP/1.1) or, over HTTP/2 (browsers negotiate it on TLS), the `:authority`,
/// which hyper exposes as the URI authority because h2 requests carry no Host header.
fn request_host(req: &Request) -> String {
    req.headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()))
        .unwrap_or_default()
}

/// Peer, Host and Origin checks, security headers, and a request timeout. No CORS header is ever sent,
/// so only the page itself (same origin) can call the API.
async fn guard(State(c): State<Ctx>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let host = request_host(&req);
    let scheme = if c.tls { "https" } else { "http" };
    let origin_ok = req.headers().get(header::ORIGIN).is_none_or(|o| o.to_str().ok() == Some(format!("{scheme}://{host}").as_str()));
    if !peer_allowed(peer.ip()) || !host_allowed(&host, c.port) || !origin_ok {
        return StatusCode::FORBIDDEN.into_response();
    }
    // requests in flight; a WebSocket upgrade holds its slot only until it is answered
    let Ok(_slot) = c.req_slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let mut res = match tokio::time::timeout(REQ_TIMEOUT, next.run(req)).await {
        Ok(r) => r,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    };
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("permissions-policy", HeaderValue::from_static("camera=(), microphone=(), geolocation=()"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    res
}

/// Bearer token -> device; the request gets a `DeviceId` extension.
async fn auth(State(c): State<Ctx>, ConnectInfo(ip): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    match c.sign_in(bearer(req.headers()), ip.ip()) {
        Ok(id) => {
            req.extensions_mut().insert(DeviceId(id));
            next.run(req).await
        }
        Err(r) => r,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairBody {
    token: Option<String>,
    code: Option<String>,
    device_name: Option<String>,
}

async fn pair(State(c): State<Ctx>, ConnectInfo(ip): ConnectInfo<SocketAddr>, Json(b): Json<PairBody>) -> Response {
    let (ip, now) = (ip.ip(), now_ms());
    if lk(&c.limits).pair_locked(now) {
        return api_err(StatusCode::TOO_MANY_REQUESTS, "pair_locked", "pareamento bloqueado por 5 minutos");
    }
    if lk(&c.limits).locked(ip, now) {
        return api_err(StatusCode::TOO_MANY_REQUESTS, "locked", "muitas tentativas; espere 1 minuto");
    }
    // a code needs no token: its device waits for the desktop's approval. The QR token is trusted (shown on the desktop).
    let by_code = b.token.is_none();
    let ok = {
        let mut p = lk(&c.pairing);
        match (b.token.as_deref(), b.code.as_deref()) {
            (Some(t), _) => p.take_token(t, now),
            (None, Some(code)) => p.check_code(code, now),
            _ => false,
        }
    };
    if !ok {
        lk(&c.limits).failed(ip, now);
        lk(&c.limits).pair_failed(now);
        return api_err(StatusCode::UNAUTHORIZED, "pair_refused", "código ou token inválido");
    }
    lk(&c.limits).succeeded(ip);
    let token = match crate::mcp::new_token() {
        Ok(t) => t,
        Err(e) => return app_err(e),
    };
    let id = new_id();
    let name = device_name(b.device_name.as_deref());
    if let Err(e) = c.state.db.mobile_device_add(&id, &name, &token_hash(&token), now, by_code) {
        return app_err(e);
    }
    if by_code {
        c.state.emit("mobile-pair-request", json!({"deviceId": id, "deviceName": name}));
    }
    Json(json!({"deviceToken": token, "deviceId": id, "name": name, "pending": by_code})).into_response()
}

async fn workspaces(State(c): State<Ctx>) -> Response {
    res(c.state.db.list_workspaces())
}

async fn missions(State(c): State<Ctx>, Path(ws): Path<String>) -> Response {
    res(c.state.db.list_missions(&ws))
}

fn pane_summary(p: &PaneRow, status: String) -> Value {
    json!({"id": p.id, "workspaceId": p.workspace_id, "missionId": p.mission_id, "parentId": p.parent_id, "name": p.name, "kind": p.kind, "status": status, "permission": p.permission, "role": p.role, "cli": p.cli, "costUsd": p.cost_usd})
}

async fn panes(State(c): State<Ctx>, Path(ws): Path<String>) -> Response {
    res(c.state.list_panes(&ws).map(|rows| rows.iter().map(|p| pane_summary(p, p.status.clone())).collect::<Vec<_>>()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnBody {
    workspace_id: String,
    /// "agent" (needs `cli`) or "terminal".
    kind: String,
    cli: Option<String>,
    name: Option<String>,
    prompt: Option<String>,
}

/// Opens a pane in the workspace folder. Never `danger`; an agent starts in `pend`.
fn open_pane(s: &AppState, b: &SpawnBody) -> AppResult<Value> {
    let w = s.db.get_workspace(&b.workspace_id)?;
    let name = b.name.as_deref().map(jarvis::label).filter(|n| !n.trim().is_empty());
    let spec = match b.kind.as_str() {
        "agent" => {
            let cli = b.cli.as_deref().filter(|c| !c.trim().is_empty()).ok_or_else(|| AppError::new("bad_input", "falta o cli"))?;
            plain_arg(cli)?;
            let prompt = b.prompt.clone().filter(|p| !p.is_empty());
            if let Some(p) = &prompt {
                crate::mcp::check_texts(&[p])?;
            }
            PaneSpec { workspace_id: w.id.clone(), kind: PaneKind::Agent, name, cli: Some(cli.into()), permission: Permission::Pend, cwd: w.path.clone(), prompt, ..Default::default() }
        }
        "terminal" => PaneSpec { workspace_id: w.id.clone(), kind: PaneKind::Terminal, name, permission: Permission::Pend, cwd: w.path.clone(), ..Default::default() },
        _ => return fail("bad_input", "tipo de painel desconhecido"),
    };
    let row = s.spawn_pane(spec)?;
    Ok(json!({"id": row.id, "name": row.name}))
}

async fn spawn(State(c): State<Ctx>, Extension(dev): Extension<DeviceId>, Json(b): Json<SpawnBody>) -> Response {
    if !c.allow(&dev.0, "spawn", SPAWN_PER_MIN) {
        return api_err(StatusCode::TOO_MANY_REQUESTS, "rate", "limite de 3 aberturas por minuto");
    }
    res(open_pane(&c.state, &b))
}

#[derive(Deserialize)]
struct InputBody {
    text: Option<String>,
    key: Option<String>,
    enter: Option<bool>,
}

/// Text or a key for a pane. Direct for an AI pane (keys only there); otherwise a text waits for the desktop's
/// confirmation (`jarvis-confirm`), as Jarvis does. `confirm` = the device's shared state when a confirm may be
/// asked (HTTP); None (the WebSocket) refuses instead.
fn send_input(s: &AppState, id: &str, b: InputBody, confirm: Option<(&Shared, &str)>) -> AppResult<Value> {
    let row = s.db.get_pane(id)?;
    if row.closed_at.is_some() {
        return fail("not_found", "painel fechado");
    }
    // a pane that just moved to or from "pergunta" is still the user's: the phone's input waits for the desktop
    let asking = s.status_of(id) == Some(PaneStatus::Asking) || s.asking_recent(id);
    let direct = input_is_direct(&row.kind, &row.permission, asking);
    if let Some(key) = b.key.as_deref() {
        let bytes = key_bytes(key).ok_or_else(|| AppError::new("bad_input", "tecla desconhecida"))?;
        if !direct {
            return fail("desktop_only", "teclas só para painel de IA comum; use o computador");
        }
        s.write_input(&row.id, bytes)?;
        return Ok(json!({"ok": true}));
    }
    let text = b.text.unwrap_or_default();
    if text.is_empty() {
        return fail("bad_input", "texto vazio");
    }
    crate::mcp::check_texts(&[&text])?;
    let enter = b.enter.unwrap_or(true);
    if direct {
        s.write_input(&row.id, &jarvis::write_bytes(&text, enter))?;
        return Ok(json!({"ok": true}));
    }
    let Some((sh, dev)) = confirm else {
        return fail("desktop_only", "este painel só recebe texto com confirmação no computador");
    };
    sh.confirm_slot(dev)?;
    jarvis::confirm(s, &row.workspace_id, json!({"kind": "write", "paneId": row.id, "paneName": jarvis::label(&row.name), "text": text, "enter": enter, "source": "celular", "device": sh.device_name(dev)}));
    Ok(json!({"status": "aguardando confirmação no computador"}))
}

async fn input(State(c): State<Ctx>, Extension(dev): Extension<DeviceId>, Path(id): Path<String>, Json(b): Json<InputBody>) -> Response {
    res(send_input(&c.state, &id, b, Some((c.as_ref(), dev.0.as_str()))))
}

async fn wake(State(c): State<Ctx>, Extension(dev): Extension<DeviceId>, Path(id): Path<String>) -> Response {
    if !c.allow(&dev.0, "wake", WAKE_PER_MIN) {
        return api_err(StatusCode::TOO_MANY_REQUESTS, "rate", "limite de 6 retomadas por minuto");
    }
    res((|| -> AppResult<Value> {
        let row = c.state.db.get_pane(&id)?;
        if row.closed_at.is_some() {
            return fail("not_found", "painel fechado");
        }
        c.state.wake(&row.id);
        Ok(json!({"ok": true}))
    })())
}

/// The screen as text without escapes, last 300 lines (a hibernated pane shows its frozen frame). A Claude pane
/// also gets its conversation as `items` (see `conversation_items`).
fn conversation_text(s: &AppState, id: &str) -> AppResult<Value> {
    let row = s.db.get_pane(id)?;
    let live = s.status_of(id);
    let raw = if live == Some(PaneStatus::Hibernated) {
        s.db.get_frame(id)?.unwrap_or_default()
    } else {
        String::from_utf8_lossy(&s.panes.snapshot(id)).into_owned()
    };
    let items = conversation_items(s, &row);
    let status = live.map(|x| x.as_str().to_string()).unwrap_or(row.status);
    Ok(json!({"status": status, "text": last_lines(&strip_ansi(&raw), CONVERSATION_LINES), "items": items}))
}

/// The last turns of a Claude pane's session, structured for the phone: tool calls keep only name and status.
/// Empty for other CLIs, remote panes, or while the session file is not there.
fn conversation_items(s: &AppState, row: &PaneRow) -> Vec<Value> {
    if row.cli.as_deref() != Some("claude") || crate::remote::is_remote(&row.cwd) {
        return vec![];
    }
    let Some(sid) = row.session_id.as_deref() else { return vec![] };
    let dir = s.config_dir_for("claude", row.account.as_deref());
    let Some(path) = crate::cost::claude::find_session_file(&dir, sid) else { return vec![] };
    let Ok(text) = crate::transcript::tail_text(&path, ITEMS_READ_BYTES) else { return vec![] };
    let items = crate::transcript::claude_items(&text);
    let skip = items.len().saturating_sub(CONVERSATION_ITEMS);
    items.into_iter().skip(skip).collect()
}

async fn conversation(State(c): State<Ctx>, Path(id): Path<String>) -> Response {
    res(conversation_text(&c.state, &id))
}

/// Closed agent conversations of a workspace that the phone can reopen.
async fn resumable(State(c): State<Ctx>, Path(ws): Path<String>) -> Response {
    res(c.state.db.resumable_panes(&ws).map(|rows| rows.iter().map(|p| json!({"id": p.id, "name": p.name, "cli": p.cli, "closedAt": p.closed_at})).collect::<Vec<_>>()))
}

/// Reopens a closed conversation with `session_resume` (in `pend`), under the spawn limit. The id names a pane of
/// this workspace list, never a raw session, and only a closed one is accepted.
async fn resume_closed(State(c): State<Ctx>, Extension(dev): Extension<DeviceId>, Path(id): Path<String>) -> Response {
    if !c.allow(&dev.0, "spawn", SPAWN_PER_MIN) {
        return api_err(StatusCode::TOO_MANY_REQUESTS, "rate", "limite de 3 aberturas por minuto");
    }
    res((|| -> AppResult<Value> {
        let row = c.state.db.get_pane(&id)?;
        if row.closed_at.is_none() || row.kind != "agent" {
            return fail("bad_state", "só conversas fechadas podem ser retomadas");
        }
        let (Some(cli), Some(session)) = (row.cli.clone(), row.session_id.clone()) else {
            return fail("bad_state", "esta conversa não tem sessão para retomar");
        };
        let p = crate::commands::resume_session(&c.state, row.workspace_id.clone(), cli, session)?;
        Ok(json!({"id": p.id, "name": p.name}))
    })())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MissionBody {
    workspace_id: String,
    name: Option<String>,
    /// "livre" or "squad".
    mode: String,
    squad_id: Option<String>,
    brief: String,
}

/// Proposes a mission: it opens only after the user confirms on the desktop (the same modal Jarvis uses).
fn propose_mission(s: &AppState, b: &MissionBody, device: &str) -> AppResult<Value> {
    s.db.get_workspace(&b.workspace_id)?;
    if !matches!(b.mode.as_str(), "livre" | "squad") {
        return fail("bad_input", "modo de missão desconhecido");
    }
    if b.brief.trim().is_empty() {
        return fail("bad_input", "falta o brief");
    }
    crate::mcp::check_texts(&[&b.brief])?;
    if b.mode == "squad" && b.squad_id.as_deref().and_then(|id| squads::get(&s.data, id)).is_none() {
        return fail("bad_squad", "squad desconhecida");
    }
    let name = b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Missão do celular");
    // the exact input of the existing missionCreate command, as Jarvis builds it
    let req = json!({"workspaceId": b.workspace_id, "name": name, "mode": b.mode, "worktree": false, "squadId": b.squad_id, "recipeId": Value::Null, "brief": b.brief});
    let excerpt: String = jarvis::strip_bidi(&b.brief).chars().take(200).collect();
    jarvis::confirm(s, &b.workspace_id, json!({"kind": "mission", "mode": b.mode, "name": jarvis::label(name), "briefExcerpt": excerpt, "input": req, "source": "celular", "device": device}));
    Ok(json!({"status": "aguardando confirmação no computador"}))
}

async fn missions_propose(State(c): State<Ctx>, Extension(dev): Extension<DeviceId>, Json(b): Json<MissionBody>) -> Response {
    let device = c.device_name(&dev.0);
    res(c.confirm_slot(&dev.0).and_then(|()| propose_mission(&c.state, &b, &device)))
}

async fn seen(State(c): State<Ctx>, Path(id): Path<String>) -> Response {
    res(c.state.db.mark_handoffs_seen(&id).map(|_| json!({"ok": true})))
}

async fn gates(State(c): State<Ctx>) -> Response {
    let r = c.state.db.open_gates(None).map(|list| {
        list.into_iter()
            .map(|g| {
                let options: Vec<String> = g.options_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
                json!({"id": g.id, "paneId": g.pane_id, "missionId": g.mission_id, "title": g.title, "summary": g.summary, "options": options})
            })
            .collect::<Vec<_>>()
    });
    res(r)
}

#[derive(Deserialize)]
struct DecisionBody {
    decision: String,
    note: Option<String>,
}

async fn answer_gate(State(c): State<Ctx>, Path(id): Path<String>, Json(b): Json<DecisionBody>) -> Response {
    if let Some(n) = &b.note {
        if let Err(e) = crate::mcp::check_texts(&[n]) {
            return app_err(e);
        }
    }
    res(squads::answer_gate(&c.state, &id, &b.decision, b.note.as_deref()))
}

/// Input over the WebSocket: `{"type":"input","data"}` for an AI pane only; anything else gets an error message.
fn ws_input(s: &AppState, id: &str, raw: &str) -> Option<String> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v["type"] != "input" {
        return None;
    }
    let data = v["data"].as_str()?.to_string();
    match send_input(s, id, InputBody { text: Some(data), key: None, enter: Some(false) }, None) {
        Ok(_) => None,
        Err(e) => Some(json!({"type": "error", "error": e.code()}).to_string()),
    }
}

async fn ws_pane(ws: WebSocketUpgrade, State(c): State<Ctx>, Path(id): Path<String>, ConnectInfo(ip): ConnectInfo<SocketAddr>) -> Response {
    // subscribed before the upgrade and the sign-in: a revoke from here on is never missed
    let revoked = c.revoked.subscribe();
    ws.max_message_size(WS_MSG_MAX).max_frame_size(WS_MSG_MAX).on_upgrade(move |socket| serve_pane(socket, c, id, ip.ip(), revoked))
}

fn close_frame(reason: &str) -> Message {
    if reason.is_empty() {
        Message::Close(None)
    } else {
        Message::Close(Some(CloseFrame { code: 1008, reason: reason.into() }))
    }
}

/// The token sent as the first WebSocket message (within `WS_AUTH_TIMEOUT`), if any.
async fn first_token(socket: &mut WebSocket) -> Option<String> {
    match tokio::time::timeout(WS_AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<Value>(t.as_str()).ok().and_then(|v| v["token"].as_str().map(str::to_string)),
        _ => None,
    }
}

/// Avisos de agente para o PWA aberto: um `{kind, paneId}` por quadro. Mesmo login e mesmos limites da conexão de
/// painel (o primeiro quadro é o token; uma conexão por aparelho e o total do servidor).
async fn ws_notices(ws: WebSocketUpgrade, State(c): State<Ctx>, ConnectInfo(ip): ConnectInfo<SocketAddr>) -> Response {
    let notices = c.state.notices.subscribe();
    let revoked = c.revoked.subscribe();
    ws.max_message_size(WS_MSG_MAX).max_frame_size(WS_MSG_MAX).on_upgrade(move |socket| serve_notices(socket, c, ip.ip(), notices, revoked))
}

async fn serve_notices(mut socket: WebSocket, c: Ctx, ip: IpAddr, mut notices: broadcast::Receiver<String>, mut revoked: broadcast::Receiver<String>) {
    let dev = match first_token(&mut socket).await.map(|t| c.sign_in(&t, ip)) {
        Some(Ok(d)) => d,
        _ => {
            let _ = socket.send(close_frame("não autorizado")).await;
            return;
        }
    };
    let Some(_slot) = take_ws_slot(&c, &dev) else {
        let _ = socket.send(close_frame("limite de conexões")).await;
        return;
    };
    let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + WS_RECHECK, WS_RECHECK);
    let reason: String = loop {
        tokio::select! {
            n = notices.recv() => match n {
                Ok(text) => {
                    // celular que não lê não pode prender a conexão (e o aviso seguinte): corta após 5 s
                    let sent = tokio::time::timeout(WS_SEND_TIMEOUT, socket.send(Message::Text(text.into()))).await;
                    if !matches!(sent, Ok(Ok(()))) {
                        break String::new();
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break String::new(),
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break String::new(),
                Some(Ok(_)) => {}
            },
            r = revoked.recv() => match r {
                Ok(who) if who == "*" || who == dev => break "aparelho revogado".to_string(),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => break "revogação não confirmada; reconecte".to_string(),
                Err(broadcast::error::RecvError::Closed) => break String::new(),
            },
            _ = recheck.tick() => {
                if !c.device_ok(&dev) {
                    break "aparelho revogado".to_string();
                }
            }
        }
    };
    let _ = socket.send(close_frame(&reason)).await;
}

/// Output of a pane (ring replay, then live) as binary frames. The first message must be the device token:
/// browsers cannot send headers on a WebSocket, and the token stays out of the URL.
async fn serve_pane(mut socket: WebSocket, c: Ctx, id: String, ip: IpAddr, mut revoked: broadcast::Receiver<String>) {
    let token = first_token(&mut socket).await;
    let dev = match token.map(|t| c.sign_in(&t, ip)) {
        Some(Ok(d)) => d,
        _ => {
            let _ = socket.send(close_frame("não autorizado")).await;
            return;
        }
    };
    let Some(_slot) = take_ws_slot(&c, &dev) else {
        let _ = socket.send(close_frame("limite de conexões")).await;
        return;
    };
    let s = c.state.clone();
    if !s.db.get_pane(&id).is_ok_and(|r| r.closed_at.is_none()) {
        let _ = socket.send(close_frame("painel não encontrado")).await;
        return;
    }
    // bounded: a phone that cannot keep up loses its sink (and the socket closes with a reason)
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(WS_OUT_BUFFER);
    let overflow = Arc::new(AtomicBool::new(false));
    let full = overflow.clone();
    let sink = Box::new(move |d: &[u8]| match tx.try_send(d.to_vec()) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            full.store(true, Ordering::Relaxed);
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    });
    let sid = match s.attach(&id, &format!("celular:{dev}"), sink) {
        Ok(sid) => sid,
        Err(_) => {
            let _ = socket.send(close_frame("painel não está rodando")).await;
            return;
        }
    };
    let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + WS_RECHECK, WS_RECHECK);
    let reason: String = loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(bytes) => {
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        break String::new();
                    }
                }
                None => break if overflow.load(Ordering::Relaxed) { "saída atrasada demais".to_string() } else { String::new() },
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if !c.device_ok(&dev) {
                        break "aparelho revogado".to_string();
                    }
                    if !c.allow(&dev, "ws_in", WS_INPUT_PER_MIN) {
                        let err = json!({"type": "error", "error": "rate"}).to_string();
                        if socket.send(Message::Text(err.into())).await.is_err() {
                            break String::new();
                        }
                        continue;
                    }
                    if let Some(err) = ws_input(&s, &id, t.as_str()) {
                        if socket.send(Message::Text(err.into())).await.is_err() {
                            break String::new();
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break String::new(),
                Some(Ok(_)) => {}
            },
            r = revoked.recv() => match r {
                Ok(who) if who == "*" || who == dev => break "aparelho revogado".to_string(),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => break "revogação não confirmada; reconecte".to_string(),
                Err(broadcast::error::RecvError::Closed) => break String::new(),
            },
            _ = recheck.tick() => {
                if !c.device_ok(&dev) {
                    break "aparelho revogado".to_string();
                }
            }
        }
    };
    s.panes.detach(&id, sid);
    let _ = socket.send(close_frame(&reason)).await;
}

/// The PWA page and its assets, and nothing else: any other path answers 404.
async fn static_file(State(c): State<Ctx>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let key = if path.is_empty() { "mobile.html" } else { path };
    let allowed = matches!(key, "mobile.html" | "manifest.webmanifest" | "mobile-icon.svg") || (key.starts_with("assets/") && !key.contains(".."));
    if !allowed {
        return StatusCode::NOT_FOUND.into_response();
    }
    match (c.assets)(key) {
        Some((bytes, mime)) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The whole server: pairing and static files are open; the API needs a device token; the WebSocket authenticates
/// with its first message.
pub fn router(c: Ctx) -> Router {
    let api = Router::new()
        .route("/api/workspaces", get(workspaces))
        .route("/api/workspaces/{id}/missions", get(missions))
        .route("/api/workspaces/{id}/panes", get(panes))
        .route("/api/workspaces/{id}/resumable", get(resumable))
        .route("/api/panes/{id}/resume", post(resume_closed))
        .route("/api/panes", post(spawn))
        .route("/api/panes/{id}/input", post(input))
        .route("/api/panes/{id}/wake", post(wake))
        .route("/api/panes/{id}/conversation", get(conversation))
        .route("/api/missions", post(missions_propose))
        .route("/api/missions/{id}/seen", post(seen))
        .route("/api/gates", get(gates))
        .route("/api/gates/{id}", post(answer_gate))
        .route_layer(middleware::from_fn_with_state(c.clone(), auth));
    Router::new()
        .route("/api/pair", post(pair))
        .merge(api)
        .route("/ws/panes/{id}", get(ws_pane))
        .route("/ws/notices", get(ws_notices))
        .fallback(static_file)
        .layer(DefaultBodyLimit::max(BODY_MAX))
        .layer(middleware::from_fn_with_state(c.clone(), guard))
        .with_state(c)
}

/// The DER of the first certificate of a PEM text (for its fingerprint).
fn pem_der(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// `AA:BB:..` SHA-256 of the certificate, as the pairing screen shows it.
fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// The self-signed certificate of `data/mobile/` (created once, then reused): for localhost and the LAN address.
async fn tls_config(data: &FsPath, lan: Option<IpAddr>) -> AppResult<(RustlsConfig, String)> {
    let dir = data.join("mobile");
    std::fs::create_dir_all(&dir)?;
    let (cert_path, key_path, lan_path) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("lan.txt"));
    // the stored certificate is reused only while the LAN address is still the one in its names
    let lan_now = lan.map_or(String::new(), |ip| ip.to_string());
    let same_lan = std::fs::read_to_string(&lan_path).ok().as_deref() == Some(lan_now.as_str());
    let stored = if same_lan { std::fs::read_to_string(&cert_path).ok().zip(std::fs::read_to_string(&key_path).ok()) } else { None };
    let (cert_pem, key_pem) = match stored {
        Some(pair) if pem_der(&pair.0).is_some() => pair,
        _ => {
            let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
            names.extend(lan.map(|ip| ip.to_string()));
            let ck = rcgen::generate_simple_self_signed(names).map_err(|e| AppError::new("tls", e.to_string()))?;
            let pair = (ck.cert.pem(), ck.signing_key.serialize_pem());
            std::fs::write(&cert_path, &pair.0)?;
            std::fs::write(&key_path, &pair.1)?;
            std::fs::write(&lan_path, &lan_now)?;
            pair
        }
    };
    let der = pem_der(&cert_pem).ok_or_else(|| AppError::new("tls", "certificado inválido"))?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = RustlsConfig::from_pem(cert_pem.into_bytes(), key_pem.into_bytes()).await.map_err(|e| AppError::new("tls", e.to_string()))?;
    Ok((cfg, fingerprint(&der)))
}

struct Running {
    ctx: Ctx,
    handle: axum_server::Handle<SocketAddr>,
    url: String,
    tls: bool,
    fingerprint: Option<String>,
    /// Tells this run apart from a later one, so a dying old server cannot clear a new run.
    run_id: u64,
}

static RUN_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Serves in the background. When the server stops (stop or a crash), its run is cleared and the desktop is told.
fn serve_in_background<F>(run: Arc<Mutex<Option<Running>>>, run_id: u64, state: Arc<AppState>, fut: F)
where
    F: std::future::Future<Output = std::io::Result<()>> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        if let Err(e) = fut.await {
            tracing::error!("servidor do celular parou: {e}");
        }
        let cleared = {
            let mut r = lk(&run);
            let mine = r.as_ref().is_some_and(|x| x.run_id == run_id);
            if mine {
                *r = None;
            }
            mine
        };
        if cleared {
            state.emit("mobile-status", json!({"running": false}));
        }
    });
}

/// The mobile server as managed by Tauri. Off until the user starts it; stopped on exit.
#[derive(Default)]
pub struct Mobile {
    run: Arc<Mutex<Option<Running>>>,
}

impl Mobile {
    /// Binds `0.0.0.0:8789` (the port the phones use) and serves, with TLS. `http_only` binds 127.0.0.1 only.
    pub async fn start(&self, state: Arc<AppState>, assets: Assets, http_only: bool) -> AppResult<()> {
        let ip = if http_only { Ipv4Addr::LOCALHOST } else { Ipv4Addr::UNSPECIFIED };
        self.start_at(state, assets, http_only, SocketAddr::from((ip, PORT))).await
    }

    async fn start_at(&self, state: Arc<AppState>, assets: Assets, http_only: bool, addr: SocketAddr) -> AppResult<()> {
        if lk(&self.run).is_some() {
            return Ok(());
        }
        let listener = std::net::TcpListener::bind(addr).map_err(|e| AppError::new("bind_failed", format!("não foi possível abrir a porta {}: {e}", addr.port())))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let lan = lan_ip();
        let (revoked, _) = broadcast::channel(16);
        let data = state.data.clone();
        let ctx: Ctx = Arc::new(Shared {
            state,
            port,
            tls: !http_only,
            pairing: Mutex::new(Pairing::new()?),
            limits: Mutex::default(),
            revoked,
            assets,
            ws_open: Mutex::default(),
            confirm_pending: Mutex::default(),
            req_slots: Arc::new(tokio::sync::Semaphore::new(REQ_SLOTS)),
        });
        let app = router(ctx.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let handle = axum_server::Handle::<SocketAddr>::new();
        let url = base_url(http_only, lan, port);
        let run_id = RUN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let fp = if http_only {
            let mut server = axum_server::from_tcp(listener)?.handle(handle.clone());
            server.http_builder().http1().timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT);
            serve_in_background(self.run.clone(), run_id, ctx.state.clone(), server.serve(app));
            None
        } else {
            let (cfg, fp) = tls_config(&data, lan).await?;
            let mut server = from_tcp_rustls(listener, cfg)?.handle(handle.clone());
            server.http_builder().http1().timer(hyper_util::rt::TokioTimer::new()).header_read_timeout(HEADER_TIMEOUT);
            serve_in_background(self.run.clone(), run_id, ctx.state.clone(), server.serve(app));
            Some(fp)
        };
        *lk(&self.run) = Some(Running { ctx, handle, url, tls: !http_only, fingerprint: fp, run_id });
        tracing::info!("celular: servidor ligado na porta {port}");
        Ok(())
    }

    /// Stops the server and drops every phone's WebSocket. Pairing tokens die with the run.
    pub fn stop(&self) {
        if let Some(r) = lk(&self.run).take() {
            let _ = r.ctx.revoked.send("*".into());
            r.handle.graceful_shutdown(Some(Duration::from_secs(2)));
            tracing::info!("celular: servidor desligado");
        }
    }

    /// `{running, url, httpOnly, fingerprint, code, devices, lanIp}`; the code is null unless shown; the devices
    /// are listed even when off.
    pub fn status(&self, state: &AppState) -> AppResult<Value> {
        let devices: Vec<MobileDevice> = state.db.mobile_devices()?;
        let lan = lan_ip().map(|ip| ip.to_string());
        let run = lk(&self.run);
        let Some(r) = run.as_ref() else {
            return Ok(json!({"running": false, "devices": devices, "port": PORT, "lanIp": lan}));
        };
        let code = lk(&r.ctx.pairing).code_now(now_ms());
        Ok(json!({"running": true, "url": r.url, "httpOnly": !r.tls, "fingerprint": r.fingerprint, "code": code, "devices": devices, "port": PORT, "lanIp": lan}))
    }

    /// Shows a code on the desktop for 5 minutes. Until then the code is inactive.
    pub fn show_code(&self) -> AppResult<String> {
        let run = lk(&self.run);
        let r = run.as_ref().ok_or_else(|| AppError::new("not_running", "o celular está desligado"))?;
        let code = lk(&r.ctx.pairing).show_code(now_ms());
        Ok(code)
    }

    /// A new pairing token (10 min, single use) as a QR code: an SVG of `https://<lan>:8789/#pair=<token>`.
    pub fn pair_qr(&self) -> AppResult<String> {
        let run = lk(&self.run);
        let r = run.as_ref().ok_or_else(|| AppError::new("not_running", "o celular está desligado"))?;
        let token = lk(&r.ctx.pairing).issue_token(now_ms())?;
        let url = format!("{}/#pair={token}", r.url);
        let code = qrcode::QrCode::new(url.as_bytes()).map_err(|e| AppError::new("qr", e.to_string()))?;
        Ok(code.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).build())
    }

    /// Approves a device paired by code (`approve`), or refuses it (it is revoked).
    pub fn decide(&self, state: &AppState, id: &str, approve: bool) -> AppResult<()> {
        if approve {
            state.db.mobile_approve(id)
        } else {
            self.revoke(state, id)
        }
    }

    /// Revokes one device (its id), or all with `"all"`; its open WebSockets close.
    pub fn revoke(&self, state: &AppState, id: &str) -> AppResult<()> {
        state.db.mobile_revoke(id)?;
        if let Some(r) = lk(&self.run).as_ref() {
            let _ = r.ctx.revoked.send(if id == "all" { "*".to_string() } else { id.to_string() });
        }
        Ok(())
    }
}

#[tauri::command]
pub fn mobile_status(s: TState<'_, Arc<AppState>>, m: TState<'_, Mobile>) -> AppResult<Value> {
    m.status(&s)
}

#[tauri::command]
pub async fn mobile_start(app: AppHandle, s: TState<'_, Arc<AppState>>, m: TState<'_, Mobile>, http_only: bool) -> AppResult<Value> {
    let resolver = app.asset_resolver();
    let assets: Assets = Arc::new(move |p: &str| resolver.get(p.to_string()).map(|a| (a.bytes, a.mime_type)));
    m.start(s.inner().clone(), assets, http_only).await?;
    m.status(&s)
}

#[tauri::command]
pub fn mobile_stop(m: TState<'_, Mobile>) {
    m.stop();
}

#[tauri::command]
pub fn mobile_show_code(m: TState<'_, Mobile>) -> AppResult<String> {
    m.show_code()
}

#[tauri::command]
pub fn mobile_pair_qr(m: TState<'_, Mobile>) -> AppResult<String> {
    m.pair_qr()
}

#[tauri::command]
pub fn mobile_revoke(s: TState<'_, Arc<AppState>>, m: TState<'_, Mobile>, id: String) -> AppResult<()> {
    m.revoke(&s, &id)
}

#[tauri::command]
pub fn mobile_approve(s: TState<'_, Arc<AppState>>, m: TState<'_, Mobile>, id: String, approve: bool) -> AppResult<()> {
    m.decide(&s, &id, approve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn state(dir: &std::path::Path) -> Arc<AppState> {
        AppState::new(dir.into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap()
    }

    fn assets() -> Assets {
        Arc::new(|p: &str| (p == "mobile.html").then(|| (b"<!doctype html><title>Regente</title>".to_vec(), "text/html".to_string())))
    }

    const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));

    #[test]
    fn token_hash_verifies_only_the_same_token() {
        let t = to_hex(&[7u8; 32]);
        let h = token_hash(&t);
        assert_ne!(h, t);
        assert!(verify_token(&h, &t));
        assert!(!verify_token(&h, &to_hex(&[8u8; 32])));
    }

    #[test]
    fn pairing_token_is_single_use_and_expires() {
        let mut p = Pairing::with_secret([1; 32]);
        let t = p.issue_token(0).unwrap();
        assert!(!p.take_token("errado", 1), "a wrong guess does not spend the token");
        assert!(p.take_token(&t, 1));
        assert!(!p.take_token(&t, 2), "single use");
        let t2 = p.issue_token(0).unwrap();
        assert!(!p.take_token(&t2, PAIR_TTL_MS), "expired at 10 min");
    }

    #[test]
    fn code_is_inactive_until_shown() {
        let mut p = Pairing::with_secret([3; 32]);
        assert!(!p.check_code("000000", 0));
        assert!(p.code_now(0).is_none());
        let c = p.show_code(0);
        assert_eq!(p.code_now(1), Some(c.clone()));
        assert_eq!(c.len(), 6);
    }

    #[test]
    fn code_works_five_minutes_and_only_once() {
        let mut p = Pairing::with_secret([4; 32]);
        let c = p.show_code(0);
        assert!(!p.check_code(&c, CODE_TTL_MS), "expired at 5 min");
        let c = p.show_code(0);
        assert!(p.check_code(&c, 1));
        assert!(!p.check_code(&c, 2), "burned by the success");
        assert!(p.code_now(2).is_none());
    }

    #[test]
    fn code_rotates_after_five_failures() {
        let mut p = Pairing::with_secret([5; 32]);
        let old = p.show_code(0);
        for _ in 0..CODE_FAILS_ROTATE - 1 {
            assert!(!p.check_code("999999x", 1));
        }
        assert_eq!(p.code_now(1), Some(old.clone()), "still the same before the 5th failure");
        assert!(!p.check_code("999999x", 1));
        let new = p.code_now(1).unwrap();
        assert_ne!(new, old, "the 5th failure rotates the code");
        assert!(!p.check_code(&old, 2), "the old code is dead");
        assert!(p.check_code(&new, 2), "the new code works until its deadline");
    }

    #[test]
    fn failed_sign_ins_lock_the_ip_for_a_minute() {
        let mut l = Limits::default();
        for _ in 0..9 {
            l.failed(IP, 0);
        }
        assert!(!l.locked(IP, 0));
        l.failed(IP, 0);
        assert!(l.locked(IP, 0));
        assert!(l.locked(IP, LOCK_MS - 1));
        assert!(!l.locked(IP, LOCK_MS));
    }

    #[test]
    fn global_pairing_lock_after_thirty_failures() {
        let mut l = Limits::default();
        for _ in 0..GLOBAL_PAIR_FAILS - 1 {
            l.pair_failed(0);
        }
        assert!(!l.pair_locked(0));
        l.pair_failed(0);
        assert!(l.pair_locked(0), "the 30th failure locks pairing for everyone");
        assert!(l.pair_locked(GLOBAL_LOCK_MS - 1));
        assert!(!l.pair_locked(GLOBAL_LOCK_MS));
    }

    #[test]
    fn spawn_budget_is_three_a_minute_per_device() {
        let mut l = Limits::default();
        assert!(l.allow("dev", "spawn", 3, 60_000, 0));
        assert!(l.allow("dev", "spawn", 3, 60_000, 1));
        assert!(l.allow("dev", "spawn", 3, 60_000, 2));
        assert!(!l.allow("dev", "spawn", 3, 60_000, 3), "the 4th inside the minute is refused");
        assert!(l.allow("other", "spawn", 3, 60_000, 3), "budgets are per device");
        assert!(l.allow("dev", "spawn", 3, 60_000, 60_000), "the window slides");
    }

    #[test]
    fn host_allow_list_refuses_names_and_public_addresses() {
        assert!(host_allowed("127.0.0.1:8789", 8789));
        // o modo "Apenas HTTP" é só deste computador: nada de IP da rede
        let lan = Some(IpAddr::from([192, 168, 0, 7]));
        assert_eq!(base_url(true, lan, 8789), "http://127.0.0.1:8789");
        assert_eq!(base_url(false, lan, 8789), "https://192.168.0.7:8789");
        assert!(host_allowed("localhost:8789", 8789));
        assert!(host_allowed("192.168.1.20:8789", 8789));
        assert!(host_allowed("10.0.0.5:8789", 8789));
        assert!(!host_allowed("192.168.1.20:9999", 8789), "wrong port");
        assert!(!host_allowed("evil.example:8789", 8789), "DNS names (rebinding)");
        assert!(!host_allowed("8.8.8.8:8789", 8789), "public address");
        assert!(!host_allowed("192.168.1.20", 8789), "no port");
    }

    #[test]
    fn host_comes_from_the_header_or_the_h2_authority() {
        // HTTP/1.1: Host header
        let h1 = Request::builder().uri("/x").header("host", "127.0.0.1:8789").body(axum::body::Body::empty()).unwrap();
        assert_eq!(request_host(&h1), "127.0.0.1:8789");
        // HTTP/2 (browsers over TLS): no Host header, the :authority is the URI authority
        let h2 = Request::builder().uri("https://127.0.0.1:8789/x").body(axum::body::Body::empty()).unwrap();
        assert_eq!(request_host(&h2), "127.0.0.1:8789");
        assert!(host_allowed(&request_host(&h2), 8789));
        let none = Request::builder().uri("/x").body(axum::body::Body::empty()).unwrap();
        assert_eq!(request_host(&none), "", "no host at all is refused by host_allowed");
    }

    #[test]
    fn peer_classifier_accepts_this_machine_and_the_lan_only() {
        let ok = ["127.0.0.1", "192.168.1.5", "10.1.2.3", "172.20.0.1", "100.100.1.1", "::1", "fe80::1", "fd00::1", "::ffff:192.168.0.2"];
        for ip in ok {
            assert!(peer_allowed(ip.parse().unwrap()), "{ip} should be allowed");
        }
        let no = ["8.8.8.8", "2001:db8::1", "::ffff:8.8.8.8", "1.1.1.1"];
        for ip in no {
            assert!(!peer_allowed(ip.parse().unwrap()), "{ip} should be refused");
        }
    }

    #[test]
    fn phone_input_is_direct_only_to_plain_agent_panes() {
        assert!(input_is_direct("agent", "pend", false));
        assert!(input_is_direct("agent", "auto", false));
        assert!(!input_is_direct("agent", "danger", false));
        assert!(!input_is_direct("terminal", "pend", false));
        assert!(!input_is_direct("agent", "pend", true), "a pane asking a question");
        assert!(key_bytes("ctrl-c").is_some());
        assert!(key_bytes("ESC[2J").is_none(), "free-form escapes are not keys");
    }

    #[test]
    fn device_names_are_clean_and_short() {
        assert_eq!(device_name(None), "Celular");
        assert_eq!(device_name(Some("  \u{7}Pixel\n ")), "Pixel");
        assert_eq!(device_name(Some("x".repeat(80).as_str())).chars().count(), 40);
    }

    #[test]
    fn websocket_slots_cap_per_device_and_in_total() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let (revoked, _) = broadcast::channel(4);
        let ctx: Ctx = Arc::new(Shared {
            state: st,
            port: 1,
            tls: false,
            pairing: Mutex::new(Pairing::with_secret([2; 32])),
            limits: Mutex::default(),
            revoked,
            assets: assets(),
            ws_open: Mutex::default(),
            confirm_pending: Mutex::default(),
            req_slots: Arc::new(tokio::sync::Semaphore::new(REQ_SLOTS)),
        });
        let mut held = Vec::new();
        for _ in 0..WS_PER_DEVICE {
            held.push(take_ws_slot(&ctx, "a").expect("up to 4 per device"));
        }
        assert!(take_ws_slot(&ctx, "a").is_none(), "the 5th socket of one device is refused");
        for i in 0..(WS_TOTAL - WS_PER_DEVICE) {
            held.push(take_ws_slot(&ctx, &format!("b{}", i / WS_PER_DEVICE)).expect("up to 16 in total"));
        }
        assert!(take_ws_slot(&ctx, "c").is_none(), "16 in total, whatever the device");
        held.clear();
        assert!(take_ws_slot(&ctx, "c").is_some(), "dropping a socket frees its slot");
    }

    /// Serves on 127.0.0.1 with a free port (HTTP, no TLS in tests); returns (mobile, state, base url).
    async fn serve(dir: &std::path::Path) -> (Mobile, Arc<AppState>, String) {
        let st = state(dir);
        let m = Mobile::default();
        m.start_at(st.clone(), assets(), true, SocketAddr::from(([127, 0, 0, 1], 0))).await.unwrap();
        let port = lk(&m.run).as_ref().unwrap().ctx.port;
        (m, st, format!("http://127.0.0.1:{port}"))
    }

    fn issue(m: &Mobile) -> String {
        let run = lk(&m.run);
        let ctx = &run.as_ref().unwrap().ctx;
        lk(&ctx.pairing).issue_token(now_ms()).unwrap()
    }

    #[tokio::test]
    async fn api_without_token_is_401_and_static_page_has_security_headers() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        let r = c.get(format!("{base}/api/workspaces")).send().await.unwrap();
        assert_eq!(r.status(), 401);
        let r = c.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()["x-content-type-options"], "nosniff");
        assert_eq!(r.headers()["x-frame-options"], "DENY");
        assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("default-src 'none'"));
        assert!(r.headers().get("access-control-allow-origin").is_none(), "no CORS: same origin only");
        let r = c.get(format!("{base}/api/nope")).send().await.unwrap();
        assert_eq!(r.status(), 404);
        m.stop();
    }

    #[tokio::test]
    async fn cross_origin_requests_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        let r = c.get(format!("{base}/")).header("origin", "https://evil.example").send().await.unwrap();
        assert_eq!(r.status(), 403);
        let r = c.get(format!("{base}/")).header("origin", base.replace("127.0.0.1", "127.0.0.1")).send().await.unwrap();
        assert_eq!(r.status(), 200);
        m.stop();
    }

    #[tokio::test]
    async fn pairing_then_api_then_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let (m, st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        let token = issue(&m);
        let r = c.post(format!("{base}/api/pair")).json(&json!({"token": token, "deviceName": "Meu celular"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let body: Value = r.json().await.unwrap();
        let device = body["deviceToken"].as_str().unwrap().to_string();
        assert_eq!(body["name"], "Meu celular");
        assert_eq!(body["pending"], false, "the QR is trusted: no approval needed");
        // the token is single use
        let again = c.post(format!("{base}/api/pair")).json(&json!({"token": token})).send().await.unwrap();
        assert_eq!(again.status(), 401);
        // the device token opens the API
        let r = c.get(format!("{base}/api/workspaces")).bearer_auth(&device).send().await.unwrap();
        assert_eq!(r.status(), 200);
        // the stored value is the hash, not the token
        let stored = st.db.mobile_active_hashes().unwrap();
        assert_eq!(stored.len(), 1);
        assert_ne!(stored[0].1, device);
        assert!(verify_token(&stored[0].1, &device));
        // revoked: the same token stops working
        let id = body["deviceId"].as_str().unwrap();
        m.revoke(&st, id).unwrap();
        let r = c.get(format!("{base}/api/workspaces")).bearer_auth(&device).send().await.unwrap();
        assert_eq!(r.status(), 401);
        m.stop();
    }

    #[tokio::test]
    async fn code_pairing_waits_for_desktop_approval() {
        let dir = tempfile::tempdir().unwrap();
        let (m, st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        let code = m.show_code().unwrap();
        let r = c.post(format!("{base}/api/pair")).json(&json!({"code": code, "deviceName": "Outro"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["pending"], true);
        let device = body["deviceToken"].as_str().unwrap().to_string();
        let id = body["deviceId"].as_str().unwrap().to_string();
        // a pending device is refused until the desktop approves it
        let r = c.get(format!("{base}/api/workspaces")).bearer_auth(&device).send().await.unwrap();
        assert_eq!(r.status(), 403);
        let err: Value = r.json().await.unwrap();
        assert_eq!(err["error"], "pending_approval");
        m.decide(&st, &id, true).unwrap();
        let r = c.get(format!("{base}/api/workspaces")).bearer_auth(&device).send().await.unwrap();
        assert_eq!(r.status(), 200);
        // the code burned on success
        let r = c.post(format!("{base}/api/pair")).json(&json!({"code": code})).send().await.unwrap();
        assert_eq!(r.status(), 401);
        m.stop();
    }

    #[tokio::test]
    async fn wrong_codes_lock_the_ip_even_for_the_right_one() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        let code = m.show_code().unwrap();
        for _ in 0..MAX_FAILS {
            let r = c.post(format!("{base}/api/pair")).json(&json!({"code": "000000x"})).send().await.unwrap();
            assert_eq!(r.status(), 401);
        }
        let r = c.post(format!("{base}/api/pair")).json(&json!({"code": code})).send().await.unwrap();
        assert_eq!(r.status(), 429, "locked for a minute");
        m.stop();
    }

    #[tokio::test]
    async fn websocket_without_device_token_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _st, base) = serve(dir.path()).await;
        // a plain GET on the WebSocket route without the upgrade headers is not a valid upgrade
        let r = reqwest::Client::new().get(format!("{base}/ws/panes/abc")).send().await.unwrap();
        assert_ne!(r.status(), 101);
        m.stop();
    }

    #[test]
    fn stop_is_a_no_op_when_off_and_status_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let m = Mobile::default();
        m.stop();
        let v = m.status(&st).unwrap();
        assert_eq!(v["running"], false);
        assert!(m.pair_qr().is_err(), "no token while off");
        assert!(m.show_code().is_err(), "no code while off");
    }

    type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    /// A device paired by QR token (approved at once): (device token, device id).
    async fn pair_device(m: &Mobile, base: &str) -> (String, String) {
        let token = issue(m);
        let body: Value = reqwest::Client::new().post(format!("{base}/api/pair")).json(&json!({"token": token})).send().await.unwrap().json().await.unwrap();
        (body["deviceToken"].as_str().unwrap().to_string(), body["deviceId"].as_str().unwrap().to_string())
    }

    /// A pane that really runs (`cmd` in a ConPTY), so a WebSocket can attach to it.
    fn live_pane(st: &AppState) -> String {
        let path = st.data.to_string_lossy().to_string();
        let ws = st.db.upsert_workspace(&path).unwrap();
        let id = new_id();
        st.db
            .insert_pane(&crate::db::PaneRow { id: id.clone(), workspace_id: ws.id, kind: "agent".into(), name: "teste".into(), permission: "pend".into(), cwd: path.clone(), status: "iniciando".into(), created_at: now_ms(), ..Default::default() })
            .unwrap();
        st.panes.spawn(&id, &crate::pty::SpawnPlan { program: "cmd.exe".into(), args: vec!["/Q".into()], cwd: path, ..Default::default() }, 80, 24).unwrap();
        id
    }

    fn ws_url(base: &str, pane: &str) -> String {
        format!("{}/ws/panes/{pane}", base.replacen("http://", "ws://", 1))
    }

    async fn auth_ws(ws: &mut WsStream, token: &str) {
        use futures_util::SinkExt;
        let msg = json!({"type": "auth", "token": token }).to_string();
        ws.send(tokio_tungstenite::tungstenite::Message::Text(msg.into())).await.unwrap();
    }

    #[tokio::test]
    async fn websocket_without_a_valid_token_gets_no_output() {
        use futures_util::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let (m, st, base) = serve(dir.path()).await;
        let pane = live_pane(&st);
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(&base, &pane)).await.unwrap();
        auth_ws(&mut ws, "nao-e-um-token").await;
        let got = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
        assert!(!matches!(got, Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(_)))), "no replay for a bad token");
        let _ = st.panes.kill(&pane);
        m.stop();
    }

    #[tokio::test]
    async fn websocket_replays_then_closes_when_the_device_is_revoked() {
        use futures_util::StreamExt;
        use tokio_tungstenite::tungstenite::Message as WsMsg;
        let dir = tempfile::tempdir().unwrap();
        let (m, st, base) = serve(dir.path()).await;
        let pane = live_pane(&st);
        let (token, dev) = pair_device(&m, &base).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url(&base, &pane)).await.unwrap();
        auth_ws(&mut ws, &token).await;
        let first = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap();
        assert!(matches!(first, Some(Ok(WsMsg::Binary(_)))), "the replay comes first");
        m.revoke(&st, &dev).unwrap();
        let end = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    Some(Ok(WsMsg::Binary(_))) => continue,
                    other => break other,
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(end, None | Some(Ok(WsMsg::Close(_))) | Some(Err(_))), "revoking closes the live socket at once");
        let _ = st.panes.kill(&pane);
        m.stop();
    }

    #[tokio::test]
    async fn websocket_from_a_foreign_origin_is_refused() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let dir = tempfile::tempdir().unwrap();
        let (m, st, base) = serve(dir.path()).await;
        let pane = live_pane(&st);
        let mut req = ws_url(&base, &pane).into_client_request().unwrap();
        req.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
        assert!(tokio_tungstenite::connect_async(req).await.is_err(), "the handshake is refused");
        let _ = st.panes.kill(&pane);
        m.stop();
    }

    #[tokio::test]
    async fn phone_reopens_only_closed_conversations_of_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let (m, st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        assert_eq!(c.get(format!("{base}/api/workspaces/x/resumable")).send().await.unwrap().status(), 401);
        assert_eq!(c.post(format!("{base}/api/panes/x/resume")).send().await.unwrap().status(), 401);
        let (token, _) = pair_device(&m, &base).await;
        let ws = st.db.upsert_workspace("C:\\proj").unwrap();
        let open = PaneRow {
            id: "p-open".into(),
            workspace_id: ws.id.clone(),
            kind: "agent".into(),
            name: "aberto".into(),
            permission: "pend".into(),
            cwd: "C:\\proj".into(),
            status: "ocioso".into(),
            cli: Some("claude".into()),
            session_id: Some("s1".into()),
            created_at: 1,
            ..Default::default()
        };
        st.db.insert_pane(&open).unwrap();
        // an open pane is not a conversation to reopen
        assert_eq!(c.post(format!("{base}/api/panes/p-open/resume")).bearer_auth(&token).send().await.unwrap().status(), 409);
        // a closed pane with no session has nothing to resume
        let nosess = PaneRow { id: "p-nos".into(), session_id: None, ..open.clone() };
        st.db.insert_pane(&nosess).unwrap();
        st.db.close_pane("p-nos").unwrap();
        assert_eq!(c.post(format!("{base}/api/panes/p-nos/resume")).bearer_auth(&token).send().await.unwrap().status(), 409);
        // once closed, the conversation is on the list (the one without a session is not)
        st.db.close_pane("p-open").unwrap();
        let list: Value = c.get(format!("{base}/api/workspaces/{}/resumable", ws.id)).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
        let ids: Vec<&str> = list.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["p-open"]);
        m.stop();
    }

    #[tokio::test]
    async fn confirms_are_rate_limited_and_one_is_pending_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _st, base) = serve(dir.path()).await;
        let (token, _) = pair_device(&m, &base).await;
        let c = reqwest::Client::new();
        let body = json!({"workspaceId": "nao-existe", "mode": "livre", "brief": "x"});
        let first = c.post(format!("{base}/api/missions")).bearer_auth(&token).json(&body).send().await.unwrap();
        assert_ne!(first.status(), 429, "the first request gets through to validation");
        let second = c.post(format!("{base}/api/missions")).bearer_auth(&token).json(&body).send().await.unwrap();
        assert_eq!(second.status(), 429, "a second confirm while one is pending is dropped");
        m.stop();
    }

    #[tokio::test]
    async fn only_the_pwa_files_are_served() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _st, base) = serve(dir.path()).await;
        let c = reqwest::Client::new();
        assert_eq!(c.get(format!("{base}/mobile.html")).send().await.unwrap().status(), 200);
        assert_eq!(c.get(format!("{base}/cert.pem")).send().await.unwrap().status(), 404);
        assert_eq!(c.get(format!("{base}/mobile/cert.pem")).send().await.unwrap().status(), 404);
        assert_eq!(c.get(format!("{base}/assets/..%2Fx.js")).send().await.unwrap().status(), 404);
        let r = c.get(format!("{base}/")).send().await.unwrap();
        assert_eq!(r.headers()["permissions-policy"], "camera=(), microphone=(), geolocation=()");
        assert_eq!(r.headers()["cross-origin-resource-policy"], "same-origin");
        assert_eq!(r.headers()["cross-origin-opener-policy"], "same-origin");
        m.stop();
    }

    #[test]
    fn bidi_marks_are_stripped_from_confirm_texts() {
        assert_eq!(crate::jarvis::strip_bidi("ab\u{202E}cd\u{2067}"), "abcd");
        assert_eq!(crate::jarvis::label("x\u{200F}y"), "xy");
    }

    #[tokio::test]
    async fn tls_certificate_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let (_, fp1) = tls_config(dir.path(), Some(IP)).await.unwrap();
        assert!(dir.path().join("mobile").join("cert.pem").is_file());
        assert!(dir.path().join("mobile").join("key.pem").is_file());
        assert_eq!(fp1.split(':').count(), 32, "SHA-256 as 32 hex pairs");
        let (_, fp2) = tls_config(dir.path(), Some(IP)).await.unwrap();
        assert_eq!(fp1, fp2, "the stored certificate is reused");
    }
}
