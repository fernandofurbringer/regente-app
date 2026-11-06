//! Shot (F3): captura do monitor sob o cursor, região com overlay, gravação de região com ffmpeg e extração de quadros.
//! Tudo começa com o usuário: atalho global (desligado por padrão, `shot.json`) ou botão. A captura pelo agente (MCP
//! `shot_capture`) exige `agentCapture` ligado. As capturas vão para `Imagens\Regente Capturas`. Os cortes, nomes, listagem,
//! caminho e os argumentos do ffmpeg são puros e testados em pastas temporárias; a captura, a gravação real e o ffmpeg
//! de verdade são `#[ignore]`. A gravação roda só enquanto a barra "Parar" está aberta, e tem tempo máximo.

use crate::dictation::parse_hotkey;
use crate::error::{fail, AppError, AppResult};
use crate::media::write_out;
use crate::runtime::AppState;
use crate::settings::{load_or_default, save_json};
use crate::util::{self, home_dir, lk};
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};
use tauri_plugin_notification::NotificationExt;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ShotConfig {
    /// Atalhos globais de captura. Desligados por padrão.
    pub hotkeys: bool,
    pub full_key: String,
    pub region_key: String,
    pub record_key: String,
    /// Deixa o agente (MCP `shot_capture`) capturar a tela. Desligado por padrão.
    pub agent_capture: bool,
}

impl Default for ShotConfig {
    fn default() -> Self {
        Self {
            hotkeys: false,
            full_key: "Ctrl+Shift+3".into(),
            region_key: "Ctrl+Shift+4".into(),
            record_key: "Ctrl+Shift+5".into(),
            agent_capture: false,
        }
    }
}

pub fn load(data: &Path) -> ShotConfig {
    load_or_default(&data.join("shot.json"))
}

/// Writes the config file only. Use `apply` to change the config (it checks and registers the hotkeys first).
fn save(data: &Path, c: &ShotConfig) -> AppResult<()> {
    std::fs::create_dir_all(data)?;
    save_json(&data.join("shot.json"), c)
}

/// The three keys must parse, differ, and not be Esc (reserved to cancel) or the dictation hotkey.
/// The Shot keys: distinct, not Esc, and not the dictation key (`dictation`, when global dictation is on).
pub(crate) fn validate(c: &ShotConfig, dictation: Option<&str>) -> AppResult<()> {
    let esc = Shortcut::new(None, Code::Escape);
    let dict = dictation.and_then(|d| parse_hotkey(d).ok());
    let mut seen: Vec<Shortcut> = Vec::new();
    for text in [&c.full_key, &c.region_key, &c.record_key] {
        let sc = parse_hotkey(text)?;
        if seen.contains(&sc) {
            return fail("bad_hotkey", "dois atalhos de captura são iguais");
        }
        if sc == esc {
            return fail("bad_hotkey", "Esc é reservado para cancelar a captura");
        }
        if Some(sc) == dict {
            return fail("bad_hotkey", "este atalho já é o do ditado por voz");
        }
        seen.push(sc);
    }
    Ok(())
}

/// The config in use, read from disk once and cached (the hotkey handler reads it on every key event).
pub fn current(app: &AppHandle, data: &Path) -> ShotConfig {
    let shot = app.state::<Shot>();
    let mut cache = lk(&shot.config);
    cache.get_or_insert_with(|| load(data)).clone()
}

/// Startup: loads the config, caches it and registers its hotkeys.
pub fn init(app: &AppHandle, data: &Path) -> AppResult<()> {
    let cfg = load(data);
    *lk(&app.state::<Shot>().config) = Some(cfg.clone());
    sync_hotkeys(app, &cfg)
}

/// Settings → Shot: checks the keys, registers them, and only then saves. When the registration fails (another app
/// owns a key), the previous hotkeys come back and the file keeps the previous config.
pub fn apply(app: &AppHandle, data: &Path, cfg: &ShotConfig) -> AppResult<()> {
    let dictation = crate::stt::load(data).global_hotkey;
    validate(cfg, Some(&dictation))?;
    let prev = current(app, data);
    if let Err(e) = sync_hotkeys(app, cfg) {
        let _ = sync_hotkeys(app, &prev);
        return Err(e);
    }
    save(data, cfg)?;
    *lk(&app.state::<Shot>().config) = Some(cfg.clone());
    Ok(())
}

/// The agent may capture only when the user allowed it in Settings → Shot. Read from memory (loaded at startup, set by
/// `shot_save`): a pane that rewrites shot.json on disk changes nothing.
pub fn agent_allowed(s: &crate::runtime::AppState) -> AppResult<()> {
    if lk(&s.guarded).agent_capture {
        Ok(())
    } else {
        fail("agent_capture_off", "captura pelo agente desligada em Configurações → Captura de tela")
    }
}

/// `p` without the `\\?\` prefix that canonicalize adds on Windows (ffmpeg and the UI take plain paths).
fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

/// `Imagens\Regente Capturas` in the user profile, created now.
pub fn shots_dir() -> PathBuf {
    let dir = home_dir().join("Pictures").join("Regente Capturas");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::canonicalize(&dir).map(plain).unwrap_or(dir)
}

/// `p` canonicalized, and only when it stays inside `root` (a symlink or `..` cannot leave the folder).
pub fn inside(root: &Path, p: &Path) -> AppResult<PathBuf> {
    let root = std::fs::canonicalize(root)?;
    let path = std::fs::canonicalize(p).map_err(|_| AppError::new("not_found", "arquivo não existe"))?;
    if !path.starts_with(&root) {
        return fail("outside_shots", "o arquivo fica fora da pasta de capturas");
    }
    Ok(plain(path))
}

fn cap_err(e: impl std::fmt::Display) -> AppError {
    AppError::new("capture_failed", e.to_string())
}

