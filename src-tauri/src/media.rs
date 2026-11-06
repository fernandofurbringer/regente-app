//! Mídia gerada por IA: imagem (Gemini ou GPT Image), vídeo (Replicate) e áudio (ElevenLabs). Cada cliente recebe a
//! `base` do provedor (os testes apontam para um axum em 127.0.0.1). As chaves vêm só do cofre (`media.<provedor>`).
//! As saídas vão para o diretório que o chamador escolhe; o nome sai de `out_name` (slug seguro + data).
//! Config em `data/media.json`.

use crate::error::{fail, AppError, AppResult};
use crate::runtime::AppState;
use crate::settings::save_json;
use crate::squads::MediaAllow;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::util::now_ms;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Largest reference image sent to a provider.
const MAX_REF: u64 = 20 * 1024 * 1024;
/// Largest reply read from a provider.
const MAX_JSON: usize = 2 * 1024 * 1024;
const MAX_IMAGE: usize = 50 * 1024 * 1024;
const MAX_AUDIO: usize = 50 * 1024 * 1024;
const MAX_VIDEO: usize = 300 * 1024 * 1024;
/// Replicate is polled this often until the prediction ends, and for at most this long.
const POLL_EVERY: Duration = Duration::from_secs(3);
const VIDEO_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Generation requests (a provider may take minutes), one status poll, one download.
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Names of the switches in `MediaConfig::enabled`.
pub const KINDS: [&str; 4] = ["google-image", "openai-image", "replicate-video", "elevenlabs"];
const ASPECTS: [&str; 7] = ["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaConfig {
    /// google | openai
    pub image_provider: String,
    pub google_model: String,
    pub openai_model: String,
    /// owner/name on Replicate, e.g. kwaivgi/kling-v2.1
    pub video_model: String,
    /// ElevenLabs voice id for `voz` (efeito does not need one).
    pub eleven_voice: String,
    /// google-image, openai-image, replicate-video, elevenlabs
    pub enabled: Vec<String>,
    /// Endpoint per provider (google, openai, replicate, elevenlabs). Empty = the official one.
    pub base_urls: BTreeMap<String, String>,
    /// Spending caps for MCP calls, per kind (image, video, audio): attempts per day (all missions) ...
    pub max_per_day: BTreeMap<String, u32>,
    /// ... and per mission. The UI's [Testar] is not counted.
    pub max_per_mission: BTreeMap<String, u32>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        let caps = |v: [(&str, u32); 3]| v.iter().map(|(k, n)| (k.to_string(), *n)).collect();
        Self {
            image_provider: "google".into(),
            google_model: "gemini-2.5-flash-image".into(),
            openai_model: "gpt-image-1".into(),
            video_model: "kwaivgi/kling-v2.1".into(),
            eleven_voice: String::new(),
            enabled: KINDS.iter().map(|k| k.to_string()).collect(),
            base_urls: BTreeMap::new(),
            max_per_day: caps([("image", 30), ("video", 5), ("audio", 30)]),
            max_per_mission: caps([("image", 10), ("video", 2), ("audio", 10)]),
        }
    }
}

pub fn load(data: &Path) -> MediaConfig {
    let path = data.join("media.json");
    if !path.exists() {
        let _ = save_json(&path, &MediaConfig::default());
    }
    crate::settings::load_or_default(&path)
}

pub fn save(data: &Path, c: &MediaConfig) -> AppResult<()> {
    validate(c)?;
    std::fs::create_dir_all(data)?;
    save_json(&data.join("media.json"), c)
}

fn validate(c: &MediaConfig) -> AppResult<()> {
    if !matches!(c.image_provider.as_str(), "google" | "openai") {
        return fail("bad_provider", "provedor de imagem: google ou openai");
    }
    if let Some(k) = c.enabled.iter().find(|k| !KINDS.contains(&k.as_str())) {
        return fail("bad_provider", format!("provedor de mídia desconhecido: {k}"));
    }
    for k in c.max_per_day.keys().chain(c.max_per_mission.keys()) {
        kind_index(k)?;
    }
    for (name, url) in &c.base_urls {
        if !matches!(name.as_str(), "google" | "openai" | "replicate" | "elevenlabs") {
            return fail("bad_provider", format!("provedor de mídia desconhecido: {name}"));
        }
        if !url.trim().is_empty() {
            check_endpoint(url.trim())?;
        }
    }
    Ok(())
}

