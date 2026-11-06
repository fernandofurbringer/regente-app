//! Ditado global: um atalho global (segurado ou alternado) grava no Regente, o áudio vai ao motor de voz e o
//! texto é colado no app focado (clipboard + Ctrl+V). Desligado por padrão (`voice.globalEnabled`).
//! Só o Regente toca o teclado e o clipboard; a lógica (rotas, máquina do atalho, sequência de colagem) é pura e
//! testada com um `Os` falso. Nenhum teste registra atalho nem cola nada.

use crate::error::{fail, AppError, AppResult};
use crate::jarvis;
use crate::runtime::AppState;
use crate::stt::{self, VoiceConfig};
use crate::util::{lk, now_ms};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;

/// Instruction of the code mode rewrite: Portuguese dictation in, English out, literals kept as spoken.
pub const CODE_REWRITE_PROMPT: &str = "Você reescreve um ditado em português como texto em inglês para um agente de programação. \
Traduza para inglês e, ao traduzir, manter literais exatamente como foram ditados: nomes de arquivos, caminhos, comandos, flags, \
nomes de bibliotecas, identificadores e trechos de código. Responda só com o texto em inglês, sem aspas e sem explicações.";

/// How long the text stays in the clipboard before the saved one comes back (the target app reads it on Ctrl+V).
pub const RESTORE_AFTER: Duration = Duration::from_millis(500);
/// How long a paste waits for Ctrl/Shift/Alt/Win to be released. A held Alt turns Ctrl+V into menu keys in some apps.
pub const MODIFIER_WAIT: Duration = Duration::from_secs(1);
const TRAY_ID: &str = "regente-tray";

/// Global state: whether a dictation is recording. The frontend owns the microphone; this flag owns the hotkey.
#[derive(Default)]
pub struct Dictation {
    recording: AtomicBool,
    /// The registered dictation hotkey (sync drops only this one: Shot's hotkeys share the plugin).
    hotkey: Mutex<Option<Shortcut>>,
}

/// What a global dictation does with the transcription.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Route {
    /// Faithful Portuguese transcription (chat mode).
    Chat,
    /// Whisper's own English translation (code mode without an IA auxiliar).
    WhisperEnglish,
    /// Portuguese transcription, then the IA auxiliar rewrites it in English keeping the literals (code mode with the IA auxiliar).
    RewriteEnglish,
}

pub fn route(global_mode: &str, jarvis_configured: bool) -> Route {
    match (global_mode, jarvis_configured) {
        ("code", true) => Route::RewriteEnglish,
        ("code", false) => Route::WhisperEnglish,
        _ => Route::Chat,
    }
}

