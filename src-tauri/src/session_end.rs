//! Fim de sessão do Windows (logoff, reinício, desligamento).
//!
//! O tao não repassa `WM_ENDSESSION` como `ExitRequested`: ao receber a mensagem com `wParam = TRUE`, ele chama
//! `process::exit(0)` direto, sem rodar `graceful_shutdown`. Por isso a janela principal ganha um wndproc que
//! encerra o app antes de repassar a mensagem ao wndproc original do tao.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;

/// Quem tem a ordem de desligar: `Some` depois de `install`.
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
/// wndproc original (tao) da janela principal, guardado como `isize` para caber em `SetWindowLongPtrW`.
static PREV_PROC: AtomicIsize = AtomicIsize::new(0);
/// `WM_ENDSESSION` pode chegar mais de uma vez; o desligamento roda uma única vez.
static DONE: AtomicBool = AtomicBool::new(false);

/// Decide se uma mensagem de fim de sessão deve disparar o desligamento limpo (pura, testável).
pub fn should_shutdown(msg: u32, wparam: usize, already_done: bool) -> bool {
    msg == windows_sys::Win32::UI::WindowsAndMessaging::WM_ENDSESSION && wparam != 0 && !already_done
}

/// Instala o hook na janela principal. Chamar uma vez, na thread principal, no `setup`.
#[cfg(windows)]
pub fn install(app: &tauri::AppHandle, hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_WNDPROC};
    let _ = APP.set(app.clone());
    let prev = unsafe { SetWindowLongPtrW(hwnd as _, GWLP_WNDPROC, session_proc as *const () as usize as isize) };
    PREV_PROC.store(prev, Ordering::SeqCst);
}

#[cfg(windows)]
unsafe extern "system" fn session_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallWindowProcW, WNDPROC};
    // swap só para a mensagem de desligamento real: a consulta (WM_QUERYENDSESSION) não consome a ordem
    if should_shutdown(msg, wparam, false) && !DONE.swap(true, Ordering::SeqCst) {
        if let Some(app) = APP.get() {
            let t0 = std::time::Instant::now();
            // um pânico no desligamento não pode atravessar o wndproc (FFI): vira log e a mensagem segue
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::graceful_shutdown(app))).is_err();
            tracing::info!("desligamento de sessão do Windows: {} ms (pânico: {panicked})", t0.elapsed().as_millis());
        }
    }
    // SAFETY: o valor veio de SetWindowLongPtrW na instalação, é o wndproc original do tao
    let prev: WNDPROC = unsafe { std::mem::transmute(PREV_PROC.load(Ordering::SeqCst)) };
    unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::should_shutdown;
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENDSESSION, WM_QUERYENDSESSION};

    #[test]
    fn only_real_session_end_triggers_shutdown_once() {
        // desligamento de verdade: wParam TRUE
        assert!(should_shutdown(WM_ENDSESSION, 1, false));
        // o usuário cancelou o desligamento: wParam FALSE, nada a fazer
        assert!(!should_shutdown(WM_ENDSESSION, 0, false));
        // já desligou: não repete
        assert!(!should_shutdown(WM_ENDSESSION, 1, true));
        // a consulta (WM_QUERYENDSESSION) não encerra nada
        assert!(!should_shutdown(WM_QUERYENDSESSION, 1, false));
    }
}
