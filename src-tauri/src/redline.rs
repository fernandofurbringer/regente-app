//! Redline (F3): limites de uso do Claude e do Codex, só leitura. Nunca renova token e nunca escreve nos arquivos do
//! Claude ou do Codex. Do Claude lê só `claudeAiOauth.accessToken` de `<config>/.credentials.json` e consulta
//! `GET {base}/api/oauth/usage`. Do Codex lê o `rate_limits` mais recente dos rollouts de hoje e de ontem. Tokens nunca
//! são logados, emitidos ou devolvidos. Cada consulta tem timeout. Sem leitura nova, a última boa fica com selo de idade.
//! Os parsers são tolerantes e testados com fixtures; o HTTP é testado com um axum em 127.0.0.1:0.

use crate::error::{AppError, AppResult};
use crate::runtime::AppState;
use crate::settings::{load_or_default, save_json};
use crate::util::{lk, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The Anthropic API. Only this base ever receives a token; tests pass a mock's base instead.
pub const DEFAULT_BASE: &str = "https://api.anthropic.com";
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// Largest usage reply read (a real one is a few KB).
const MAX_REPLY: usize = 1024 * 1024;
/// Polling interval, and the most history days the window can ask for.
pub const POLL_EVERY_MS: i64 = 5 * 60 * 1000;
pub const MAX_DAYS: u32 = 30;

/// One limit of one account: `used_percent` in 0..=100, `resets_at` in epoch ms.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub provider: String,
    pub account: String,
    /// The window's key as the provider names it (`five_hour`, `primary`), or `5h` / `semana` for Codex.
    pub window: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

/// A point of the history: one window's reading at `ts`.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitPoint {
    pub ts: i64,
    pub provider: String,
    pub account: String,
    pub window: String,
    pub used_percent: f64,
}

/// Epoch ms from an ISO-8601 string, or from a number (seconds, or ms when it is large).
fn ms_of(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis()),
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()).map(|x| if x > 1e11 { x as i64 } else { (x * 1000.0) as i64 }),
        _ => None,
    }
}

/// Claude's usage reply: every object with `utilization` becomes a window named by its key, at any depth. The scale is
/// decided per reply: fractions (0.4 → 40%) only when every value is at most 1 and some value has a fractional part;
/// otherwise the values are percentages. The endpoint's real shape is unverified.
pub fn parse_claude_usage(v: &Value, account: &str) -> Vec<Window> {
    let mut raw = Vec::new();
    walk_claude(v, &mut raw);
    let fractions = raw.iter().all(|(_, u, _)| *u <= 1.0) && raw.iter().any(|(_, u, _)| *u != 0.0 && u.fract() != 0.0);
    raw.into_iter()
        .map(|(window, u, resets_at)| Window {
            provider: "claude".into(),
            account: account.into(),
            window,
            used_percent: if fractions { u * 100.0 } else { u },
            resets_at,
        })
        .collect()
}

fn walk_claude(v: &Value, out: &mut Vec<(String, f64, Option<i64>)>) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if let Some(o) = val.as_object() {
                    // `extra_usage` (and similar) come with `is_enabled: false` when the feature is off: not a limit
                    if o.get("is_enabled").and_then(Value::as_bool) == Some(false) {
                        continue;
                    }
                    if let Some(u) = o.get("utilization").and_then(Value::as_f64).filter(|u| u.is_finite()) {
                        out.push((k.clone(), u, o.get("resets_at").and_then(ms_of)));
                        continue;
                    }
                }
                walk_claude(val, out);
            }
        }
        Value::Array(a) => {
            for x in a {
                walk_claude(x, out);
            }
        }
        _ => {}
    }
}

/// Codex's `rate_limits`: `primary` and `secondary`, named by `window_minutes` (300 → 5h, 10080 → semana, else `<n>min`).
/// A reset given as seconds from now (`resets_in_seconds`) becomes an absolute time against `now_ms`.
pub fn parse_codex_limits(v: &Value, account: &str, now_ms: i64) -> Vec<Window> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let o = v.get(*key)?.as_object()?;
            let used = o.get("used_percent")?.as_f64()?;
            let window = match o.get("window_minutes").and_then(Value::as_u64) {
                Some(300) => "5h".to_string(),
                Some(10080) => "semana".to_string(),
                Some(n) => format!("{n}min"),
                None => key.to_string(),
            };
            let resets_at = o
                .get("resets_at")
                .and_then(ms_of)
                .or_else(|| o.get("resets_in_seconds").and_then(Value::as_f64).map(|s| now_ms + (s * 1000.0) as i64));
            Some(Window { provider: "codex".into(), account: account.into(), window, used_percent: used, resets_at })
        })
        .collect()
}