/// The text pasted into the app: one line without blank edges. A newline would press Enter in a terminal.
pub fn clean(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Cmd {
    Start,
    Stop,
    Ignore,
}

/// What a hotkey event does. Hold: press starts, release stops (repeats are ignored). Toggle: each press flips.
pub fn on_hotkey(recording: bool, hold: bool, pressed: bool) -> Cmd {
    match (hold, pressed, recording) {
        (true, true, false) | (false, true, false) => Cmd::Start,
        (true, false, true) | (false, true, true) => Cmd::Stop,
        _ => Cmd::Ignore,
    }
}

/// The keyboard and clipboard calls of a paste. The real one is `SystemOs`; tests use a fake.
pub trait Os {
    /// The clipboard's text, None when it holds no text.
    fn clipboard(&mut self) -> AppResult<Option<String>>;
    /// Sets the clipboard text. The dictated text is kept out of the clipboard history where the OS allows it.
    fn set_clipboard(&mut self, text: &str) -> AppResult<()>;
    /// True when a Regente window has the focus right now (the keystroke would land in our own UI).
    fn regente_focused(&self) -> bool {
        false
    }
    /// True when no Ctrl, Shift, Alt or Win key is held right now.
    fn modifiers_up(&self) -> bool {
        true
    }
    /// Ctrl+V into the focused window.
    fn paste_keys(&mut self) -> AppResult<()>;
}

/// Puts `text` into the focused app: saves the clipboard, sets the text, sends Ctrl+V and, after `restore_after`,
/// puts the saved text back. The focus is checked again right before the keystroke: if a Regente window has it, the
/// text stays in the clipboard and `regente_focused` comes back. A failed keystroke also leaves the text there (no
/// restore). Either way the previous clipboard content is gone.
pub fn paste_text(os: &mut dyn Os, text: &str, restore_after: Duration) -> AppResult<()> {
    let saved = os.clipboard()?;
    os.set_clipboard(text)?;
    if os.regente_focused() {
        return fail("regente_focused", "o Regente estava em foco");
    }
    // the hotkey's own modifiers may still be down (the user lets go a moment after the release): wait, then paste anyway
    let until = Instant::now() + MODIFIER_WAIT;
    while !os.modifiers_up() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    os.paste_keys()?;
    std::thread::sleep(restore_after);
    // a clipboard that held no text (an image, say) comes back empty: it cannot be saved as text
    os.set_clipboard(&saved.unwrap_or_default())
}

/// The real clipboard and keyboard. Without an app handle (the manual test) the focus is not checked.
pub struct SystemOs {
    app: Option<AppHandle>,
}

impl SystemOs {
    pub fn new(app: &AppHandle) -> Self {
        Self { app: Some(app.clone()) }
    }

    #[cfg(test)]
    pub fn detached() -> Self {
        Self { app: None }
    }
}

fn clip_err(e: arboard::Error) -> AppError {
    AppError::new("clipboard_failed", format!("área de transferência indisponível: {e}"))
}

impl Os for SystemOs {
    fn clipboard(&mut self) -> AppResult<Option<String>> {
        let mut cb = arboard::Clipboard::new().map_err(clip_err)?;
        match cb.get_text() {
            Ok(t) => Ok(Some(t)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(clip_err(e)),
        }
    }

    fn set_clipboard(&mut self, text: &str) -> AppResult<()> {
        let mut cb = arboard::Clipboard::new().map_err(clip_err)?;
        #[cfg(windows)]
        {
            use arboard::SetExtWindows;
            // ExcludeClipboardContentFromMonitorProcessing: no Win+V history, no cloud clipboard
            return cb.set().exclude_from_monitoring().text(text).map_err(clip_err);
        }
        #[cfg(not(windows))]
        cb.set_text(text).map_err(clip_err)
    }

    fn regente_focused(&self) -> bool {
        self.app.as_ref().is_some_and(regente_focused)
    }

    #[cfg(windows)]
    fn modifiers_up(&self) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
        // the high bit is set while the key is down
        [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN].iter().all(|&k| (unsafe { GetAsyncKeyState(k as i32) } as u16) & 0x8000 == 0)
    }

    fn paste_keys(&mut self) -> AppResult<()> {
        use enigo::{Direction, Enigo, Key, Keyboard, Settings};
        let input = |e: enigo::InputError| AppError::new("paste_failed", format!("não foi possível colar no app focado: {e}"));
        let mut en = Enigo::new(&Settings::default()).map_err(|e| AppError::new("paste_failed", format!("teclado indisponível: {e}")))?;
        en.key(Key::Control, Direction::Press).map_err(input)?;
        let v = en.key(Key::V, Direction::Click);
        // Ctrl comes up even when the V press failed, so the user's keyboard is never left held down
        let release = en.key(Key::Control, Direction::Release);
        v.and(release).map_err(input)
    }
}

/// The hotkey string as a shortcut. Invalid text is a clear error (Settings shows it).
pub fn parse_hotkey(text: &str) -> AppResult<Shortcut> {
    text.parse::<Shortcut>().map_err(|_| AppError::new("bad_hotkey", format!("atalho inválido: {text}")))
}

/// Registers the voice hotkey when global dictation is on, and drops the previous registrations.
/// Called on save and at startup. Fails when another app already owns the combination.
pub fn sync(app: &AppHandle, cfg: &VoiceConfig) -> AppResult<()> {
    let gs = app.global_shortcut();
    if let Some(d) = app.try_state::<Dictation>() {
        if let Some(old) = lk(&d.hotkey).take() {
            let _ = gs.unregister(old);
        }
    }
    let _ = gs.unregister(Shortcut::new(None, Code::Escape));
    if !cfg.global_enabled {
        // turned off while a dictation was recording: that recording is cancelled
        if app.try_state::<Dictation>().is_some_and(|d| d.recording.load(Ordering::SeqCst)) {
            set_recording(app, false);
            if let Some(s) = app.try_state::<Arc<AppState>>() {
                s.emit("global-voice", json!({"state": "cancel"}));
            }
        }
        return Ok(());
    }
    let shortcut = parse_hotkey(&cfg.global_hotkey)?;
    gs.register(shortcut).map_err(|e| AppError::new("hotkey_busy", format!("o atalho {} não está disponível (em uso por outro app?): {e}", cfg.global_hotkey)))?;
    if let Some(d) = app.try_state::<Dictation>() {
        *lk(&d.hotkey) = Some(shortcut);
    }
    // the sync dropped Esc: a dictation still recording keeps its cancel key
    if app.try_state::<Dictation>().is_some_and(|d| d.recording.load(Ordering::SeqCst)) {
        let _ = gs.register(Shortcut::new(None, Code::Escape));
    }
    Ok(())
}

/// The plugin with its handler: presses and releases of the voice hotkey, and Esc while recording.
pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new().with_handler(|app, shortcut, ev| on_shortcut(app, shortcut, ev.state == ShortcutState::Pressed)).build()
}

fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, pressed: bool) {
    if crate::shot::on_shortcut(app, shortcut, pressed) {
        return;
    }
    let Some(s) = app.try_state::<Arc<AppState>>() else { return };
    let recording = app.state::<Dictation>().recording.load(Ordering::SeqCst);
    if shortcut.matches(Modifiers::empty(), Code::Escape) {
        if pressed && recording {
            set_recording(app, false);
            s.emit("global-voice", json!({"state": "cancel"}));
        }
        return;
    }
    let cfg = stt::load(&s.data);
    if !cfg.global_enabled || parse_hotkey(&cfg.global_hotkey).ok().as_ref() != Some(shortcut) {
        return;
    }
    match on_hotkey(recording, cfg.global_hold, pressed) {
        Cmd::Start => {
            set_recording(app, true);
            s.emit("global-voice", json!({"state": "start"}));
        }
        Cmd::Stop => {
            set_recording(app, false);
            s.emit("global-voice", json!({"state": "stop"}));
        }
        Cmd::Ignore => {}
    }
}

/// Sets the recording flag, the tray (icon and tooltip) and Esc as a global cancel while recording.
fn set_recording(app: &AppHandle, on: bool) {
    app.state::<Dictation>().recording.store(on, Ordering::SeqCst);
    let gs = app.global_shortcut();
    if on {
        let _ = gs.register(Shortcut::new(None, Code::Escape));
    } else {
        let _ = gs.unregister(Shortcut::new(None, Code::Escape));
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(if on { "Regente · gravando ditado" } else { "Regente" }));
        let icon = if on { Image::from_bytes(include_bytes!("../icons/tray-recording.png")).ok() } else { app.default_window_icon().cloned() };
        let _ = tray.set_icon(icon);
    }
}

/// Frontend could not start the microphone: back to idle.
pub fn cancel(app: &AppHandle) {
    set_recording(app, false);
}

/// Transcribes a global dictation and pastes it into the focused app. The frontend records the audio (even with
/// the Regente window in the background) and sends it here when the hotkey is released.
pub async fn finish(app: &AppHandle, s: &Arc<AppState>, audio: Vec<u8>, mime: &str) -> AppResult<String> {
    set_recording(app, false);
    let cfg = stt::load(&s.data);
    let (to_english, rewrite) = match route(&cfg.global_mode, jarvis::configured(s)) {
        Route::Chat => (false, false),
        Route::WhisperEnglish => (true, false),
        Route::RewriteEnglish => (false, true),
    };
    let mut text = clean(&stt::transcribe_as(s, audio, mime, "global", Some(to_english)).await?);
    if text.is_empty() {
        return fail("nothing_recognized", "nada foi reconhecido");
    }
    if rewrite {
        text = clean(&jarvis::rewrite_english(s, &text).await?);
    }
    s.db.add_voice_history(now_ms(), &cfg.global_mode, &text)?;
    // the focus is checked inside the paste, right before the keystroke: never type into the Regente window
    let t = text.clone();
    let os_app = app.clone();
    let pasted = tokio::task::spawn_blocking(move || paste_text(&mut SystemOs::new(&os_app), &t, RESTORE_AFTER)).await.map_err(|e| AppError::new("internal", e.to_string()))?;
    if let Err(e) = pasted {
        tracing::warn!("ditado global: colagem não feita ({}); texto na área de transferência", e.code());
        let why = if e.code() == "regente_focused" { "O Regente estava em foco." } else { "Não consegui colar no app focado." };
        notify(app, &format!("{why} O texto ditado está na área de transferência e substituiu o conteúdo anterior."));
    }
    Ok(text)
}

/// Global dictation off (or the app quitting): drops the registrations and, when a dictation was recording, cancels it.
pub fn shutdown(app: &AppHandle) {
    if app.try_state::<Dictation>().is_some_and(|d| d.recording.load(Ordering::SeqCst)) {
        set_recording(app, false);
        if let Some(s) = app.try_state::<Arc<AppState>>() {
            s.emit("global-voice", json!({"state": "cancel"}));
        }
    }
    let _ = app.global_shortcut().unregister_all();
}