/// https, or http only for loopback: the keys travel in these requests.
fn check_endpoint(raw: &str) -> AppResult<()> {
    let url = reqwest::Url::parse(raw).map_err(|_| AppError::new("bad_media_url", "URL de mídia inválida"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => fail("bad_media_url", "a URL de mídia precisa ser https (http só em localhost)"),
    }
}

/// The endpoint of a provider: the override from the config, or the official one.
fn base_of(cfg: &MediaConfig, provider: &str) -> String {
    let default = match provider {
        "google" => "https://generativelanguage.googleapis.com",
        "openai" => "https://api.openai.com",
        "replicate" => "https://api.replicate.com",
        _ => "https://api.elevenlabs.io",
    };
    match cfg.base_urls.get(provider).map(|u| u.trim()).filter(|u| !u.is_empty()) {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => default.to_string(),
    }
}

/// The key of a provider, from the vault (`media.<provider>`). Missing: an error that names the key.
fn key(s: &AppState, provider: &str) -> AppResult<String> {
    let name = format!("media.{provider}");
    s.secret(&name)?.ok_or_else(|| AppError::new("missing_secret", format!("configure a chave '{name}' no Cofre")))
}

fn ensure_enabled(cfg: &MediaConfig, kind: &str) -> AppResult<()> {
    if cfg.enabled.iter().any(|k| k == kind) {
        return Ok(());
    }
    fail("media_disabled", format!("{kind} está desativado em Configurações → Mídia"))
}

/// A squad may restrict the media it uses (`media: {providers, models}`). Empty lists allow everything.
pub fn check_allowlist(squad_media: Option<&MediaAllow>, provider: &str, model: &str) -> AppResult<()> {
    let Some(a) = squad_media else { return Ok(()) };
    if !a.providers.is_empty() && !a.providers.iter().any(|p| p == provider) {
        return fail("media_blocked", format!("a squad não permite o provedor {provider}"));
    }
    if !a.models.is_empty() && !a.models.iter().any(|m| m == model) {
        return fail("media_blocked", format!("a squad não permite o modelo {model}"));
    }
    Ok(())
}

/// Names and models of the enabled providers, and whether each key is in the vault. Never the key itself.
pub fn providers_info(s: &AppState) -> AppResult<Value> {
    let cfg = load(&s.data);
    let mut keys = serde_json::Map::new();
    for p in ["google", "openai", "replicate", "elevenlabs"] {
        keys.insert(p.into(), json!(s.secret(&format!("media.{p}"))?.is_some()));
    }
    Ok(json!({
        "enabled": cfg.enabled,
        "imageProvider": cfg.image_provider,
        "models": {"google": cfg.google_model, "openai": cfg.openai_model, "replicate": cfg.video_model},
        "elevenVoice": cfg.eleven_voice,
        "keys": keys,
    }))
}

fn client() -> AppResult<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    // no redirects (the key must not follow one); each request sets its own timeout
    let c = reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::new("media_failed", e.to_string()))?;
    Ok(CLIENT.get_or_init(|| c))
}

/// Sends a request and reads the reply up to `max` bytes. A failure names the status, never the body or the key.
async fn bytes_of(req: reqwest::RequestBuilder, max: usize) -> AppResult<Vec<u8>> {
    bytes_of_t(req, max, CALL_TIMEOUT).await
}