// The account's email is never read: only the token is, so there is no email to mask on screen.

/// The Claude OAuth token of one config dir. Read only; None when the file or the field is missing.
pub fn claude_token(config_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_dir.join(".credentials.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().filter(|t| !t.is_empty()).map(String::from)
}

fn client() -> AppResult<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AppError::new("redline_failed", "cliente HTTP indisponível"))?;
    Ok(CLIENT.get_or_init(|| c))
}

/// `GET {base}/api/oauth/usage` with the token as a bearer. A failure names the status, never the token or the body.
pub async fn fetch_claude(client: &reqwest::Client, base: &str, token: &str) -> AppResult<Value> {
    let url = format!("{}/api/oauth/usage", base.trim_end_matches('/'));
    let res = client
        .get(&url)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
        .map_err(|_| AppError::new("redline_failed", "sem resposta do Claude"))?;
    if !res.status().is_success() {
        return Err(AppError::new("redline_failed", format!("Claude respondeu {}", res.status().as_u16())));
    }
    let bytes = res.bytes().await.map_err(|_| AppError::new("redline_failed", "resposta do Claude incompleta"))?;
    if bytes.len() > MAX_REPLY {
        return Err(AppError::new("redline_failed", "resposta do Claude grande demais"));
    }
    serde_json::from_slice(&bytes).map_err(|_| AppError::new("redline_failed", "resposta do Claude inválida"))
}

/// The rollouts of today and yesterday under `<codex_home>/sessions`, newest first.
fn codex_rollouts(codex_home: &Path) -> Vec<(SystemTime, PathBuf)> {
    let base = codex_home.join("sessions");
    let now = chrono::Local::now();
    let mut files = Vec::new();
    for d in [now, now - chrono::Duration::days(1)] {
        let dir = base.join(d.format("%Y").to_string()).join(d.format("%m").to_string()).join(d.format("%d").to_string());
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
                continue;
            }
            let Ok(modified) = e.metadata().and_then(|m| m.modified()) else { continue };
            files.push((modified, e.path()));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
}

fn mtime_ms(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// The last `rate_limits` of one rollout, with the time of its event (its `timestamp`, else the file's mtime). Read line
/// by line and lenient: a line that is not valid UTF-8 or JSON is skipped, the rest of the file is still read.
fn last_rate_limits(path: &Path, mtime: i64) -> Option<(Value, i64)> {
    let mut reader = BufReader::new(std::fs::File::open(path).ok()?);
    let mut buf = Vec::new();
    let mut last = None;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(line) = std::str::from_utf8(&buf) else { continue };
        if !line.contains("rate_limits") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["payload"]["rate_limits"].is_object() {
            last = Some((v["payload"]["rate_limits"].clone(), ms_of(&v["timestamp"]).unwrap_or(mtime)));
        }
    }
    last
}

/// The newest `rate_limits` under `<codex_home>/sessions`: the newest rollout that has one wins, so a file without any
/// is skipped. Returns the limits and the time of their event.
pub fn latest_codex_limits(codex_home: &Path) -> Option<(Value, i64)> {
    codex_rollouts(codex_home).into_iter().find_map(|(m, p)| last_rate_limits(&p, mtime_ms(m)))
}

/// The last good reading of one account. `stale` is true when the latest poll failed; `at` is when it was read.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub windows: Vec<Window>,
    pub at: i64,
    pub stale: bool,
}

/// Last readings by `provider/account`. Lives in AppState.
#[derive(Default)]
pub struct Store {
    last: Mutex<BTreeMap<String, Reading>>,
    /// One poll at a time: the background thread and a forced refresh never insert the same reading twice.
    pub gate: tokio::sync::Mutex<()>,
}