/// The cursor position in screen pixels (0,0 when the call fails, which only picks the primary monitor).
#[cfg(windows)]
fn cursor_pos() -> (i32, i32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: GetCursorPos writes one POINT into the valid, live `p`.
    unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

#[cfg(not(windows))]
fn cursor_pos() -> (i32, i32) {
    (0, 0)
}

/// The monitor that holds the cursor, its image, and its origin on the virtual screen (region offsets need it).
pub fn capture_screen_under_cursor() -> AppResult<(RgbaImage, (i32, i32))> {
    let (x, y) = cursor_pos();
    let m = xcap::Monitor::from_point(x, y).map_err(cap_err)?;
    let img = m.capture_image().map_err(cap_err)?;
    Ok((img, (m.x().map_err(cap_err)?, m.y().map_err(cap_err)?)))
}

/// The rectangle `x,y,w,h` of `img`. It must be non-empty and fully inside the image.
pub fn crop(img: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> AppResult<RgbaImage> {
    let fits_x = x.checked_add(w).is_some_and(|r| r <= img.width());
    let fits_y = y.checked_add(h).is_some_and(|b| b <= img.height());
    if w == 0 || h == 0 || !fits_x || !fits_y {
        return fail("bad_region", "região fora da imagem");
    }
    Ok(image::imageops::crop_imm(img, x, y, w, h).to_image())
}

fn png_bytes(img: &RgbaImage) -> AppResult<Vec<u8>> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8)
        .map_err(cap_err)?;
    Ok(bytes)
}

/// Writes `shot-<yyyymmdd-hhmmss-mmm>.png` into `dir` (a taken name gets a counter; nothing is overwritten).
pub fn save_png(img: &RgbaImage, dir: &Path) -> AppResult<PathBuf> {
    let name = format!("shot-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f"));
    write_out(dir, &name, &png_bytes(img)?)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShotInfo {
    pub path: String,
    pub name: String,
    /// png | mp4 | frames
    pub kind: &'static str,
}

/// The captures in `dir` (names `shot-*`), newest first. The names carry the timestamp, so the order is by name.
pub fn list(dir: &Path) -> Vec<ShotInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<ShotInfo> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("shot-") {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir();
            let ext = Path::new(&name).extension().and_then(|x| x.to_str()).unwrap_or("").to_string();
            let kind = match (is_dir, ext.as_str()) {
                (true, _) if name.ends_with("-frames") => "frames",
                (false, "png") if !name.ends_with(".orig.png") => "png",
                (false, "mp4") => "mp4",
                _ => return None,
            };
            Some(ShotInfo { path: e.path().to_string_lossy().to_string(), name, kind })
        })
        .collect();
    out.sort_by(|a, b| b.name.cmp(&a.name));
    out
}

/// What a region pick does with the frozen frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Region,
    Record,
}

struct Pending {
    img: RgbaImage,
    origin: (i32, i32),
    mode: Mode,
}

/// A recording in progress: the ffmpeg child (a `q` on its stdin ends it) and the file it writes.
struct Recording {
    child: Child,
    out: PathBuf,
}

/// Managed state: the frame waiting for the overlay's region pick, the registered hotkeys, the recording in progress,
/// and the cached config.
#[derive(Default)]
pub struct Shot {
    pending: Mutex<Option<Pending>>,
    registered: Mutex<Vec<Shortcut>>,
    recording: Mutex<Option<Recording>>,
    config: Mutex<Option<ShotConfig>>,
}

/// Serializes every edit save of the process: two saves of one capture never interleave.
static EDIT_LOCK: Mutex<()> = Mutex::new(());
/// Videos being turned into frames right now (one extraction per video at a time).
static EXTRACTING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Captures the monitor under the cursor into the shots folder and tells the user.
pub fn capture_full(app: &AppHandle) -> AppResult<PathBuf> {
    let (img, _) = capture_screen_under_cursor()?;
    let path = save_png(&img, &shots_dir())?;
    announce(app, &path);
    Ok(path)
}

/// Agent capture: the monitor (or a region of it), saved into `dir` with no overlay and no user pick.
pub fn capture_for_agent(dir: &Path, region: Option<(u32, u32, u32, u32)>) -> AppResult<PathBuf> {
    let (img, _) = capture_screen_under_cursor()?;
    let img = match region {
        Some((x, y, w, h)) => crop(&img, x, y, w, h)?,
        None => img,
    };
    save_png(&img, dir)
}

/// "Captura salva" as a notification and as an event the app turns into a toast.
fn announce(app: &AppHandle, path: &Path) {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let _ = app.notification().builder().title("Captura salva").body(name.as_str()).show();
    if let Some(s) = app.try_state::<Arc<AppState>>() {
        s.emit("shot-saved", json!({"path": path.to_string_lossy(), "name": name}));
    }
}

fn close_overlay(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("shot-overlay") {
        let _ = w.destroy();
    }
}

/// Freezes the monitor under the cursor and opens the overlay (fullscreen, always on top) over it. Any failure clears
/// the pending frame and the overlay, so nothing is left half-open.
pub fn begin(app: &AppHandle, mode: Mode) -> AppResult<()> {
    if mode == Mode::Record && ffmpeg_path().is_none() {
        return Err(missing_ffmpeg());
    }
    let (img, origin) = capture_screen_under_cursor()?;
    let (w, h) = img.dimensions();
    *lk(&app.state::<Shot>().pending) = Some(Pending { img, origin, mode });
    close_overlay(app);
    let opened = open_overlay(app, mode, origin, (w, h));
    if opened.is_err() {
        *lk(&app.state::<Shot>().pending) = None;
        close_overlay(app);
    }
    opened
}

fn open_overlay(app: &AppHandle, mode: Mode, origin: (i32, i32), size: (u32, u32)) -> AppResult<()> {
    let route = match mode {
        Mode::Region => "index.html#/shot-overlay",
        Mode::Record => "index.html#/shot-overlay/record",
    };
    let win = WebviewWindowBuilder::new(app, "shot-overlay", WebviewUrl::App(route.into()))
        .title("Regente Captura")
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .build()
        .map_err(|e| AppError::new("overlay_failed", e.to_string()))?;
    let fail_win = |e: tauri::Error| AppError::new("overlay_failed", e.to_string());
    win.set_position(PhysicalPosition::new(origin.0, origin.1)).map_err(fail_win)?;
    win.set_size(PhysicalSize::new(size.0, size.1)).map_err(fail_win)?;
    win.set_fullscreen(true).map_err(fail_win)?;
    Ok(())
}