async fn bytes_of_t(req: reqwest::RequestBuilder, max: usize, timeout: Duration) -> AppResult<Vec<u8>> {
    let mut resp = req.timeout(timeout).send().await.map_err(|e| AppError::new("media_failed", format!("falha de rede: {}", e.without_url())))?;
    let status = resp.status();
    if !status.is_success() {
        return fail("media_failed", format!("o provedor respondeu HTTP {}", status.as_u16()));
    }
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return fail("media_failed", "resposta do provedor acima do limite");
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::new("media_failed", format!("falha ao ler a resposta: {}", e.without_url())))? {
        if out.len() + chunk.len() > max {
            return fail("media_failed", "resposta do provedor acima do limite");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn json_of(bytes: &[u8]) -> AppResult<Value> {
    serde_json::from_slice(bytes).map_err(|_| AppError::new("media_failed", "resposta inválida do provedor"))
}

/// Model and voice ids go into URL paths: only letters, digits and `._-` (and one `/` for Replicate owner/name).
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 120 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A reference image read from disk: png, jpg or webp, up to `MAX_REF`.
#[derive(Debug)]
pub struct Ref {
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub fn read_ref(p: &Path) -> AppResult<Ref> {
    let mime = match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => return fail("bad_reference", "imagem de referência: use png, jpg ou webp"),
    };
    if std::fs::metadata(p)?.len() > MAX_REF {
        return fail("bad_reference", "imagem de referência acima de 20 MB");
    }
    Ok(Ref { mime: mime.into(), bytes: std::fs::read(p)? })
}

fn data_uri(r: &Ref) -> String {
    format!("data:{};base64,{}", r.mime, B64.encode(&r.bytes))
}

fn aspect_ok(a: &str) -> AppResult<&str> {
    if ASPECTS.contains(&a) {
        return Ok(a);
    }
    fail("bad_aspect", "proporção: 1:1, 16:9, 9:16, 4:3, 3:4, 3:2 ou 2:3")
}

/// GPT Image takes three sizes; the aspect picks one.
fn openai_size(aspect: Option<&str>) -> &'static str {
    match aspect {
        Some("16:9" | "3:2" | "4:3") => "1536x1024",
        Some("9:16" | "2:3" | "3:4") => "1024x1536",
        _ => "1024x1024",
    }
}

fn decode(b64: &str) -> AppResult<Vec<u8>> {
    B64.decode(b64).map_err(|_| AppError::new("media_failed", "imagem inválida na resposta do provedor"))
}

/// Gemini image model: `inlineData` (or `inline_data`) of the first candidate's parts.
async fn google_image(c: &reqwest::Client, base: &str, key: &str, model: &str, prompt: &str, aspect: Option<&str>, reference: Option<&Ref>) -> AppResult<Vec<u8>> {
    if !safe_id(model) {
        return fail("bad_model", "modelo de imagem inválido");
    }
    let mut parts = vec![json!({"text": prompt})];
    if let Some(r) = reference {
        parts.push(json!({"inline_data": {"mime_type": r.mime, "data": B64.encode(&r.bytes)}}));
    }
    let mut config = json!({"responseModalities": ["IMAGE"]});
    if let Some(a) = aspect {
        config["imageConfig"] = json!({"aspectRatio": aspect_ok(a)?});
    }
    let body = json!({"contents": [{"parts": parts}], "generationConfig": config});
    let req = c.post(format!("{base}/v1beta/models/{model}:generateContent")).header("x-goog-api-key", key).json(&body);
    let reply = json_of(&bytes_of(req, MAX_JSON).await?)?;
    let data = reply["candidates"][0]["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|p| p.get("inlineData").or_else(|| p.get("inline_data")).and_then(|d| d["data"].as_str()));
    let bytes = decode(data.ok_or_else(|| AppError::new("media_failed", "a resposta não trouxe imagem"))?)?;
    if bytes.len() > MAX_IMAGE {
        return fail("media_failed", "imagem acima de 50 MB");
    }
    Ok(bytes)
}

/// GPT Image: generations, or edits (multipart) when there is a reference image.
async fn openai_image(c: &reqwest::Client, base: &str, key: &str, model: &str, prompt: &str, aspect: Option<&str>, reference: Option<&Ref>) -> AppResult<Vec<u8>> {
    if !safe_id(model) {
        return fail("bad_model", "modelo de imagem inválido");
    }
    if let Some(a) = aspect {
        aspect_ok(a)?;
    }
    let size = openai_size(aspect);
    let req = match reference {
        Some(r) => {
            let part = reqwest::multipart::Part::bytes(r.bytes.clone()).file_name("referencia").mime_str(&r.mime).map_err(|_| AppError::new("bad_reference", "tipo de imagem inválido"))?;
            let form = reqwest::multipart::Form::new().part("image", part).text("prompt", prompt.to_string()).text("model", model.to_string()).text("size", size);
            c.post(format!("{base}/v1/images/edits")).bearer_auth(key).multipart(form)
        }
        None => c.post(format!("{base}/v1/images/generations")).bearer_auth(key).json(&json!({"model": model, "prompt": prompt, "size": size, "n": 1})),
    };
    let reply = json_of(&bytes_of(req, MAX_JSON).await?)?;
    let bytes = decode(reply["data"][0]["b64_json"].as_str().ok_or_else(|| AppError::new("media_failed", "a resposta não trouxe imagem"))?)?;
    if bytes.len() > MAX_IMAGE {
        return fail("media_failed", "imagem acima de 50 MB");
    }
    Ok(bytes)
}

/// Replicate: one prediction with `Prefer: wait`, then polling of `urls.get` until it ends. The poll goes to our own
/// endpoint only (the key is sent there); the output is downloaded without the key.
#[allow(clippy::too_many_arguments)]
async fn replicate_video(c: &reqwest::Client, base: &str, key: &str, model: &str, prompt: &str, start: Option<&Ref>, end: Option<&Ref>, poll: Duration, timeout: Duration) -> AppResult<Vec<u8>> {
    if !model.contains('/') || !model.split('/').all(safe_id) {
        return fail("bad_model", "modelo de vídeo: owner/nome (ex.: kwaivgi/kling-v2.1)");
    }
    let mut input = json!({"prompt": prompt});
    if let Some(s) = start {
        input["start_image"] = json!(data_uri(s));
    }
    if let Some(e) = end {
        input["end_image"] = json!(data_uri(e));
    }
    let req = c.post(format!("{base}/v1/models/{model}/predictions")).bearer_auth(key).header("Prefer", "wait").json(&json!({"input": input}));
    let t0 = Instant::now();
    let mut pred = json_of(&bytes_of(req, MAX_JSON).await?)?;
    loop {
        match pred["status"].as_str().unwrap_or("") {
            "succeeded" => break,
            "failed" | "canceled" => return fail("media_failed", "o Replicate não concluiu o vídeo"),
            _ => {}
        }
        if t0.elapsed() > timeout {
            return fail("media_timeout", "o vídeo passou de 10 min");
        }
        let get = pred["urls"]["get"].as_str().unwrap_or("").to_string();
        // the key goes to this URL: same scheme, host and port as the configured endpoint, or nothing
        if !same_origin(&get, base) {
            return fail("media_failed", "o provedor devolveu um endereço de consulta inesperado");
        }
        tokio::time::sleep(poll).await;
        pred = json_of(&bytes_of_t(c.get(get).bearer_auth(key), MAX_JSON, POLL_TIMEOUT).await?)?;
    }
    let url = match &pred["output"] {
        Value::String(u) => u.clone(),
        Value::Array(a) => a.first().and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    };
    check_download(&url, base).map_err(|_| AppError::new("media_failed", "o provedor não devolveu um vídeo"))?;
    bytes_of_t(c.get(url), MAX_VIDEO, DOWNLOAD_TIMEOUT).await
}

fn is_loopback(u: &reqwest::Url) -> bool {
    matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// Same scheme, host and port (the URL parser, not string prefixes: `api.replicate.com.evil.tld` is another host).
fn same_origin(url: &str, base: &str) -> bool {
    match (reqwest::Url::parse(url), reqwest::Url::parse(base)) {
        (Ok(a), Ok(b)) => a.scheme() == b.scheme() && a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default(),
        _ => false,
    }
}

/// Where a finished output may be downloaded from: https, or loopback http when the configured endpoint is loopback
/// too (tests and local proxies). Never plain http on the internet.
fn check_download(url: &str, base: &str) -> AppResult<()> {
    let u = reqwest::Url::parse(url).map_err(|_| AppError::new("media_failed", "endereço de saída inválido"))?;
    let base_loopback = reqwest::Url::parse(base).is_ok_and(|b| is_loopback(&b));
    match u.scheme() {
        "https" => Ok(()),
        "http" if base_loopback && is_loopback(&u) => Ok(()),
        _ => fail("media_failed", "endereço de saída sem https"),
    }
}

/// Saves the media settings, and the copy the MCP caps are checked against (see `runtime::Guarded`).
pub fn set_config(s: &AppState, c: &MediaConfig) -> AppResult<()> {
    save(&s.data, c)?;
    crate::util::lk(&s.guarded).media = c.clone();
    Ok(())
}

/// Spending gate of an MCP call, run before any network call. The per-day cap counts attempts from local midnight
/// across all missions, the per-mission cap counts the mission's attempts; failed attempts count too. Only one
/// generation per kind runs at a time: the returned ticket holds that slot until it is dropped.
pub fn acquire(s: &AppState, kind: &str, mission: Option<&str>) -> AppResult<Ticket> {
    let idx = kind_index(kind)?;
    // the caps in memory (loaded at startup, set by `set_config`): a pane rewriting media.json on disk changes nothing
    let cfg = crate::util::lk(&s.guarded).media.clone();
    if BUSY[idx].compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return fail("media_busy", format!("já há uma geração de {kind} em andamento; espere ela terminar"));
    }
    // from here on, every early return drops the ticket and frees the slot
    let ticket = Ticket(idx);
    if let Some(&max) = cfg.max_per_day.get(kind) {
        if s.db.media_attempts(kind, start_of_today_ms(), None)? >= i64::from(max) {
            return fail("media_cap_reached", format!("limite diário de {max} geração(ões) de {kind} atingido"));
        }
    }
    if let (Some(m), Some(&max)) = (mission, cfg.max_per_mission.get(kind)) {
        if s.db.media_attempts(kind, 0, Some(m))? >= i64::from(max) {
            return fail("media_cap_reached", format!("limite de {max} geração(ões) de {kind} nesta missão atingido"));
        }
    }
    s.db.add_media_attempt(kind, mission, now_ms())?;
    Ok(ticket)
}

fn start_of_today_ms() -> i64 {
    chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| d.and_local_timezone(chrono::Local).earliest())
        .map_or(0, |d| d.timestamp_millis())
}

/// Attempts of each kind today, and the daily cap: Settings → Mídia shows them.
pub fn usage_today(s: &AppState) -> AppResult<Value> {
    let cfg = load(&s.data);
    let day = start_of_today_ms();
    let mut out = vec![];
    for kind in ["image", "video", "audio"] {
        out.push(json!({
            "kind": kind,
            "today": s.db.media_attempts(kind, day, None)?,
            "perDay": cfg.max_per_day.get(kind),
            "perMission": cfg.max_per_mission.get(kind),
        }));
    }
    Ok(json!(out))
}

/// One generation slot of a kind; released on drop.
#[derive(Debug)]
pub struct Ticket(usize);

impl Drop for Ticket {
    fn drop(&mut self) {
        BUSY[self.0].store(false, Ordering::SeqCst);
    }
}

/// In-flight flags per kind (image, video, audio).
static BUSY: [AtomicBool; 3] = [AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false)];

