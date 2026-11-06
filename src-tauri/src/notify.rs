//! Avisos do Windows: agente terminou, perguntou ou abriu um gate. Só quando o Regente não está com a tela do painel
//! em foco, e no máximo 1 aviso por painel a cada 30 s. As regras de decisão são puras (testadas aqui); o envio
//! fica em `AppState::notify_pane` e em `lib.rs` (evento `os-notify`).

use crate::runtime::AppState;
use crate::util::lk;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

/// Trabalho mínimo antes de um "terminou" valer aviso: resposta curta não incomoda.
pub const MIN_WORK: Duration = Duration::from_secs(20);
/// Intervalo mínimo entre avisos do mesmo painel.
pub const RATE: Duration = Duration::from_secs(30);
/// Painéis guardados para o toast do próximo foco.
const PENDING_MAX: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Finished,
    Asks,
    Gate,
}

/// Terminou depois de trabalhar pelo menos `MIN_WORK`.
pub fn finished_worth_notice(worked: Duration) -> bool {
    worked >= MIN_WORK
}

/// Nunca avisou, ou o último aviso do painel é de `RATE` atrás ou mais.
pub fn rate_allows(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= RATE)
}

/// O usuário está vendo este painel: a janela tem o foco e a tela mostra o painel.
pub fn pane_visible(window_focused: bool, in_view: bool) -> bool {
    window_focused && in_view
}

impl AppState {
    /// Aviso do Windows de um painel, se a preferência estiver ligada, o painel não estiver visível e ele não
    /// tiver avisado nos últimos 30 s. O envio é o evento `os-notify` (ver `show_os_notice` em `lib.rs`).
    pub fn notify_pane(&self, pane: &str, kind: Kind, title: &str, body: &str) {
        let on = {
            let s = lk(&self.settings);
            match kind {
                Kind::Finished => s.notify.finished,
                Kind::Asks => s.notify.asks,
                Kind::Gate => s.notify.gates,
            }
        };
        if !on {
            return;
        }
        let focused = self.window_focused.load(Ordering::Relaxed);
        let now = Instant::now();
        {
            let mut rt = self.rt();
            if pane_visible(focused, rt.visible_panes.contains(pane)) || !rate_allows(rt.last_notified.get(pane).copied(), now) {
                return;
            }
            rt.last_notified.insert(pane.into(), now);
            // o toast "ir para o painel" só serve quando o aviso chegou com a janela fora de foco
            if !focused {
                rt.pending_notices.retain(|p| p != pane);
                rt.pending_notices.push(pane.into());
                if rt.pending_notices.len() > PENDING_MAX {
                    rt.pending_notices.remove(0);
                }
            }
        }
        self.emit("os-notify", json!({"title": title, "body": body, "paneId": pane}));
        // o celular aberto recebe só o tipo e o painel: o texto (que pode vir do agente) fica no computador
        let name = match kind {
            Kind::Finished => "finished",
            Kind::Asks => "asks",
            Kind::Gate => "gate",
        };
        let _ = self.notices.send(json!({"kind": name, "paneId": pane}).to_string());
    }

    /// Aviso de um agente que terminou ou perguntou (painéis de IA apenas).
    pub fn notify_status(&self, pane: &str, kind: Kind) {
        let Ok(row) = self.db.get_pane(pane) else { return };
        if row.kind != "agent" {
            return;
        }
        let (title, body) = match kind {
            Kind::Finished => ("Agente terminou".to_string(), format!("{} voltou a ficar ocioso.", row.name)),
            // o texto da pergunta fica de fora: aparece na tela de bloqueio do Windows
            Kind::Asks => ("Agente perguntou".to_string(), format!("{} está esperando sua resposta", row.name)),
            Kind::Gate => return,
        };
        self.notify_pane(pane, kind, &title, &body);
    }

    /// Painéis que avisaram e ainda não foram vistos, do mais antigo ao mais novo: o foco na janela os leva ao toast.
    pub fn take_pending_notices(&self) -> Vec<String> {
        std::mem::take(&mut self.rt().pending_notices)
    }

    /// O usuário clicou no aviso deste painel: ele já vai para a tela, o toast do foco não precisa repetir.
    pub fn notice_opened(&self, pane: &str) {
        self.rt().pending_notices.retain(|p| p != pane);
    }
}