impl Store {
    pub fn put(&self, key: &str, windows: Vec<Window>, at: i64) {
        lk(&self.last).insert(key.into(), Reading { windows, at, stale: false });
    }

    /// The poll failed: the last good reading stays, marked stale. Nothing is invented when there was none.
    pub fn fail(&self, key: &str) {
        if let Some(r) = lk(&self.last).get_mut(key) {
            r.stale = true;
        }
    }

    pub fn remove(&self, key: &str) {
        lk(&self.last).remove(key);
    }

    pub fn snapshot(&self) -> BTreeMap<String, Reading> {
        lk(&self.last).clone()
    }
}

/// Per-account settings, in `redline.json`. Missing accounts are active and not on the top bar.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountPref {
    pub ativa: bool,
    pub no_topo: bool,
}

impl Default for AccountPref {
    fn default() -> Self {
        Self { ativa: true, no_topo: false }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub accounts: BTreeMap<String, AccountPref>,
}

impl Prefs {
    pub fn of(&self, key: &str) -> AccountPref {
        self.accounts.get(key).cloned().unwrap_or_default()
    }
}

pub fn load_prefs(data: &Path) -> Prefs {
    load_or_default(&data.join("redline.json"))
}

pub fn save_prefs(data: &Path, p: &Prefs) -> AppResult<()> {
    std::fs::create_dir_all(data)?;
    save_json(&data.join("redline.json"), p)
}

/// `(provider, account)` of every known account: each CLI's principal, then the Regente accounts.
pub fn known_accounts(s: &AppState) -> Vec<(String, String)> {
    let extra = lk(&s.settings).accounts.clone();
    let mut out = Vec::new();
    for cli in ["claude", "codex"] {
        out.push((cli.to_string(), "principal".to_string()));
        for a in extra.get(cli).into_iter().flatten() {
            out.push((cli.to_string(), a.clone()));
        }
    }
    out
}

/// One account's fresh reading and the time it was taken (None when it could not be read: the caller keeps the last
/// good one).
async fn read_account(s: &AppState, provider: &str, account: &str, now: i64) -> Option<(Vec<Window>, i64)> {
    let dir = s.config_dir_for(provider, Some(account));
    match provider {
        "claude" => {
            let token = claude_token(&dir)?;
            match fetch_claude(client().ok()?, DEFAULT_BASE, &token).await {
                Ok(v) => Some((parse_claude_usage(&v, account), now)),
                Err(e) => {
                    tracing::warn!("redline claude/{account}: {}", e.code());
                    None
                }
            }
        }
        _ => {
            let (v, at) = latest_codex_limits(&dir)?;
            Some((parse_codex_limits(&v, account, now), at))
        }
    }
}

/// What the Redline shows: every active account's windows, the accounts whose latest poll failed, and the
/// `provider/account` keys marked for the top bar.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub windows: Vec<Window>,
    pub stale: Vec<Stale>,
    pub topo: Vec<String>,
}

/// The name a window shows: `five_hour` → 5h, `seven_day` → semana; other keys as they are.
pub fn label(window: &str) -> &str {
    match window {
        "five_hour" => "5h",
        "seven_day" => "semana",
        other => other,
    }
}

/// Time left, as the tray and the window show it: `agora`, `45min`, `2h10`, `3d4h`.
pub fn countdown(ms_left: i64) -> String {
    if ms_left <= 0 {
        return "agora".into();
    }
    let mins = ms_left / 60_000;
    if mins < 60 {
        return format!("{mins}min");
    }
    let h = mins / 60;
    if h < 24 {
        return format!("{h}h{:02}", mins % 60);
    }
    format!("{}d{}h", h / 24, h % 24)
}