fn kind_index(kind: &str) -> AppResult<usize> {
    match kind {
        "image" => Ok(0),
        "video" => Ok(1),
        "audio" => Ok(2),
        _ => fail("bad_kind", "tipo de mídia: image, video ou audio"),
    }
}

/// ElevenLabs: `voz` is text-to-speech with a voice; `efeito` is sound generation. Both return mp3 bytes.
async fn elevenlabs_audio(c: &reqwest::Client, base: &str, key: &str, kind: &str, text: &str, voice: &str) -> AppResult<Vec<u8>> {
    let req = match kind {
        "efeito" => c.post(format!("{base}/v1/sound-generation")).header("xi-api-key", key).json(&json!({"text": text})),
        "voz" => {
            if !safe_id(voice) {
                return fail("bad_voice", "voz do ElevenLabs inválida: use o id da voz");
            }
            c.post(format!("{base}/v1/text-to-speech/{voice}")).header("xi-api-key", key).header("Accept", "audio/mpeg").json(&json!({"text": text, "model_id": "eleven_multilingual_v2"}))
        }
        _ => return fail("bad_kind", "tipo de áudio: voz ou efeito"),
    };
    bytes_of(req, MAX_AUDIO).await
}

/// `<slug de 40>-<yyyymmdd-hhmmss>.<ext>`: the slug is lowercase ascii letters, digits and dashes only, so it is a safe file name.
pub fn out_name(prompt: &str, ext: &str) -> String {
    let mut slug: String = prompt.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug: String = slug.trim_matches('-').chars().take(40).collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "midia" } else { slug };
    format!("{slug}-{}.{ext}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
}

/// Writes an output into `dir` (created if missing). A name already taken gets a counter before its extension.
pub fn write_out(dir: &Path, name: &str, bytes: &[u8]) -> AppResult<PathBuf> {
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) || name.starts_with('.') {
        return fail("bad_name", "nome de arquivo inválido");
    }
    std::fs::create_dir_all(dir)?;
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), e.to_string()),
        None => (name.to_string(), String::new()),
    };
    let mut path = dir.join(name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    std::fs::write(&path, bytes)?;
    Ok(path)
}

pub struct ImageReq {
    pub prompt: String,
    /// 1:1 | 16:9 | 9:16 | 4:3 | 3:4 | 3:2 | 2:3
    pub aspect: Option<String>,
    pub reference: Option<PathBuf>,
    /// google | openai (default: the configured one)
    pub provider: Option<String>,
    pub model: Option<String>,
}

pub async fn image_generate(s: &AppState, r: ImageReq, out_dir: &Path, allow: Option<&MediaAllow>) -> AppResult<PathBuf> {
    let cfg = load(&s.data);
    let provider = r.provider.clone().unwrap_or_else(|| cfg.image_provider.clone());
    let (kind, model) = match provider.as_str() {
        "google" => ("google-image", r.model.clone().unwrap_or_else(|| cfg.google_model.clone())),
        "openai" => ("openai-image", r.model.clone().unwrap_or_else(|| cfg.openai_model.clone())),
        other => return fail("bad_provider", format!("provedor de imagem desconhecido: {other}")),
    };
    ensure_enabled(&cfg, kind)?;
    check_allowlist(allow, kind, &model)?;
    let api_key = key(s, &provider)?;
    let reference = r.reference.as_deref().map(read_ref).transpose()?;
    let base = base_of(&cfg, &provider);
    let bytes = if provider == "google" {
        google_image(client()?, &base, &api_key, &model, &r.prompt, r.aspect.as_deref(), reference.as_ref()).await?
    } else {
        openai_image(client()?, &base, &api_key, &model, &r.prompt, r.aspect.as_deref(), reference.as_ref()).await?
    };
    write_out(out_dir, &out_name(&r.prompt, "png"), &bytes)
}