/// The overlay's region: crops the frozen frame, then saves it (region) or starts recording it (record mode).
/// The overlay closes either way.
pub fn region_commit(app: &AppHandle, x: u32, y: u32, w: u32, h: u32) -> AppResult<Option<PathBuf>> {
    let pending = lk(&app.state::<Shot>().pending).take();
    close_overlay(app);
    let Some(p) = pending else { return fail("no_capture", "nenhuma captura pendente") };
    match p.mode {
        Mode::Region => {
            let path = save_png(&crop(&p.img, x, y, w, h)?, &shots_dir())?;
            announce(app, &path);
            Ok(Some(path))
        }
        Mode::Record => {
            crop(&p.img, x, y, w, h)?; // the region must lie inside the monitor
            let (mw, mh) = p.img.dimensions();
            Ok(Some(start_recording(app, p.origin, (x, y, w, h), (mw, mh))?))
        }
    }
}

pub fn region_cancel(app: &AppHandle) {
    *lk(&app.state::<Shot>().pending) = None;
    close_overlay(app);
}

/// The frozen frame as PNG, for the overlay's background.
pub fn overlay_png(app: &AppHandle) -> AppResult<Vec<u8>> {
    let shot = app.state::<Shot>();
    let guard = lk(&shot.pending);
    match guard.as_ref() {
        Some(p) => png_bytes(&p.img),
        None => fail("no_capture", "nenhuma captura pendente"),
    }
}

/// Registers the three hotkeys when the user turned them on (drops the previous ones first). Called on save and at startup.
pub fn sync_hotkeys(app: &AppHandle, cfg: &ShotConfig) -> AppResult<()> {
    let shot = app.state::<Shot>();
    let gs = app.global_shortcut();
    let old: Vec<Shortcut> = lk(&shot.registered).drain(..).collect();
    for sc in old {
        let _ = gs.unregister(sc);
    }
    if !cfg.hotkeys {
        return Ok(());
    }
    for text in [&cfg.full_key, &cfg.region_key, &cfg.record_key] {
        let sc = parse_hotkey(text)?;
        gs.register(sc).map_err(|e| AppError::new("hotkey_busy", format!("o atalho {text} não está disponível (em uso por outro app?): {e}")))?;
        lk(&shot.registered).push(sc);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Full,
    Region,
    Record,
}

/// The plugin's handler for Shot's keys. Returns true when the key is one of them (the dictation handler then stops).
pub fn on_shortcut(app: &AppHandle, sc: &Shortcut, pressed: bool) -> bool {
    let Some(s) = app.try_state::<Arc<AppState>>() else { return false };
    let cfg = current(app, &s.data);
    if !cfg.hotkeys {
        return false;
    }
    let is = |text: &str| parse_hotkey(text).ok().as_ref() == Some(sc);
    let action = if is(&cfg.full_key) {
        Action::Full
    } else if is(&cfg.region_key) {
        Action::Region
    } else if is(&cfg.record_key) {
        Action::Record
    } else {
        return false;
    };
    if pressed {
        run(app, action);
    }
    true
}

/// Runs off the hotkey thread, so a capture never blocks the keyboard hook.
fn run(app: &AppHandle, action: Action) {
    let app = app.clone();
    std::thread::spawn(move || {
        let r = match action {
            Action::Full => capture_full(&app).map(|_| ()),
            Action::Region => begin(&app, Mode::Region),
            Action::Record => begin(&app, Mode::Record),
        };
        if let Err(e) = r {
            tracing::warn!("captura: {}", e.code());
            if let Some(s) = app.try_state::<Arc<AppState>>() {
                s.emit("shot-error", json!({"message": e.to_string()}));
            }
        }
    });
}

/// Largest edited PNG and largest layers document the editor may save.
const MAX_EDIT_PNG: usize = 50 * 1024 * 1024;
const MAX_LAYERS: usize = 1024 * 1024;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// `<file>.shot.json`: the editor's document (crop and layers), beside the PNG.
fn sidecar(png: &Path) -> PathBuf {
    let name = png.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    png.with_file_name(format!("{name}.shot.json"))
}

/// `<stem>.orig.png`: the capture as first taken. The editor re-opens from it, so the layers are never drawn twice.
fn orig_of(png: &Path) -> PathBuf {
    let stem = png.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    png.with_file_name(format!("{stem}.orig.png"))
}

/// What the editor opens: the image to draw on (the original once edited) and the saved document, if any.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditState {
    pub base: PathBuf,
    pub doc: Option<String>,
}

/// The editor's autosave: the rendered PNG replaces the original and the layers JSON goes beside it. Only a PNG inside
/// `root`, and only a valid JSON document, is written.
pub fn save_edit(root: &Path, path: &Path, png: &[u8], layers: &str) -> AppResult<()> {
    if path.extension().and_then(|e| e.to_str()) != Some("png") {
        return fail("bad_name", "só PNG pode ser editado");
    }
    if png.len() > MAX_EDIT_PNG || layers.len() > MAX_LAYERS {
        return fail("too_big", "edição grande demais");
    }
    if !png.starts_with(&PNG_SIGNATURE) {
        return fail("not_png", "a imagem não é PNG");
    }
    serde_json::from_str::<serde_json::Value>(layers).map_err(|_| AppError::new("bad_layers", "camadas inválidas"))?;
    let _serial = lk(&EDIT_LOCK);
    let p = inside(root, path)?;
    let orig = orig_of(&p);
    if !orig.exists() {
        keep_original(&p, &orig)?;
    }
    write_atomic(&p, png)?;
    write_atomic(&sidecar(&p), layers.as_bytes())?;
    Ok(())
}

/// Writes `bytes` to `dest` through a temp file in the same folder, renamed into place: a crash never leaves a
/// half-written capture or sidecar.
fn write_atomic(dest: &Path, bytes: &[u8]) -> AppResult<()> {
    let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = dest.with_file_name(format!(".{name}.{}.tmp", util::new_id()));
    let res = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.sync_all()
        })
        .and_then(|_| std::fs::rename(&tmp, dest));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(res?)
}