/// Some Regente window has the focus (also used by the Windows notices in `notify.rs`).
pub fn regente_focused(app: &AppHandle) -> bool {
    app.webview_windows().values().any(|w| w.is_focused().unwrap_or(false))
}

fn notify(app: &AppHandle, body: &str) {
    let _ = app.notification().builder().title("Regente").body(body).show();
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// The tray menu: one disabled line per Redline window, then "Abrir limites de uso", "Abrir Regente" and "Sair".
fn tray_menu(app: &AppHandle, limits: &crate::redline::Payload) -> tauri::Result<Menu<tauri::Wry>> {
    let now = now_ms();
    let mut owned: Vec<MenuItem<tauri::Wry>> = Vec::new();
    if limits.windows.is_empty() {
        owned.push(MenuItem::with_id(app, "limit-none", "Limites: sem leitura", false, None::<&str>)?);
    }
    for (i, w) in limits.windows.iter().enumerate() {
        owned.push(MenuItem::with_id(app, format!("limit-{i}"), crate::redline::tray_line(w, now), false, None::<&str>)?);
    }
    let sep = PredefinedMenuItem::separator(app)?;
    let redline = MenuItem::with_id(app, "redline", "Abrir limites de uso", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Abrir Regente", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let mut refs: Vec<&dyn IsMenuItem<tauri::Wry>> = owned.iter().map(|m| m as &dyn IsMenuItem<tauri::Wry>).collect();
    refs.push(&sep);
    refs.push(&redline);
    refs.push(&open);
    refs.push(&quit);
    Menu::with_items(app, &refs)
}

/// Rebuilds the tray menu with the latest limits (every `limits` event). The icon is left alone (it may be recording).
pub fn set_limits(app: &AppHandle, limits: &crate::redline::Payload) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    match tray_menu(app, limits) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => tracing::warn!("bandeja: menu de limites não montado ({e})"),
    }
}

/// Shows and focuses the Redline window when it exists; false when it still has to be created.
fn focus_redline(app: &AppHandle) -> bool {
    let Some(w) = app.get_webview_window("redline") else { return false };
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
    true
}

/// Opens the Redline window, or brings it to the front when it is already open. Creating a window must not run on
/// the main thread (the tray's handler runs there): call this from a worker.
pub fn open_redline(app: &AppHandle) {
    if focus_redline(app) {
        return;
    }
    let built = tauri::WebviewWindowBuilder::new(app, "redline", tauri::WebviewUrl::App("index.html#/redline".into()))
        .title("Regente Limites de uso")
        .inner_size(980.0, 720.0)
        .min_inner_size(640.0, 480.0)
        .build();
    if let Err(e) = built {
        tracing::warn!("janela de limites não abriu: {e}");
    }
}