pub async fn video_generate(s: &AppState, prompt: &str, start: Option<PathBuf>, end: Option<PathBuf>, model: Option<String>, out_dir: &Path, allow: Option<&MediaAllow>) -> AppResult<PathBuf> {
    let cfg = load(&s.data);
    ensure_enabled(&cfg, "replicate-video")?;
    let model = model.unwrap_or_else(|| cfg.video_model.clone());
    check_allowlist(allow, "replicate-video", &model)?;
    let api_key = key(s, "replicate")?;
    let start = start.as_deref().map(read_ref).transpose()?;
    let end = end.as_deref().map(read_ref).transpose()?;
    let base = base_of(&cfg, "replicate");
    let bytes = replicate_video(client()?, &base, &api_key, &model, prompt, start.as_ref(), end.as_ref(), POLL_EVERY, VIDEO_TIMEOUT).await?;
    write_out(out_dir, &out_name(prompt, "mp4"), &bytes)
}

/// Áudio (Mídia e MCP). `voz` usa a voz local quando instalada (grátis, sem chave); senão o ElevenLabs, se houver chave.
/// `efeito` é só ElevenLabs.
pub async fn audio_generate(s: &AppState, text: &str, kind: &str, voice: Option<String>, out_dir: &Path, allow: Option<&MediaAllow>) -> AppResult<PathBuf> {
    let cfg = load(&s.data);
    if kind == "voz" && crate::voz_local::instalada(&s.data) {
        return audio_local(s, text, voice, out_dir).await;
    }
    if kind == "voz" && !(cfg.enabled.iter().any(|k| k == "elevenlabs") && s.secret("media.elevenlabs")?.is_some()) {
        return fail("voz_local_ausente", "Baixe a voz local em Configurações → Voz (ou configure o ElevenLabs em Mídia).");
    }
    ensure_enabled(&cfg, "elevenlabs")?;
    check_allowlist(allow, "elevenlabs", kind)?;
    let voice = voice.filter(|v| !v.is_empty()).unwrap_or_else(|| cfg.eleven_voice.clone());
    if kind == "voz" && voice.is_empty() {
        return fail("no_voice", "defina a voz do ElevenLabs em Configurações → Mídia");
    }
    let api_key = key(s, "elevenlabs")?;
    let bytes = elevenlabs_audio(client()?, &base_of(&cfg, "elevenlabs"), &api_key, kind, text, &voice).await?;
    if bytes.len() > MAX_AUDIO {
        return fail("media_failed", "áudio acima de 50 MB");
    }
    write_out(out_dir, &out_name(text, "mp3"), &bytes)
}

/// Voz local (Supertonic 3) para a Mídia: a voz pedida (se for uma das locais) ou a das preferências.
/// Quem chama (o `gate` da ferramenta MCP) já segura o slot de áudio e contou a tentativa no limite: pegar o slot de
/// novo aqui dava sempre `media_busy`.
async fn audio_local(s: &AppState, text: &str, voice: Option<String>, out_dir: &Path) -> AppResult<PathBuf> {
    let p = crate::util::lk(&s.settings).voz_local.clone();
    let voz = voice.filter(|v| crate::voz_local::VOZES.contains(&v.as_str())).unwrap_or(p.voz);
    let nome = out_name(text, "wav");
    let (data, texto) = (s.data.clone(), text.to_string());
    let wav = tauri::async_runtime::spawn_blocking(move || crate::voz_local::sintetizar(&data, &texto, &voz, p.velocidade, p.passos))
        .await
        .map_err(|e| AppError::new("voz_local", e.to_string()))??;
    write_out(out_dir, &nome, &wav)
}

/// Longest text one spoken utterance takes (voz local e ElevenLabs).
pub const SPEECH_MAX_CHARS: usize = 1000;

/// The text the voice reads: at most `SPEECH_MAX_CHARS` characters, cut at the last space when it is in the second half.
pub fn cap_speech(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= SPEECH_MAX_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(SPEECH_MAX_CHARS).collect();
    match cut.rfind(' ') {
        Some(i) if i > cut.len() / 2 => cut[..i].to_string(),
        _ => cut,
    }
}