/// Painel de um clique no aviso: só o clique no próprio aviso (`tap`), não em botões.
pub fn tapped_pane(action: &str, extra: Option<&std::collections::HashMap<String, Value>>) -> Option<String> {
    (action == "tap").then_some(())?;
    extra?.get("paneId")?.as_str().map(str::to_string)
}

/// Clique no aviso do Windows (enquanto ele está na tela; da Central de Ações o Windows não avisa o app): traz a
/// janela principal e leva ao painel (`notify-open`, ver `appEvents.ts`).
pub fn on_notice_click(app: &AppHandle) {
    let h = app.clone();
    let _ = app.notification().on_action(move |a| {
        let Some(pane) = tapped_pane(a.action_id(), a.notification().map(|n| n.extra())) else { return };
        if let Some(s) = h.try_state::<std::sync::Arc<AppState>>() {
            s.notice_opened(&pane);
        }
        if let Some(w) = h.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
        let _ = h.emit_to("main", "notify-open", json!({"paneId": pane}));
    });
}

/// Mostra o aviso do Windows (chamado pelo emissor de eventos em `lib.rs`).
pub fn show_os_notice(app: &AppHandle, payload: &Value) {
    let title = payload["title"].as_str().unwrap_or("Regente");
    let body = payload["body"].as_str().unwrap_or_default();
    let mut b = app.notification().builder().title(title).body(body);
    // o clique no aviso (`on_notice_click`) volta com o painel
    if let Some(p) = payload["paneId"].as_str() {
        b = b.extra("paneId", p);
    }
    if let Err(e) = b.show() {
        tracing::warn!("aviso do Windows: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clique_no_aviso_volta_com_o_painel() {
        let extra: std::collections::HashMap<String, Value> = [("paneId".to_string(), json!("p1"))].into();
        assert_eq!(tapped_pane("tap", Some(&extra)).as_deref(), Some("p1"));
        assert_eq!(tapped_pane("outro", Some(&extra)), None, "botão do aviso não é o clique");
        assert_eq!(tapped_pane("tap", None), None);
        assert_eq!(tapped_pane("tap", Some(&Default::default())), None);
    }

    #[test]
    fn avisos_pendentes_guardam_todos_os_paineis_sem_repetir() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), std::sync::Arc::new(|_: &str, _: Value| {})).unwrap();
        lk(&s.settings).notify.finished = true;
        s.window_focused.store(false, Ordering::Relaxed);
        for p in ["a", "b", "a"] {
            s.rt().last_notified.clear();
            s.notify_pane(p, Kind::Finished, "t", "c");
        }
        s.notice_opened("b");
        assert_eq!(s.take_pending_notices(), vec!["a".to_string()]);
        assert!(s.take_pending_notices().is_empty());
    }

    #[test]
    fn phone_notice_carries_kind_and_pane_only() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), std::sync::Arc::new(|_: &str, _: Value| {})).unwrap();
        let mut rx = s.notices.subscribe();
        lk(&s.settings).notify.asks = true;
        // a janela sem foco: o aviso passa; o texto da pergunta não sai no canal do celular
        s.notify_pane("p1", Kind::Asks, "Agente perguntou", "texto secreto do agente");
        assert_eq!(serde_json::from_str::<Value>(&rx.try_recv().unwrap()).unwrap(), json!({"kind": "asks", "paneId": "p1"}));
        // o mesmo painel de novo dentro de 30 s: nada
        s.notify_pane("p1", Kind::Asks, "Agente perguntou", "outra");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn finished_needs_twenty_seconds_of_work() {
        assert!(!finished_worth_notice(Duration::from_secs(19)));
        assert!(finished_worth_notice(Duration::from_secs(20)));
    }

    #[test]
    fn one_notice_per_pane_per_thirty_seconds() {
        let now = Instant::now();
        assert!(rate_allows(None, now));
        assert!(!rate_allows(Some(now), now + Duration::from_secs(29)));
        assert!(rate_allows(Some(now), now + Duration::from_secs(30)));
    }

    #[test]
    fn visible_only_when_window_focused_and_pane_active() {
        assert!(pane_visible(true, true));
        assert!(!pane_visible(false, true), "janela fora de foco: avisa");
        assert!(!pane_visible(true, false), "painel fora da tela: avisa");
        assert!(!pane_visible(false, false));
    }
}