/// The tray: the icon, the limits, "Abrir limites de uso", "Abrir Regente" and "Sair". It pulses to the recording icon while dictating.
pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = tray_menu(app, &crate::redline::Payload::default())?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID).tooltip("Regente").menu(&menu).on_menu_event(|app, ev| match ev.id().as_ref() {
        "open" => show_main(app),
        "redline" => {
            // show here; a window still to be created is built on a worker, not on the menu's main thread
            if !focus_redline(app) {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { open_redline(&app) });
            }
        }
        "quit" => app.exit(0),
        _ => {}
    });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeOs {
        clip: Option<String>,
        calls: Vec<String>,
        fail_paste: bool,
        fail_read: bool,
        focused: bool,
        /// polls that still find a modifier held
        held_polls: Cell<u32>,
    }

    impl Os for FakeOs {
        fn regente_focused(&self) -> bool {
            self.focused
        }

        fn modifiers_up(&self) -> bool {
            let left = self.held_polls.get();
            self.held_polls.set(left.saturating_sub(1));
            left == 0
        }

        fn clipboard(&mut self) -> AppResult<Option<String>> {
            self.calls.push("get".into());
            if self.fail_read {
                return fail("clipboard_failed", "travada");
            }
            Ok(self.clip.clone())
        }
        fn set_clipboard(&mut self, text: &str) -> AppResult<()> {
            self.calls.push(format!("set:{text}"));
            self.clip = Some(text.to_string());
            Ok(())
        }
        fn paste_keys(&mut self) -> AppResult<()> {
            self.calls.push("ctrl+v".into());
            if self.fail_paste {
                return fail("paste_failed", "sem teclado");
            }
            Ok(())
        }
    }

    #[test]
    fn code_rewrite_prompt_keeps_literals() {
        assert!(CODE_REWRITE_PROMPT.contains("manter literais"));
        assert!(CODE_REWRITE_PROMPT.contains("inglês"));
    }

    #[test]
    fn route_picks_the_rewrite_only_in_code_mode_with_a_jarvis() {
        assert_eq!(route("chat", true), Route::Chat);
        assert_eq!(route("chat", false), Route::Chat);
        assert_eq!(route("code", true), Route::RewriteEnglish);
        assert_eq!(route("code", false), Route::WhisperEnglish);
    }

    #[test]
    fn hold_starts_on_press_and_stops_on_release() {
        assert_eq!(on_hotkey(false, true, true), Cmd::Start);
        assert_eq!(on_hotkey(true, true, false), Cmd::Stop);
        // a key repeat while held does not restart; a stray release while idle does nothing
        assert_eq!(on_hotkey(true, true, true), Cmd::Ignore);
        assert_eq!(on_hotkey(false, true, false), Cmd::Ignore);
    }

    #[test]
    fn toggle_flips_on_each_press_and_ignores_releases() {
        assert_eq!(on_hotkey(false, false, true), Cmd::Start);
        assert_eq!(on_hotkey(true, false, true), Cmd::Stop);
        assert_eq!(on_hotkey(true, false, false), Cmd::Ignore);
        assert_eq!(on_hotkey(false, false, false), Cmd::Ignore);
    }

    #[test]
    fn clean_joins_lines_and_drops_blank_edges() {
        assert_eq!(clean("  olá\n  mundo  \n"), "olá mundo");
        assert_eq!(clean("\n\n  \n"), "");
        assert_eq!(clean("cargo test --lib"), "cargo test --lib");
    }

    #[test]
    fn paste_saves_sets_pastes_and_restores_in_order() {
        let mut os = FakeOs { clip: Some("antigo".into()), ..Default::default() };
        paste_text(&mut os, "novo", Duration::ZERO).unwrap();
        assert_eq!(os.calls, ["get", "set:novo", "ctrl+v", "set:antigo"]);
        assert_eq!(os.clip.as_deref(), Some("antigo"));
    }

    #[test]
    fn failed_paste_keeps_the_text_in_the_clipboard() {
        let mut os = FakeOs { clip: Some("antigo".into()), fail_paste: true, ..Default::default() };
        assert_eq!(paste_text(&mut os, "novo", Duration::ZERO).unwrap_err().code(), "paste_failed");
        assert_eq!(os.calls, ["get", "set:novo", "ctrl+v"]);
        assert_eq!(os.clip.as_deref(), Some("novo"));
    }

    #[test]
    fn focus_on_regente_at_the_last_moment_never_pastes() {
        let mut os = FakeOs { clip: Some("antigo".into()), focused: true, ..Default::default() };
        assert_eq!(paste_text(&mut os, "novo", Duration::ZERO).unwrap_err().code(), "regente_focused");
        // the text waits in the clipboard, no keystroke, no restore
        assert_eq!(os.calls, ["get", "set:novo"]);
        assert_eq!(os.clip.as_deref(), Some("novo"));
    }

    #[test]
    fn paste_waits_for_the_modifiers_to_be_released() {
        let mut os = FakeOs { held_polls: Cell::new(3), ..Default::default() };
        paste_text(&mut os, "novo", Duration::ZERO).unwrap();
        assert_eq!(os.held_polls.get(), 0);
        assert_eq!(os.calls, ["get", "set:novo", "ctrl+v", "set:"]);
    }

    #[test]
    fn unreadable_clipboard_stops_before_touching_it() {
        let mut os = FakeOs { fail_read: true, ..Default::default() };
        assert!(paste_text(&mut os, "novo", Duration::ZERO).is_err());
        assert_eq!(os.calls, ["get"]);
    }

    #[test]
    fn a_clipboard_without_text_is_cleared_after_the_paste() {
        let mut os = FakeOs::default();
        paste_text(&mut os, "novo", Duration::ZERO).unwrap();
        assert_eq!(os.calls.last().map(String::as_str), Some("set:"));
    }

    #[test]
    fn hotkeys_parse_without_registering_anything() {
        assert!(parse_hotkey("Ctrl+Shift+Space").is_ok());
        assert!(parse_hotkey("Ctrl+Shift+3").is_ok());
        assert_eq!(parse_hotkey("não é tecla").unwrap_err().code(), "bad_hotkey");
    }

    /// Types into whatever window is focused: run by hand only, never in CI.
    #[test]
    #[ignore = "digita no app focado: rodar manualmente com o Bloco de Notas aberto"]
    fn paste_into_the_focused_app() {
        paste_text(&mut SystemOs::detached(), "teste do ditado global", RESTORE_AFTER).unwrap();
    }
}