/// Keeps the capture as first taken. A hard link from a temp file creates `orig` only when it does not exist yet, so
/// an existing original is never touched and a crash never leaves a partial one.
fn keep_original(p: &Path, orig: &Path) -> AppResult<()> {
    let bytes = std::fs::read(p)?;
    let tmp = orig.with_file_name(format!(".orig-{}.tmp", util::new_id()));
    std::fs::write(&tmp, &bytes)?;
    let linked = std::fs::hard_link(&tmp, orig);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The editor's start state: the original image and the saved document (None when never edited).
pub fn edit_get(root: &Path, path: &Path) -> AppResult<EditState> {
    let p = inside(root, path)?;
    let orig = orig_of(&p);
    let base = if orig.exists() { orig } else { p.clone() };
    let doc = match std::fs::read_to_string(sidecar(&p)) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    Ok(EditState { base, doc })
}

/// Moves a capture (or a frames folder) to the Recycle Bin, with its layers file.
pub fn delete(root: &Path, path: &Path) -> AppResult<()> {
    let p = inside(root, path)?;
    if p == plain(std::fs::canonicalize(root)?) {
        return fail("bad_name", "a pasta de capturas não é apagada");
    }
    for extra in [sidecar(&p), orig_of(&p)] {
        if extra.exists() {
            crate::files::delete(&extra)?;
        }
    }
    crate::files::delete(&p)
}

// ---- ffmpeg: recording and frames -------------------------------------------------------------------------------

pub const RECORD_FPS: u32 = 30;
/// A recording stops by itself after this long (the user can stop it sooner).
const RECORD_MAX: Duration = Duration::from_secs(300);
/// After `q`, ffmpeg has this long to finish the file before it is killed.
const STOP_WAIT: Duration = Duration::from_secs(15);
const EXTRACT_MAX: Duration = Duration::from_secs(300);
/// The longest clip that can be turned into frames.
const CLIP_MAX: f64 = 300.0;
const SHEET_COLS: u32 = 4;
const SHEET_THUMB_W: u32 = 320;
/// The dark band over each frame of the contact sheet, where its number goes.
const BAND: u32 = 14;

fn missing_ffmpeg() -> AppError {
    AppError::new("ffmpeg_missing", "instale o ffmpeg (winget install Gyan.FFmpeg)")
}

pub fn ffmpeg_path() -> Option<String> {
    crate::providers::which("ffmpeg")
}

/// Args of the region recording (gdigrab, desktop). Width and height go down to even numbers, which yuv420p needs.
pub fn record_args(x: i32, y: i32, w: u32, h: u32, fps: u32, out: &Path) -> Vec<String> {
    let (w, h) = (w & !1, h & !1);
    let s = |v: &str| v.to_string();
    vec![
        s("-y"),
        s("-f"),
        s("gdigrab"),
        s("-framerate"),
        fps.to_string(),
        s("-offset_x"),
        x.to_string(),
        s("-offset_y"),
        y.to_string(),
        s("-video_size"),
        format!("{w}x{h}"),
        s("-i"),
        s("desktop"),
        s("-c:v"),
        s("libx264"),
        s("-preset"),
        s("veryfast"),
        s("-pix_fmt"),
        s("yuv420p"),
        // fragmented MP4: a file cut short (crash, kill) still plays up to the last fragment
        s("-movflags"),
        s("+frag_keyframe+empty_moov"),
        out.to_string_lossy().to_string(),
    ]
}

/// Physical size of the Stop bar (140×44 logical, with room for a 150% screen).
const BAR: (i32, i32) = (210, 66);

/// Where the Stop bar goes, in monitor pixels: the first corner (bottom-right, bottom-left, top-right, top-left) that
/// lies outside the recorded region; when none does, the top-left corner.
pub fn bar_position(monitor: (u32, u32), region: (u32, u32, u32, u32), bar: (i32, i32)) -> (i32, i32) {
    let m = 8;
    let (mw, mh) = (monitor.0 as i32, monitor.1 as i32);
    let (rx, ry, rw, rh) = (region.0 as i32, region.1 as i32, region.2 as i32, region.3 as i32);
    let corners = [(mw - bar.0 - m, mh - bar.1 - m), (m, mh - bar.1 - m), (mw - bar.0 - m, m), (m, m)];
    let hits = |(x, y): (i32, i32)| x < rx + rw && rx < x + bar.0 && y < ry + rh && ry < y + bar.1;
    corners.into_iter().find(|&c| c.0 >= 0 && c.1 >= 0 && !hits(c)).unwrap_or((m, m))
}

/// Args that pull the frames of `start..end` (seconds) out of a video, dropping duplicates, into `out_dir/%04d.png`.
pub fn frames_args(input: &Path, start: f64, end: f64, fps: f64, out_dir: &Path) -> Vec<String> {
    let s = |v: &str| v.to_string();
    vec![
        s("-y"),
        s("-ss"),
        format!("{start:.3}"),
        s("-to"),
        format!("{end:.3}"),
        s("-i"),
        input.to_string_lossy().to_string(),
        s("-vf"),
        format!("fps={fps},mpdecimate,setpts=N/FRAME_RATE/TB"),
        // ffmpeg 9 dropped -vsync (checked on this machine): -fps_mode is the same thing
        s("-fps_mode"),
        s("vfr"),
        out_dir.join("%04d.png").to_string_lossy().to_string(),
    ]
}

/// 3×5 pixel digits for the frame numbers in the contact sheet.
const DIGITS: [[&str; 5]; 10] = [
    ["111", "101", "101", "101", "111"],
    ["010", "110", "010", "010", "111"],
    ["111", "001", "111", "100", "111"],
    ["111", "001", "111", "001", "111"],
    ["101", "101", "111", "001", "001"],
    ["111", "100", "111", "001", "111"],
    ["111", "100", "111", "101", "111"],
    ["111", "001", "010", "010", "010"],
    ["111", "101", "111", "101", "111"],
    ["111", "101", "111", "001", "111"],
];

fn fill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: Rgba<u8>) {
    for yy in y..y.saturating_add(h).min(img.height()) {
        for xx in x..x.saturating_add(w).min(img.width()) {
            img.put_pixel(xx, yy, c);
        }
    }
}

/// Draws `n` in white, 2 px per digit pixel, at (x, y).
fn draw_number(img: &mut RgbaImage, x: u32, y: u32, n: usize) {
    let white = Rgba([255, 255, 255, 255]);
    let mut cx = x;
    for ch in n.to_string().chars() {
        let d = ch.to_digit(10).unwrap_or(0) as usize;
        for (row, bits) in DIGITS[d].iter().enumerate() {
            for (col, b) in bits.chars().enumerate() {
                if b == '1' {
                    fill(img, cx + col as u32 * 2, y + row as u32 * 2, 2, 2, white);
                }
            }
        }
        cx += 8;
    }
}