/// Settings → Mídia → [Testar]: checks the key with a free, read-only call of each provider. Nothing is generated and
/// nothing is charged. Not counted in the spending caps (the caps are for MCP calls).
pub async fn test(s: &AppState, provider: &str) -> AppResult<String> {
    let cfg = load(&s.data);
    let c = client()?;
    let base = base_of(&cfg, provider);
    let api_key = key(s, provider)?;
    let req = match provider {
        "google" => {
            ensure_enabled(&cfg, "google-image")?;
            c.get(format!("{base}/v1beta/models?pageSize=1")).header("x-goog-api-key", &api_key)
        }
        "openai" => {
            ensure_enabled(&cfg, "openai-image")?;
            c.get(format!("{base}/v1/models")).bearer_auth(&api_key)
        }
        "replicate" => {
            ensure_enabled(&cfg, "replicate-video")?;
            c.get(format!("{base}/v1/account")).bearer_auth(&api_key)
        }
        "elevenlabs" => {
            ensure_enabled(&cfg, "elevenlabs")?;
            c.get(format!("{base}/v1/user")).header("xi-api-key", &api_key)
        }
        _ => return fail("bad_provider", format!("provedor de mídia desconhecido: {provider}")),
    };
    bytes_of(req, MAX_JSON).await?;
    Ok("chave válida".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::Vault;
    use axum::body::Bytes;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// method, path (with query), headers, body text
    type Seen = Arc<Mutex<Vec<(String, String, HeaderMap, String)>>>;

    /// Mock provider on 127.0.0.1:0. Answers each request in order with the next reply (status, body); "BASE" in a
    /// reply becomes the mock's own address. Every request is recorded.
    async fn serve(replies: Vec<(u16, Vec<u8>)>) -> (String, Seen) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen: Seen = Arc::default();
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let (rec, b) = (seen.clone(), base.clone());
        let app = axum::Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
            let (rec, queue, b) = (rec.clone(), queue.clone(), b.clone());
            async move {
                lk(&rec).push((method.to_string(), uri.to_string(), headers, String::from_utf8_lossy(&body).into_owned()));
                let (status, reply) = lk(&queue).pop_front().unwrap_or((500, b"sem resposta".to_vec()));
                let reply = String::from_utf8(reply.clone()).map(|t| t.replace("BASE", &b).into_bytes()).unwrap_or(reply);
                (StatusCode::from_u16(status).unwrap(), reply)
            }
        });
        tokio::spawn(async move { axum::serve(listener, app).await });
        (base, seen)
    }

    fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        crate::util::lk(m)
    }

    fn state(dir: &Path) -> Arc<AppState> {
        let s = AppState::new(dir.into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        s.set_vault_for_tests(Vault::with_key(&dir.join("vault.json"), [3u8; 32]));
        s
    }

    #[test]
    fn out_name_is_a_safe_slug_with_a_timestamp() {
        let n = out_name("Um gato, em uma praia!", "png");
        let re = regex::Regex::new(r"^um-gato-em-uma-praia-\d{8}-\d{6}\.png$").unwrap();
        assert!(re.is_match(&n), "{n}");
        let odd = out_name("../../etc/passwd ação ", "mp4");
        assert!(odd.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.')), "{odd}");
        assert!(out_name("   ", "mp3").starts_with("midia-"));
        assert!(out_name(&"x".repeat(200), "mp3").len() <= 40 + 1 + 15 + 4);
    }

    #[test]
    fn allowlist_blocks_other_providers_and_models() {
        assert!(check_allowlist(None, "google-image", "qualquer").is_ok());
        let a = MediaAllow { providers: vec!["google-image".into()], models: vec!["gemini-2.5-flash-image".into()] };
        assert!(check_allowlist(Some(&a), "google-image", "gemini-2.5-flash-image").is_ok());
        assert_eq!(check_allowlist(Some(&a), "openai-image", "gpt-image-1").unwrap_err().code(), "media_blocked");
        assert_eq!(check_allowlist(Some(&a), "google-image", "outro").unwrap_err().code(), "media_blocked");
        assert!(check_allowlist(Some(&MediaAllow::default()), "elevenlabs", "voz").is_ok());
    }

    #[test]
    fn config_refuses_plain_http_endpoints_and_unknown_switches() {
        let d = tempfile::tempdir().unwrap();
        let mut c = MediaConfig::default();
        c.base_urls.insert("google".into(), "http://generativelanguage.exemplo.com".into());
        assert_eq!(save(d.path(), &c).unwrap_err().code(), "bad_media_url");
        c.base_urls.clear();
        c.base_urls.insert("google".into(), "http://127.0.0.1:9".into());
        save(d.path(), &c).unwrap();
        c.enabled = vec!["piano".into()];
        assert_eq!(save(d.path(), &c).unwrap_err().code(), "bad_provider");
    }

    #[test]
    fn read_ref_accepts_only_images_and_caps_size_by_extension() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.exe");
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(read_ref(&p).unwrap_err().code(), "bad_reference");
        let p = d.path().join("a.PNG");
        std::fs::write(&p, b"\x89PNG").unwrap();
        assert_eq!(read_ref(&p).unwrap().mime, "image/png");
    }

    #[test]
    fn missing_key_names_the_key() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let e = rt.block_on(image_generate(&s, ImageReq { prompt: "x".into(), aspect: None, reference: None, provider: None, model: None }, d.path(), None)).unwrap_err();
        assert_eq!(e.code(), "missing_secret");
        assert!(e.to_string().contains("media.google"), "{e}");
    }

    #[test]
    fn write_out_refuses_bad_names_and_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(write_out(d.path(), "../fora.png", b"x").unwrap_err().code(), "bad_name");
        let a = write_out(d.path(), "foto.png", b"1").unwrap();
        let b = write_out(d.path(), "foto.png", b"2").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&b).unwrap(), b"2");
    }

    #[tokio::test]
    async fn google_inline_data_comes_back_as_the_png_bytes() {
        let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let reply = json!({"candidates": [{"content": {"parts": [{"text": "ok"}, {"inlineData": {"mimeType": "image/png", "data": B64.encode(&png)}}]}}]});
        let (base, seen) = serve(vec![(200, reply.to_string().into_bytes())]).await;
        let got = google_image(client().unwrap(), &base, "chave-g", "gemini-2.5-flash-image", "um gato", Some("16:9"), None).await.unwrap();
        assert_eq!(got, png);
        {
            let reqs = lk(&seen);
            assert_eq!(reqs[0].1, "/v1beta/models/gemini-2.5-flash-image:generateContent");
            assert_eq!(reqs[0].2["x-goog-api-key"].to_str().unwrap(), "chave-g");
            assert!(reqs[0].3.contains("\"responseModalities\":[\"IMAGE\"]"), "{}", reqs[0].3);
            assert!(reqs[0].3.contains("\"aspectRatio\":\"16:9\""), "{}", reqs[0].3);
        }
        let d = tempfile::tempdir().unwrap();
        let p = write_out(d.path(), &out_name("um gato", "png"), &got).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), png);
    }

    #[tokio::test]
    async fn openai_b64_json_for_generation_and_multipart_edit_with_reference() {
        let png = b"\x89PNG-openai".to_vec();
        let reply = json!({"data": [{"b64_json": B64.encode(&png)}]}).to_string().into_bytes();
        let (base, seen) = serve(vec![(200, reply.clone()), (200, reply)]).await;
        let got = openai_image(client().unwrap(), &base, "chave-o", "gpt-image-1", "um gato", Some("16:9"), None).await.unwrap();
        assert_eq!(got, png);
        let r = Ref { mime: "image/png".into(), bytes: vec![1, 2, 3] };
        openai_image(client().unwrap(), &base, "chave-o", "gpt-image-1", "outro", None, Some(&r)).await.unwrap();
        let reqs = lk(&seen);
        assert_eq!(reqs[0].1, "/v1/images/generations");
        assert_eq!(reqs[0].2["authorization"].to_str().unwrap(), "Bearer chave-o");
        assert!(reqs[0].3.contains("\"size\":\"1536x1024\""), "{}", reqs[0].3);
        assert_eq!(reqs[1].1, "/v1/images/edits");
        assert!(reqs[1].3.contains("name=\"image\""), "{}", reqs[1].3);
        assert!(reqs[1].3.contains("name=\"size\"\r\n\r\n1024x1024"), "{}", reqs[1].3);
    }

    #[tokio::test]
    async fn replicate_starting_processing_succeeded_then_download() {
        let mp4 = b"\0\0\0 ftypmp42-fake".to_vec();
        let pred = |status: &str| json!({"id": "p1", "status": status, "urls": {"get": "BASE/v1/predictions/p1"}}).to_string().into_bytes();
        let done = json!({"id": "p1", "status": "succeeded", "output": ["BASE/files/out.mp4"], "urls": {"get": "BASE/v1/predictions/p1"}}).to_string().into_bytes();
        let (base, seen) = serve(vec![(201, pred("starting")), (200, pred("processing")), (200, done), (200, mp4.clone())]).await;
        let got = replicate_video(client().unwrap(), &base, "chave-r", "kwaivgi/kling-v2.1", "um mar", None, None, Duration::from_millis(1), Duration::from_secs(30)).await.unwrap();
        assert_eq!(got, mp4);
        let reqs = lk(&seen);
        assert_eq!(reqs.len(), 4);
        assert_eq!(reqs[0].1, "/v1/models/kwaivgi/kling-v2.1/predictions");
        assert_eq!(reqs[0].2["prefer"].to_str().unwrap(), "wait");
        assert!(reqs[0].3.contains("\"prompt\":\"um mar\""), "{}", reqs[0].3);
        assert_eq!(reqs[1].1, "/v1/predictions/p1");
        assert_eq!(reqs[3].1, "/files/out.mp4");
        // the download goes out without the key
        assert!(reqs[3].2.get("authorization").is_none());
    }

    #[tokio::test]
    async fn replicate_failed_prediction_is_an_error() {
        let (base, _seen) = serve(vec![(201, json!({"id": "p2", "status": "failed", "urls": {"get": "BASE/v1/predictions/p2"}}).to_string().into_bytes())]).await;
        let e = replicate_video(client().unwrap(), &base, "k", "a/b", "x", None, None, Duration::from_millis(1), Duration::from_secs(30)).await.unwrap_err();
        assert_eq!(e.code(), "media_failed");
    }

    #[tokio::test]
    async fn elevenlabs_returns_mp3_bytes_for_a_voice() {
        let mp3 = b"ID3-fake".to_vec();
        let (base, seen) = serve(vec![(200, mp3.clone())]).await;
        let got = elevenlabs_audio(client().unwrap(), &base, "chave-e", "voz", "olá", "VOZ123").await.unwrap();
        assert_eq!(got, mp3);
        let reqs = lk(&seen);
        assert_eq!(reqs[0].1, "/v1/text-to-speech/VOZ123");
        assert_eq!(reqs[0].2["xi-api-key"].to_str().unwrap(), "chave-e");
        assert!(reqs[0].3.contains("eleven_multilingual_v2"));
    }

    #[tokio::test]
    async fn elevenlabs_rejects_a_voice_id_that_could_change_the_path() {
        let (base, seen) = serve(vec![]).await;
        let e = elevenlabs_audio(client().unwrap(), &base, "k", "voz", "x", "../outra").await.unwrap_err();
        assert_eq!(e.code(), "bad_voice");
        assert!(lk(&seen).is_empty());
    }

    fn keyed_state(dir: &Path, keys: &[(&str, &str)]) -> Arc<AppState> {
        let s = AppState::new(dir.into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        let v = Vault::with_key(&dir.join("vault.json"), [3u8; 32]);
        for (k, val) in keys {
            v.set(k, val).unwrap();
        }
        s.set_vault_for_tests(v);
        s
    }

    #[tokio::test]
    async fn testar_checks_each_key_with_a_free_read_and_generates_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (base, seen) = serve(vec![(200, b"{}".to_vec()); 4]).await;
        let s = keyed_state(d.path(), &[("media.google", "g"), ("media.openai", "o"), ("media.replicate", "r"), ("media.elevenlabs", "e")]);
        let mut cfg = MediaConfig::default();
        for p in ["google", "openai", "replicate", "elevenlabs"] {
            cfg.base_urls.insert(p.into(), base.clone());
        }
        save(d.path(), &cfg).unwrap();
        for p in ["google", "openai", "replicate", "elevenlabs"] {
            assert_eq!(test(&s, p).await.unwrap(), "chave válida");
        }
        let reqs = lk(&seen);
        assert_eq!(reqs.len(), 4);
        assert_eq!(reqs[0].0, "GET");
        assert_eq!(reqs[0].1, "/v1beta/models?pageSize=1");
        assert_eq!(reqs[0].2["x-goog-api-key"].to_str().unwrap(), "g");
        assert_eq!(reqs[1].1, "/v1/models");
        assert_eq!(reqs[1].2["authorization"].to_str().unwrap(), "Bearer o");
        assert_eq!(reqs[2].1, "/v1/account");
        assert_eq!(reqs[3].1, "/v1/user");
        assert_eq!(reqs[3].2["xi-api-key"].to_str().unwrap(), "e");
        assert!(!d.path().join("media-teste").exists());
    }

    #[tokio::test]
    async fn replicate_poll_refuses_a_look_alike_host_before_sending_the_key() {
        let reply = json!({"id": "p", "status": "starting", "urls": {"get": "https://api.replicate.com.evil.tld/v1/predictions/p"}});
        let (base, seen) = serve(vec![(201, reply.to_string().into_bytes())]).await;
        let e = replicate_video(client().unwrap(), &base, "k", "a/b", "x", None, None, Duration::from_millis(1), Duration::from_secs(30)).await.unwrap_err();
        assert_eq!(e.code(), "media_failed");
        // only the prediction POST went out: the poll to the look-alike never did
        assert_eq!(lk(&seen).len(), 1);
    }

    #[test]
    fn same_origin_compares_scheme_host_and_port() {
        let base = "https://api.replicate.com";
        assert!(same_origin("https://api.replicate.com/v1/predictions/1", base));
        assert!(!same_origin("https://api.replicate.com.evil.tld/v1/predictions/1", base));
        assert!(!same_origin("http://api.replicate.com/v1/predictions/1", base));
        assert!(!same_origin("https://api.replicate.com:8443/v1/predictions/1", base));
        assert!(!same_origin("https://evil.tld/https://api.replicate.com", base));
    }

    #[test]
    fn downloads_need_https_unless_both_ends_are_loopback() {
        assert!(check_download("https://replicate.delivery/x.mp4", "https://api.replicate.com").is_ok());
        assert!(check_download("http://replicate.delivery/x.mp4", "https://api.replicate.com").is_err());
        assert!(check_download("http://127.0.0.1:9/x.mp4", "https://api.replicate.com").is_err());
        assert!(check_download("http://127.0.0.1:9/x.mp4", "http://127.0.0.1:8").is_ok());
    }

    #[test]
    fn spending_caps_and_one_generation_at_a_time() {
        let _serial = SLOT_AUDIO_TESTES.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let s = keyed_state(d.path(), &[]);
        let mut cfg = MediaConfig::default();
        cfg.max_per_day.insert("audio".into(), 2);
        cfg.max_per_mission.insert("audio".into(), 1);
        set_config(&s, &cfg).unwrap();
        // one generation of a kind at a time: the slot is held while the ticket lives
        let t1 = acquire(&s, "audio", Some("m1")).unwrap_or_else(|e| panic!("primeiro acquire: {} {e}", e.code()));
        assert_eq!(acquire(&s, "audio", Some("m2")).unwrap_err().code(), "media_busy");
        drop(t1);
        // the mission m1 already spent its one: refused, and the slot comes back even on refusal
        assert_eq!(acquire(&s, "audio", Some("m1")).unwrap_err().code(), "media_cap_reached");
        let t2 = acquire(&s, "audio", Some("m2")).unwrap();
        drop(t2);
        // two attempts today: the daily cap refuses the third, in any mission
        assert_eq!(acquire(&s, "audio", Some("m3")).unwrap_err().code(), "media_cap_reached");
        assert_eq!(s.db.media_attempts("audio", start_of_today_ms(), None).unwrap(), 2);
        // the caps are for MCP calls only; usage reports them
        let u = usage_today(&s).unwrap();
        assert_eq!(u[2]["today"], 2);
        assert_eq!(u[2]["perDay"], 2);
    }

    /// Os slots de geração são globais ao processo: testes que seguram o de áudio rodam um de cada vez.
    static SLOT_AUDIO_TESTES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// O `gate` do MCP segura o slot de áudio durante a geração: a voz local não pode pedir o slot de novo.
    #[tokio::test]
    async fn voz_local_pelo_mcp_nao_disputa_o_slot_do_gate() {
        let _serial = SLOT_AUDIO_TESTES.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let s = keyed_state(d.path(), &[]);
        let gate = acquire(&s, "audio", None).unwrap();
        let e = audio_local(&s, "oi", None, d.path()).await.unwrap_err();
        assert_ne!(e.code(), "media_busy", "{e}");
        drop(gate);
    }

    #[tokio::test]
    async fn voz_sem_local_nem_elevenlabs_pede_a_voz_local() {
        let d = tempfile::tempdir().unwrap();
        let s = keyed_state(d.path(), &[]);
        let e = audio_generate(&s, "oi", "voz", None, d.path(), None).await.unwrap_err();
        assert_eq!(e.code(), "voz_local_ausente");
        assert!(e.to_string().contains("Configurações → Voz"));
    }

    #[tokio::test]
    async fn efeito_sem_chave_mantem_o_erro_de_chave() {
        let d = tempfile::tempdir().unwrap();
        let s = keyed_state(d.path(), &[]);
        assert_eq!(audio_generate(&s, "trovão", "efeito", None, d.path(), None).await.unwrap_err().code(), "missing_secret");
    }

    #[tokio::test]
    async fn voz_sem_chave_do_elevenlabs_cai_no_erro_da_voz_local() {
        let d = tempfile::tempdir().unwrap();
        let s = keyed_state(d.path(), &[("media.elevenlabs", "e")]);
        let mut cfg = MediaConfig::default();
        cfg.enabled.retain(|k| k != "elevenlabs");
        set_config(&s, &cfg).unwrap();
        assert_eq!(audio_generate(&s, "oi", "voz", None, d.path(), None).await.unwrap_err().code(), "voz_local_ausente");
    }

    #[test]
    fn speech_text_is_capped_at_a_word_boundary() {
        assert_eq!(cap_speech("  curto  "), "curto");
        let long = "palavra ".repeat(300);
        let capped = cap_speech(&long);
        assert!(capped.chars().count() <= SPEECH_MAX_CHARS);
        assert!(capped.ends_with("palavra"), "{capped:?}");
        // no space near the end: a hard cut at the limit
        let no_space = "a".repeat(SPEECH_MAX_CHARS + 50);
        assert_eq!(cap_speech(&no_space).chars().count(), SPEECH_MAX_CHARS);
    }
}