/// One tray line: `Claude · principal · 5h 42% · reseta em 2h10`.
pub fn tray_line(w: &Window, now: i64) -> String {
    let who = match w.provider.as_str() {
        "claude" => "Claude",
        "codex" => "Codex",
        other => other,
    };
    let reset = match w.resets_at {
        Some(at) => format!(" · reseta em {}", countdown(at - now)),
        None => String::new(),
    };
    format!("{who} · {} · {} {:.0}%{reset}", w.account, label(&w.window), w.used_percent)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stale {
    pub provider: String,
    pub account: String,
    /// When the shown reading was taken (epoch ms).
    pub at: i64,
}

/// A Codex reading older than this is not a fresh one: the last good reading stays, marked stale.
const CODEX_STALE_MS: i64 = 10 * 60_000;
const PRUNE_EVERY_MS: i64 = 3_600_000;
/// History kept: a little over the 30 days the window can show.
const KEEP_MS: i64 = 35 * 86_400_000;
static LAST_PRUNE: AtomicI64 = AtomicI64::new(0);

/// Deletes history older than `KEEP_MS`, at most once an hour.
fn maybe_prune(s: &AppState, now: i64) {
    let last = LAST_PRUNE.load(Ordering::Relaxed);
    if now - last < PRUNE_EVERY_MS || LAST_PRUNE.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    if let Err(e) = s.db.prune_limits(now - KEEP_MS) {
        tracing::warn!("redline: poda do histórico falhou ({})", e.code());
    }
}

/// Reads every active account and stores the readings. Fresh ones go to the history; failed ones keep the last good reading.
pub async fn poll(s: &AppState) -> Payload {
    let now = now_ms();
    let prefs = load_prefs(&s.data);
    for (provider, account) in known_accounts(s) {
        let key = format!("{provider}/{account}");
        if !prefs.of(&key).ativa {
            s.redline.remove(&key);
            continue;
        }
        match read_account(s, &provider, &account, now).await {
            Some((windows, at)) => {
                if provider == "codex" && now - at > CODEX_STALE_MS {
                    s.redline.fail(&key);
                    continue;
                }
                let unchanged = s.redline.snapshot().get(&key).is_some_and(|r| r.windows == windows);
                // codex rollouts repeat the same numbers between polls: an unchanged reading is not written again
                if !(provider == "codex" && unchanged) {
                    if let Err(e) = s.db.insert_limits(at, &windows) {
                        tracing::warn!("redline: histórico não gravado ({})", e.code());
                    }
                }
                s.redline.put(&key, windows, at);
            }
            None => s.redline.fail(&key),
        }
    }
    maybe_prune(s, now);
    current(s)
}

/// Chaves `provider/conta` do cabeçalho: as contas marcadas "no topo" (ativas e já lidas). Sem nenhuma conta do Claude
/// marcada e lida, entram também todas as contas ativas do Claude já lidas: o `/usage` da conta principal aparece sem
/// configurar nada, mesmo que só uma conta de outro provedor esteja marcada.
pub fn topo_keys(prefs: &Prefs, read: &[String]) -> Vec<String> {
    let ativa = |k: &&String| prefs.of(k).ativa;
    let mut out: Vec<String> = read.iter().filter(ativa).filter(|k| prefs.of(k).no_topo).cloned().collect();
    if !out.iter().any(|k| k.starts_with("claude/")) {
        out.extend(read.iter().filter(ativa).filter(|k| k.starts_with("claude/")).cloned());
    }
    out
}

/// The stored readings of the active accounts, without reading anything. `topo` comes from `topo_keys`.
pub fn current(s: &AppState) -> Payload {
    let prefs = load_prefs(&s.data);
    let snapshot = s.redline.snapshot();
    let read: Vec<String> = snapshot.keys().cloned().collect();
    let mut windows = Vec::new();
    let mut stale = Vec::new();
    for (key, r) in snapshot {
        if !prefs.of(&key).ativa {
            continue;
        }
        if let Some((provider, account)) = key.split_once('/') {
            if r.stale {
                stale.push(Stale { provider: provider.into(), account: account.into(), at: r.at });
            }
        }
        windows.extend(r.windows);
    }
    let topo = topo_keys(&prefs, &read);
    Payload { windows, stale, topo }
}

/// One refresh: poll, history, the `limits` event to every window, and the tray menu.
pub async fn refresh(s: &AppState, app: &tauri::AppHandle) -> Payload {
    let _one_at_a_time = s.redline.gate.lock().await;
    let p = poll(s).await;
    s.emit("limits", json!(p));
    crate::dictation::set_limits(app, &p);
    p
}

/// The Redline's background poll: every five minutes, on its own thread (a slow network never stalls the ticker).
pub fn start(s: std::sync::Arc<AppState>, app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("regente-redline".into())
        .spawn(move || {
            let mut last: Option<i64> = None;
            while !s.rt().shutting_down {
                std::thread::sleep(Duration::from_secs(1));
                let now = now_ms();
                if last.is_some_and(|l| now - l < POLL_EVERY_MS) {
                    continue;
                }
                last = Some(now);
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tauri::async_runtime::block_on(refresh(&s, &app))));
                if r.is_err() {
                    tracing::error!("panic no tick redline");
                }
            }
        })
        .expect("redline thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use axum::http::HeaderMap;
    use std::sync::Arc;

    const CLAUDE: &str = r#"{"five_hour":{"utilization":42.0,"resets_at":"2026-10-14T15:00:00Z"},"seven_day":{"utilization":0.18,"resets_at":null}}"#;

    #[test]
    fn claude_usage_gives_one_window_per_object() {
        let v: Value = serde_json::from_str(CLAUDE).unwrap();
        let w = parse_claude_usage(&v, "principal");
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[0].window, "five_hour");
        assert_eq!(w[0].used_percent, 42.0);
        assert_eq!(w[0].resets_at, Some(1791990000000)); // 2026-10-14T15:00:00Z
        assert_eq!(w[1].window, "seven_day");
        // mixed reply (42 is a percentage): the scale is percent for every window, so 0.18 stays 0.18
        assert_eq!(w[1].used_percent, 0.18);
        assert_eq!(w[1].resets_at, None);
    }

    #[test]
    fn claude_disabled_extra_usage_is_not_a_window() {
        let v = json!({"five_hour": {"utilization": 6}, "extra_usage": {"is_enabled": false, "utilization": 0}});
        let w = parse_claude_usage(&v, "p");
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0].window, "five_hour");
    }

    #[test]
    fn claude_scale_is_decided_per_reply() {
        let scaled = |vals: &[f64]| {
            let v = Value::Object(vals.iter().enumerate().map(|(i, u)| (format!("w{i}"), json!({"utilization": u}))).collect());
            parse_claude_usage(&v, "p").into_iter().map(|w| w.used_percent).collect::<Vec<f64>>()
        };
        assert_eq!(scaled(&[0.4, 0.25]), vec![40.0, 25.0]);
        assert_eq!(scaled(&[0.4, 42.0]), vec![0.4, 42.0]);
        assert_eq!(scaled(&[0.0, 1.0]), vec![0.0, 1.0]);
        assert_eq!(scaled(&[55.0, 3.0]), vec![55.0, 3.0]);
    }

    #[test]
    fn codex_limits_name_the_windows_by_minutes() {
        let v = json!({
            "primary": {"used_percent": 12.0, "window_minutes": 300, "resets_in_seconds": 3600},
            "secondary": {"used_percent": 40.0, "window_minutes": 10080, "resets_in_seconds": 86400}
        });
        let w = parse_codex_limits(&v, "principal", 1_000_000);
        assert_eq!(w.iter().map(|x| x.window.as_str()).collect::<Vec<_>>(), vec!["5h", "semana"]);
        assert_eq!(w[0].used_percent, 12.0);
        assert_eq!(w[0].resets_at, Some(1_000_000 + 3_600_000));
        let odd = json!({"primary": {"used_percent": 1.0, "window_minutes": 60}});
        assert_eq!(parse_codex_limits(&odd, "x", 0)[0].window, "60min");
    }

    #[test]
    fn codex_limits_come_from_the_newest_rollout_of_today() {
        let home = tempfile::tempdir().unwrap();
        let now = chrono::Local::now();
        let dir = home.path().join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("rollout-a-codex-sess-1.jsonl"), include_str!("../tests/fixtures/codex.jsonl")).unwrap();
        let (v, at) = latest_codex_limits(home.path()).expect("rate_limits");
        assert!(at > 0, "the event time comes from the line");
        // the fixture's last rate_limits has only the primary (13%)
        let w = parse_codex_limits(&v, "principal", 0);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].window.as_str(), w[0].used_percent), ("5h", 13.0));
        assert!(latest_codex_limits(tempfile::tempdir().unwrap().path()).is_none());
    }

    #[test]
    fn tray_lines_and_countdowns_read_like_the_redline() {
        let w = Window { provider: "claude".into(), account: "principal".into(), window: "five_hour".into(), used_percent: 42.4, resets_at: Some(10_000_000 + 2 * 3_600_000 + 10 * 60_000) };
        assert_eq!(tray_line(&w, 10_000_000), "Claude · principal · 5h 42% · reseta em 2h10");
        let semana = Window { provider: "codex".into(), account: "x".into(), window: "semana".into(), used_percent: 18.0, resets_at: None };
        assert_eq!(tray_line(&semana, 0), "Codex · x · semana 18%");
        assert_eq!(countdown(-5), "agora");
        assert_eq!(countdown(45 * 60_000), "45min");
        assert_eq!(countdown((3 * 24 + 4) * 3_600_000), "3d4h");
    }

    #[test]
    fn codex_reads_skip_bad_lines_and_fall_back_to_an_older_rollout() {
        let home = tempfile::tempdir().unwrap();
        let now = chrono::Local::now();
        let dir = home.path().join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        // a line with invalid UTF-8 in the middle of a good file: the good lines after it still count
        let mut good = b"{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":7.0,\"window_minutes\":300}}}}\n".to_vec();
        good.extend_from_slice(&[0xff, 0xfe, b'\n']);
        good.extend_from_slice(b"{\"payload\":{\"rate_limits\":{\"primary\":{\"used_percent\":9.0,\"window_minutes\":300}}}}\n");
        std::fs::write(dir.join("rollout-a.jsonl"), good).unwrap();
        // the other rollout has no rate_limits at all: whichever is newer, the reading comes from the one that has them
        std::fs::write(dir.join("rollout-b.jsonl"), b"{\"payload\":{}}\n").unwrap();
        let (v, _) = latest_codex_limits(home.path()).expect("the rollout with limits");
        assert_eq!(v["primary"]["used_percent"], 9.0);
    }

    #[test]
    fn pruning_drops_only_the_old_points() {
        let db = Db::open_in_memory().unwrap();
        let w = vec![Window { provider: "codex".into(), account: "p".into(), window: "5h".into(), used_percent: 1.0, resets_at: None }];
        db.insert_limits(1_000, &w).unwrap();
        db.insert_limits(5_000, &w).unwrap();
        db.prune_limits(2_000).unwrap();
        let left = db.limits_since(0).unwrap();
        assert_eq!(left.iter().map(|p| p.ts).collect::<Vec<_>>(), vec![5_000]);
    }

    #[test]
    fn token_comes_from_the_credentials_file_read_only() {
        let d = tempfile::tempdir().unwrap();
        assert!(claude_token(d.path()).is_none());
        std::fs::write(d.path().join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"tok-1","refreshToken":"r"}}"#).unwrap();
        assert_eq!(claude_token(d.path()).as_deref(), Some("tok-1"));
        std::fs::write(d.path().join(".credentials.json"), r#"{"outro":1}"#).unwrap();
        assert!(claude_token(d.path()).is_none());
    }

    /// A mock of the usage endpoint that records the headers of the last call.
    async fn mock(status: axum::http::StatusCode, body: &'static str) -> (String, Arc<Mutex<Option<HeaderMap>>>) {
        let seen: Arc<Mutex<Option<HeaderMap>>> = Arc::default();
        let (rec, listener) = (seen.clone(), tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let app = axum::Router::new().route(
            "/api/oauth/usage",
            axum::routing::get(move |headers: HeaderMap| {
                let rec = rec.clone();
                async move {
                    *lk(&rec) = Some(headers);
                    (status, body)
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });
        (base, seen)
    }

    #[tokio::test]
    async fn fetch_sends_the_bearer_and_the_beta_header_and_returns_the_json() {
        let (base, seen) = mock(axum::http::StatusCode::OK, CLAUDE).await;
        let v = fetch_claude(client().unwrap(), &base, "tok-secreto").await.unwrap();
        assert_eq!(parse_claude_usage(&v, "p").len(), 2);
        let h = lk(&seen).clone().expect("the mock was called");
        assert_eq!(h["authorization"], "Bearer tok-secreto");
        assert_eq!(h["anthropic-beta"], "oauth-2025-04-20");
    }

    #[tokio::test]
    async fn fetch_error_names_the_status_not_the_token() {
        let (base, _) = mock(axum::http::StatusCode::UNAUTHORIZED, "{\"error\":\"tok-secreto\"}").await;
        let e = fetch_claude(client().unwrap(), &base, "tok-secreto").await.unwrap_err();
        assert_eq!(e.code(), "redline_failed");
        assert!(e.to_string().contains("401") && !e.to_string().contains("tok-secreto"));
    }

    #[test]
    fn a_failed_poll_keeps_the_last_reading_marked_stale() {
        let st = Store::default();
        let w = vec![Window { provider: "claude".into(), account: "principal".into(), window: "five_hour".into(), used_percent: 5.0, resets_at: None }];
        st.put("claude/principal", w.clone(), 100);
        st.fail("claude/principal");
        let r = st.snapshot()["claude/principal"].clone();
        assert!(r.stale);
        assert_eq!((r.windows, r.at), (w, 100));
        st.fail("codex/nunca-lido");
        assert!(!st.snapshot().contains_key("codex/nunca-lido"));
    }

    #[test]
    fn prefs_default_to_active_and_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let mut p = load_prefs(d.path());
        assert_eq!(p.of("claude/principal"), AccountPref { ativa: true, no_topo: false });
        p.accounts.insert("claude/principal".into(), AccountPref { ativa: true, no_topo: true });
        save_prefs(d.path(), &p).unwrap();
        assert!(load_prefs(d.path()).of("claude/principal").no_topo);
    }

    #[test]
    fn history_is_stored_and_read_back_by_time() {
        let db = Db::open_in_memory().unwrap();
        let w = vec![Window { provider: "codex".into(), account: "principal".into(), window: "5h".into(), used_percent: 13.0, resets_at: None }];
        db.insert_limits(1_000, &w).unwrap();
        db.insert_limits(2_000, &w).unwrap();
        let pts = db.limits_since(1_500).unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!((pts[0].ts, pts[0].window.as_str(), pts[0].used_percent), (2_000, "5h", 13.0));
    }

    fn prefs_with(marked: &[&str]) -> Prefs {
        let mut p = Prefs::default();
        for k in marked {
            p.accounts.insert((*k).into(), AccountPref { ativa: true, no_topo: true });
        }
        p
    }

    #[test]
    fn topo_without_marks_is_every_claude_account_read() {
        let read: Vec<String> = ["claude/principal", "codex/principal", "claude/trabalho"].map(String::from).to_vec();
        assert_eq!(topo_keys(&Prefs::default(), &read), vec!["claude/principal".to_string(), "claude/trabalho".to_string()]);
        // nada lido do Claude: nada no topo (o chip mostra "—")
        assert_eq!(topo_keys(&Prefs::default(), &["codex/principal".to_string()]), Vec::<String>::new());
    }

    #[test]
    fn a_marked_claude_account_disables_the_fallback() {
        let read: Vec<String> = ["claude/principal", "codex/principal", "claude/trabalho"].map(String::from).to_vec();
        assert_eq!(topo_keys(&prefs_with(&["claude/trabalho"]), &read), vec!["claude/trabalho".to_string()]);
        // marcada mas sem leitura: não entra; sem conta do Claude marcada e lida, o fallback vale
        assert_eq!(topo_keys(&prefs_with(&["claude/nunca-lida"]), &read), vec!["claude/principal".to_string(), "claude/trabalho".to_string()]);
    }

    #[test]
    fn a_marked_codex_account_keeps_the_claude_accounts() {
        let read: Vec<String> = ["claude/principal", "codex/principal", "claude/trabalho"].map(String::from).to_vec();
        // o Codex marcado fica, e o Claude continua no cabeçalho (os chips só mostram o Claude)
        assert_eq!(topo_keys(&prefs_with(&["codex/principal"]), &read), vec!["codex/principal".to_string(), "claude/principal".to_string(), "claude/trabalho".to_string()]);
    }

    #[test]
    fn topo_skips_inactive_accounts() {
        let mut p = Prefs::default();
        p.accounts.insert("claude/trabalho".into(), AccountPref { ativa: false, no_topo: true });
        let read: Vec<String> = ["claude/principal", "claude/trabalho"].map(String::from).to_vec();
        assert_eq!(topo_keys(&p, &read), vec!["claude/principal".to_string()]);
    }
}