/// A grid of the frames, `cols` wide, every thumbnail `thumb_w` wide, each with a dark band carrying its number.
pub fn contact_sheet(frames: &[PathBuf], cols: u32, thumb_w: u32, out: &Path) -> AppResult<()> {
    if frames.is_empty() || cols == 0 || thumb_w == 0 {
        return fail("bad_sheet", "folha sem quadros");
    }
    let mut thumbs: Vec<RgbaImage> = Vec::with_capacity(frames.len());
    for f in frames {
        let im = image::open(f).map_err(cap_err)?.to_rgba8();
        let th = ((im.height() as u64 * thumb_w as u64) / im.width().max(1) as u64).max(1) as u32;
        thumbs.push(imageops::resize(&im, thumb_w, th, FilterType::Triangle));
    }
    let cell_h = BAND + thumbs.iter().map(|t| t.height()).max().unwrap_or(1);
    let cols = cols.min(thumbs.len() as u32);
    let rows = (thumbs.len() as u32).div_ceil(cols);
    let mut sheet = RgbaImage::from_pixel(cols * thumb_w, rows * cell_h, Rgba([24, 24, 24, 255]));
    for (i, t) in thumbs.iter().enumerate() {
        let (cx, cy) = ((i as u32 % cols) * thumb_w, (i as u32 / cols) * cell_h);
        imageops::overlay(&mut sheet, t, cx as i64, (cy + BAND) as i64);
        fill(&mut sheet, cx, cy, thumb_w, BAND, Rgba([0, 0, 0, 255]));
        draw_number(&mut sheet, cx + 4, cy + 3, i + 1);
    }
    sheet.save(out).map_err(cap_err)
}

/// Runs ffmpeg with no console window and a deadline: past `max` it is killed and the call fails.
fn run_ffmpeg(ffmpeg: &str, args: &[String], max: Duration) -> AppResult<()> {
    let mut child = util::cmd(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::new("ffmpeg_failed", format!("ffmpeg não iniciou: {e}")))?;
    let deadline = Instant::now() + max;
    loop {
        if let Some(st) = child.try_wait()? {
            return if st.success() { Ok(()) } else { fail("ffmpeg_failed", "o ffmpeg falhou") };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return fail("timeout", "o ffmpeg demorou demais");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The record window: a small always-on-top bar with "Parar", at the monitor's bottom-right corner (outside the region).
fn open_record_bar(app: &AppHandle, origin: (i32, i32), monitor: (u32, u32), region: (u32, u32, u32, u32)) -> AppResult<()> {
    if let Some(old) = app.get_webview_window("shot-record") {
        let _ = old.destroy();
    }
    let fail_win = |e: tauri::Error| AppError::new("overlay_failed", e.to_string());
    let win = WebviewWindowBuilder::new(app, "shot-record", WebviewUrl::App("index.html#/shot-record".into()))
        .title("Regente Captura: gravando")
        .inner_size(140.0, 44.0)
        .decorations(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .build()
        .map_err(fail_win)?;
    let (bx, by) = bar_position(monitor, region, BAR);
    win.set_position(PhysicalPosition::new(origin.0 + bx, origin.1 + by)).map_err(fail_win)?;
    // Alt+F4 on the bar stops the recording too: the file is finished, never left half-written
    let app2 = app.clone();
    win.on_window_event(move |ev| {
        if let WindowEvent::CloseRequested { api, .. } = ev {
            api.prevent_close();
            let app = app2.clone();
            std::thread::spawn(move || {
                let _ = stop_recording(&app);
            });
        }
    });
    Ok(())
}

/// Starts ffmpeg on the region (screen coordinates = monitor origin + region), opens the "Parar" bar and arms the
/// time limit. Refuses a second recording and a missing ffmpeg.
fn start_recording(app: &AppHandle, origin: (i32, i32), r: (u32, u32, u32, u32), monitor: (u32, u32)) -> AppResult<PathBuf> {
    let ffmpeg = ffmpeg_path().ok_or_else(missing_ffmpeg)?;
    let shot = app.state::<Shot>();
    // the lock covers the check, the spawn and the store: two recordings cannot race past the check
    let mut slot = lk(&shot.recording);
    if slot.is_some() {
        return fail("recording", "já há uma gravação em andamento");
    }
    let (x, y, w, h) = r;
    if w < 2 || h < 2 {
        return fail("bad_region", "região pequena demais para gravar");
    }
    let out = shots_dir().join(format!("shot-{}.mp4", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")));
    let args = record_args(origin.0 + x as i32, origin.1 + y as i32, w, h, RECORD_FPS, &out);
    // HighDpiAware: gdigrab then reads physical pixels, the same ones the overlay's region is measured in
    let mut child = util::cmd(&ffmpeg)
        .env("__COMPAT_LAYER", "HighDpiAware")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::new("ffmpeg_failed", format!("ffmpeg não iniciou: {e}")))?;
    std::thread::sleep(Duration::from_millis(400));
    if let Ok(Some(_)) = child.try_wait() {
        return fail("ffmpeg_failed", "o ffmpeg encerrou ao iniciar a gravação");
    }
    *slot = Some(Recording { child, out: out.clone() });
    if let Err(e) = open_record_bar(app, origin, monitor, r) {
        // no bar, no recording: the child is killed and nothing is announced
        if let Some(mut rec) = slot.take() {
            let _ = rec.child.kill();
            let _ = rec.child.wait();
        }
        return Err(e);
    }
    drop(slot);
    let app2 = app.clone();
    let out2 = out.clone();
    std::thread::spawn(move || {
        std::thread::sleep(RECORD_MAX);
        let same = lk(&app2.state::<Shot>().recording).as_ref().is_some_and(|rec| rec.out == out2);
        if same {
            let _ = stop_recording(&app2);
        }
    });
    Ok(out)
}

/// Ends the file: `q` on stdin, then wait; past the limit ffmpeg is killed. A file with no bytes is an error.
fn finish(mut rec: Recording) -> AppResult<PathBuf> {
    if let Some(mut stdin) = rec.child.stdin.take() {
        let _ = stdin.write_all(b"q");
    }
    let deadline = Instant::now() + STOP_WAIT;
    loop {
        match rec.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = rec.child.kill();
                let _ = rec.child.wait();
                break;
            }
        }
    }
    if std::fs::metadata(&rec.out).map(|m| m.len()).unwrap_or(0) == 0 {
        return fail("record_failed", "a gravação não gerou vídeo");
    }
    Ok(rec.out)
}

/// Stops the recording in progress, closes the bar and tells the app (it opens the trimmer).
pub fn stop_recording(app: &AppHandle) -> AppResult<PathBuf> {
    let rec = lk(&app.state::<Shot>().recording).take();
    let Some(rec) = rec else { return fail("not_recording", "nenhuma gravação em andamento") };
    if let Some(w) = app.get_webview_window("shot-record") {
        let _ = w.destroy();
    }
    let out = finish(rec)?;
    if let Some(s) = app.try_state::<Arc<AppState>>() {
        s.emit("shot-recorded", json!({"path": out.to_string_lossy()}));
    }
    Ok(out)
}

/// App exit: a recording still running is finished (`q`, at most `STOP_WAIT`), so the file is playable.
pub fn shutdown(app: &AppHandle) {
    if let Some(shot) = app.try_state::<Shot>() {
        let rec = lk(&shot.recording).take();
        if let Some(rec) = rec {
            if let Err(e) = finish(rec) {
                tracing::warn!("gravação ao sair: {}", e.code());
            }
        }
    }
}

/// Holds a video's slot in `EXTRACTING` until the extraction ends (also on an error).
struct ExtractGuard(PathBuf);

impl ExtractGuard {
    fn take(video: &Path) -> AppResult<Self> {
        let mut busy = lk(&EXTRACTING);
        if !busy.insert(video.to_path_buf()) {
            return fail("extract_busy", "este vídeo já está sendo extraído");
        }
        Ok(Self(video.to_path_buf()))
    }
}

impl Drop for ExtractGuard {
    fn drop(&mut self) {
        lk(&EXTRACTING).remove(&self.0);
    }
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Extracted {
    pub dir: PathBuf,
    pub frames: usize,
    pub contact: PathBuf,
}

/// Frames of `start..end` (seconds, at most 5 min) from a capture video, at 1 or 2 per second, without duplicates,
/// into `shot-<ts>-frames/` beside the video, with `contact.png`. A frames folder from an earlier run is replaced.
pub fn extract(root: &Path, video: &Path, start: f64, end: f64, fps: u32) -> AppResult<Extracted> {
    if !matches!(fps, 1 | 2) {
        return fail("bad_fps", "FPS de extração: 1 ou 2");
    }
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end - start < 0.1 || end - start > CLIP_MAX {
        return fail("bad_range", "intervalo inválido (até 5 minutos)");
    }
    let input = inside(root, video)?;
    // one extraction per video at a time: a second call would delete the folder the first is filling
    let _busy = ExtractGuard::take(&input)?;
    let stem = match (input.extension().and_then(|e| e.to_str()), input.file_stem().and_then(|s| s.to_str())) {
        (Some("mp4"), Some(s)) if s.starts_with("shot-") => s.to_string(),
        _ => return fail("bad_name", "só vídeo de captura (shot-*.mp4) pode ser extraído"),
    };
    let ffmpeg = ffmpeg_path().ok_or_else(missing_ffmpeg)?;
    let dir = root.join(format!("{stem}-frames"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    run_ffmpeg(&ffmpeg, &frames_args(&input, start, end, fps as f64, &dir), EXTRACT_MAX)?;
    let mut frames: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    frames.sort();
    if frames.is_empty() {
        return fail("no_frames", "nenhum quadro extraído deste intervalo");
    }
    let contact = dir.join("contact.png");
    contact_sheet(&frames, SHEET_COLS, SHEET_THUMB_W, &contact)?;
    Ok(Extracted { dir, frames: frames.len(), contact })
}

#[cfg(test)]
mod tests {
    #[test]
    fn dictation_key_cannot_equal_a_shot_key() {
        let cfg = ShotConfig { hotkeys: true, full_key: "Ctrl+Alt+P".into(), region_key: "Ctrl+Alt+R".into(), record_key: "Ctrl+Alt+G".into(), ..Default::default() };
        assert!(super::validate(&cfg, Some("Ctrl+Shift+Space")).is_ok());
        assert_eq!(super::validate(&cfg, Some("Ctrl+Alt+R")).unwrap_err().code(), "bad_hotkey", "the dictation key collides with the region key");
    }

    use super::*;

    fn img(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]))
    }

    #[test]
    fn crop_accepts_only_regions_inside_the_image() {
        let im = img(4, 4);
        assert_eq!(crop(&im, 0, 0, 4, 4).unwrap().dimensions(), (4, 4));
        assert_eq!(crop(&im, 1, 2, 3, 2).unwrap().dimensions(), (3, 2));
        assert_eq!(crop(&im, 1, 1, 4, 1).unwrap_err().code(), "bad_region");
        assert_eq!(crop(&im, 0, 0, 0, 1).unwrap_err().code(), "bad_region");
        assert_eq!(crop(&im, u32::MAX, 0, 2, 1).unwrap_err().code(), "bad_region");
    }

    #[test]
    fn save_png_writes_a_timestamped_png_that_decodes_back() {
        let d = tempfile::tempdir().unwrap();
        let p = save_png(&img(2, 3), d.path()).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        // shot-yyyymmdd-hhmmss-mmm.png
        assert_eq!(name.len(), 28, "{name}");
        assert!(name.starts_with("shot-") && name.ends_with(".png"));
        assert!(name[5..13].chars().all(|c| c.is_ascii_digit()) && name.as_bytes()[13] == b'-');
        let back = image::open(&p).unwrap();
        assert_eq!((back.width(), back.height()), (2, 3));
    }

    #[test]
    fn list_keeps_captures_newest_first_and_skips_other_files() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(dir.join("shot-20260101-000000-000.png"), b"x").unwrap();
        std::fs::write(dir.join("shot-20260301-000000-000.mp4"), b"x").unwrap();
        std::fs::create_dir(dir.join("shot-20260201-000000-000-frames")).unwrap();
        std::fs::write(dir.join("shot-20260101-000000-000.png.shot.json"), b"{}").unwrap();
        std::fs::write(dir.join("shot-20260101-000000-000.orig.png"), b"x").unwrap();
        std::fs::write(dir.join("notas.txt"), b"x").unwrap();
        let got: Vec<(String, &str)> = list(dir).into_iter().map(|i| (i.name, i.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("shot-20260301-000000-000.mp4".to_string(), "mp4"),
                ("shot-20260201-000000-000-frames".to_string(), "frames"),
                ("shot-20260101-000000-000.png".to_string(), "png"),
            ]
        );
    }

    #[test]
    fn paths_outside_the_shots_folder_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("shots");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();
        std::fs::write(d.path().join("fora.png"), b"x").unwrap();
        assert!(inside(&root, &root.join("a.png")).is_ok());
        assert_eq!(inside(&root, &d.path().join("fora.png")).unwrap_err().code(), "outside_shots");
        assert_eq!(inside(&root, &root.join("..").join("fora.png")).unwrap_err().code(), "outside_shots");
        assert_eq!(inside(&root, &root.join("nao.png")).unwrap_err().code(), "not_found");
    }

    #[test]
    fn defaults_are_valid_and_hotkeys_off() {
        let c = ShotConfig::default();
        assert!(validate(&c, None).is_ok());
        assert!(!c.hotkeys && !c.agent_capture);
        let dup = ShotConfig { region_key: "Ctrl+Shift+3".into(), ..ShotConfig::default() };
        assert_eq!(validate(&dup, None).unwrap_err().code(), "bad_hotkey");
        assert_eq!(validate(&ShotConfig { full_key: "Ctrl+Shift+Banana".into(), ..ShotConfig::default() }, None).unwrap_err().code(), "bad_hotkey");
    }

    #[test]
    fn hotkeys_never_collide_with_esc_or_the_dictation_key() {
        let esc = ShotConfig { full_key: "Escape".into(), ..ShotConfig::default() };
        assert_eq!(validate(&esc, None).unwrap_err().code(), "bad_hotkey");
        // the dictation hotkey (Ctrl+Shift+3 here) is taken: Shot may not reuse it
        assert_eq!(validate(&ShotConfig::default(), Some("Ctrl+Shift+3")).unwrap_err().code(), "bad_hotkey");
        assert!(validate(&ShotConfig::default(), Some("Ctrl+Shift+Space")).is_ok());
    }

    #[test]
    fn unc_shares_lose_the_verbatim_prefix_too() {
        assert_eq!(plain(PathBuf::from(r"\\?\UNC\srv\share\a.png")), PathBuf::from(r"\\srv\share\a.png"));
        assert_eq!(plain(PathBuf::from(r"\\?\C:\x\a.png")), PathBuf::from(r"C:\x\a.png"));
    }

    #[test]
    fn the_stop_bar_stays_outside_the_recorded_region() {
        let bar = (210, 66);
        // a region filling the bottom-right: the bar moves to the bottom-left
        assert_eq!(bar_position((1920, 1200), (1000, 800, 900, 390), bar), (8, 1126));
        // a full-screen region leaves no free corner: top-left
        assert_eq!(bar_position((1920, 1200), (0, 0, 1920, 1200), bar), (8, 8));
        // a small region in the middle: bottom-right is free
        assert_eq!(bar_position((1920, 1200), (500, 500, 200, 200), bar), (1702, 1126));
    }

    #[test]
    fn concurrent_edits_keep_the_original_and_a_valid_pair() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("shots");
        std::fs::create_dir_all(&root).unwrap();
        let shot = save_png(&img(2, 2), &root).unwrap();
        let original = std::fs::read(&shot).unwrap();
        let a = png_bytes(&img(3, 3)).unwrap();
        let b = png_bytes(&img(4, 4)).unwrap();
        let (r1, p1) = (root.clone(), shot.clone());
        let (r2, p2) = (root.clone(), shot.clone());
        let t1 = std::thread::spawn(move || save_edit(&r1, &p1, &a, r#"[{"type":"rect"}]"#));
        let t2 = std::thread::spawn(move || save_edit(&r2, &p2, &b, r#"{"crop":null,"layers":[]}"#));
        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();
        assert_eq!(std::fs::read(orig_of(&shot)).unwrap(), original, "the original is never replaced");
        assert!(image::open(&shot).is_ok(), "the capture is a whole PNG");
        let side: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(sidecar(&shot)).unwrap()).unwrap();
        assert!(side.is_array() || side.is_object());
        assert!(std::fs::read_dir(&root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")), "no temp files left");
    }

    #[test]
    fn a_video_is_extracted_by_one_call_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("shots");
        std::fs::create_dir_all(&root).unwrap();
        let v = root.join("shot-1.mp4");
        std::fs::write(&v, b"x").unwrap();
        let held = ExtractGuard::take(&std::fs::canonicalize(&v).map(plain).unwrap()).unwrap();
        assert_eq!(extract(&root, &v, 0.0, 1.0, 1).unwrap_err().code(), "extract_busy");
        drop(held);
        assert_ne!(extract(&root, &v, 0.0, 1.0, 1).unwrap_err().code(), "extract_busy");
    }

    #[test]
    fn agent_capture_needs_the_setting() {
        let d = tempfile::tempdir().unwrap();
        let s = crate::runtime::AppState::new(d.path().into(), "C:/x/regente.exe".into(), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).unwrap();
        assert_eq!(agent_allowed(&s).unwrap_err().code(), "agent_capture_off");
        // a pane rewriting shot.json on disk does not flip the in-memory value
        save(d.path(), &ShotConfig { agent_capture: true, ..ShotConfig::default() }).unwrap();
        assert_eq!(agent_allowed(&s).unwrap_err().code(), "agent_capture_off", "the file on disk is not re-read");
        lk(&s.guarded).agent_capture = true;
        assert!(agent_allowed(&s).is_ok());
    }


    #[test]
    fn edit_saves_the_png_and_the_layers_beside_it_and_refuses_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("shots");
        std::fs::create_dir_all(&root).unwrap();
        let shot = save_png(&img(2, 2), &root).unwrap();
        let png = png_bytes(&img(3, 3)).unwrap();
        save_edit(&root, &shot, &png, r#"[{"type":"rect"}]"#).unwrap();
        assert_eq!(image::open(&shot).unwrap().width(), 3);
        let st = edit_get(&root, &shot).unwrap();
        assert_eq!(st.doc.as_deref(), Some(r#"[{"type":"rect"}]"#));
        assert_eq!(st.base.file_name(), orig_of(&shot).file_name(), "the original stays the base after the first save");
        assert!(list(&root).iter().all(|i| !i.name.contains("orig")));
        assert_eq!(save_edit(&root, &shot, b"nope", "[]").unwrap_err().code(), "not_png");
        assert_eq!(save_edit(&root, &shot, &png, "{").unwrap_err().code(), "bad_layers");
        let txt = root.join("a.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert_eq!(save_edit(&root, &txt, &png, "[]").unwrap_err().code(), "bad_name");
        assert_eq!(save_edit(&root, &d.path().join("fora.png"), &png, "[]").unwrap_err().code(), "not_found");
        let outside = d.path().join("fora.png");
        std::fs::write(&outside, &png).unwrap();
        assert_eq!(save_edit(&root, &outside, &png, "[]").unwrap_err().code(), "outside_shots");
        assert_eq!(edit_get(&root, &outside).unwrap_err().code(), "outside_shots");
        assert_eq!(delete(&root, &outside).unwrap_err().code(), "outside_shots");
        assert_eq!(delete(&root, &root).unwrap_err().code(), "bad_name");
    }

    #[test]
    fn record_args_use_even_sizes_and_the_desktop_grab() {
        let a = record_args(10, 20, 1921, 1081, 30, Path::new("C:/o/shot-1.mp4"));
        let joined = a.join(" ");
        assert!(joined.contains("-video_size 1920x1080"), "{joined}");
        assert!(joined.contains("-f gdigrab -framerate 30 -offset_x 10 -offset_y 20"), "{joined}");
        assert!(joined.contains("-i desktop -c:v libx264 -preset veryfast -pix_fmt yuv420p"), "{joined}");
        assert!(joined.ends_with("-movflags +frag_keyframe+empty_moov C:/o/shot-1.mp4"), "{joined}");
        assert_eq!(a.last().unwrap(), "C:/o/shot-1.mp4");
    }

    #[test]
    fn frames_args_trim_then_dedupe_into_numbered_pngs() {
        let a = frames_args(Path::new("in.mp4"), 0.5, 2.5, 2.0, Path::new("out"));
        let joined = a.join(" ");
        assert!(joined.contains("-ss 0.500 -to 2.500 -i in.mp4"), "{joined}");
        assert!(joined.contains("-vf fps=2,mpdecimate,setpts=N/FRAME_RATE/TB -fps_mode vfr"), "{joined}");
        assert!(a.last().unwrap().ends_with("%04d.png"));
    }

    #[test]
    fn contact_sheet_has_a_grid_of_thumbnails_with_bands() {
        let d = tempfile::tempdir().unwrap();
        let frames: Vec<PathBuf> = (0..5)
            .map(|i| {
                let p = d.path().join(format!("{:04}.png", i + 1));
                RgbaImage::from_pixel(10, 10, Rgba([i as u8 * 40, 0, 0, 255])).save(&p).unwrap();
                p
            })
            .collect();
        let out = d.path().join("contact.png");
        contact_sheet(&frames, 2, 10, &out).unwrap();
        // 2 columns of 10 px; cells of 14 px band + 10 px frame; 5 frames make 3 rows
        assert_eq!(image::image_dimensions(&out).unwrap(), (20, 72));
        assert_eq!(contact_sheet(&[], 2, 10, &out).unwrap_err().code(), "bad_sheet");
    }

    #[test]
    fn extract_refuses_bad_settings_and_foreign_files_before_running_ffmpeg() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("shots");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("shot-1.mp4"), b"x").unwrap();
        std::fs::write(root.join("notas.mp4"), b"x").unwrap();
        let v = root.join("shot-1.mp4");
        assert_eq!(extract(&root, &v, 0.0, 1.0, 3).unwrap_err().code(), "bad_fps");
        assert_eq!(extract(&root, &v, 2.0, 1.0, 1).unwrap_err().code(), "bad_range");
        assert_eq!(extract(&root, &v, 0.0, 400.0, 1).unwrap_err().code(), "bad_range");
        assert_eq!(extract(&root, &root.join("notas.mp4"), 0.0, 1.0, 1).unwrap_err().code(), "bad_name");
        std::fs::write(d.path().join("fora.mp4"), b"x").unwrap();
        assert_eq!(extract(&root, &d.path().join("fora.mp4"), 0.0, 1.0, 1).unwrap_err().code(), "outside_shots");
    }

    /// The stop protocol against a real ffmpeg, with a generated test pattern (no screen is recorded).
    /// Run with `cargo test finish_ends_a_real_ffmpeg -- --ignored` (ffmpeg on PATH).
    #[test]
    #[ignore]
    fn finish_ends_a_real_ffmpeg() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("shot-test.mp4");
        let ffmpeg = ffmpeg_path().expect("ffmpeg no PATH");
        let child = util::cmd(&ffmpeg)
            .args(["-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&out)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert_eq!(finish(Recording { child, out: out.clone() }).unwrap(), out);
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
    }

    /// Frames and contact sheet from a generated video, with the real ffmpeg. Run with `-- --ignored`.
    #[test]
    #[ignore]
    fn extract_real_frames() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("shots");
        std::fs::create_dir_all(&root).unwrap();
        let video = root.join("shot-20260101-000000-000.mp4");
        let ffmpeg = ffmpeg_path().expect("ffmpeg no PATH");
        let st = util::cmd(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=3", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&video)
            .status()
            .unwrap();
        assert!(st.success());
        let got = extract(&root, &video, 0.0, 2.0, 2).unwrap();
        assert!(got.frames >= 2, "{got:?}");
        assert!(got.contact.exists());
        assert!(got.dir.ends_with("shot-20260101-000000-000-frames"));
    }

    /// Needs a real display: run with `cargo test capture_real -- --ignored`.
    #[test]
    #[ignore]
    fn capture_real() {
        let (img, _) = capture_screen_under_cursor().unwrap();
        assert!(img.width() > 0 && img.height() > 0);
    }
}
