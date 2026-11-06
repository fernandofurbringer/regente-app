//! Atualização automática assinada (Configurações → Atualizações) e "Novidades" depois de atualizar.
//! O canal é `settings.update_endpoint`; vazio = "sem canal configurado". A rede só é usada por um clique
//! em "Procurar agora". A chave pública fica em `tauri.conf.json`; a privada nunca entra no repositório.

use crate::error::{fail, AppError, AppResult};
use crate::runtime::AppState;
use crate::util::lk;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Platform key of the Windows NSIS installer in latest.json.
pub const PLATFORM: &str = "windows-x86_64";
const NOTES_FILE: &str = "update-notes.json";

/// The update found by the last check, waiting for "Instalar".
#[derive(Default)]
pub struct Pending(pub Mutex<Option<Update>>);

/// latest.json as the updater reads it: version, notes, and the signed installer of each platform.
pub fn latest_json(version: &str, notes: &str, signature: &str, url: &str) -> Value {
    json!({ "version": version, "notes": notes, "platforms": { PLATFORM: { "signature": signature, "url": url } } })
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Platform {
    pub signature: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub platforms: BTreeMap<String, Platform>,
}

/// Parses a latest.json. The Windows entry is required: without it there is nothing to install.
pub fn parse_manifest(text: &str) -> AppResult<Manifest> {
    let m: Manifest = serde_json::from_str(text).map_err(|e| AppError::new("update_manifest", e.to_string()))?;
    if m.version.trim().is_empty() {
        return fail("update_manifest", "latest.json sem versão");
    }
    if !m.platforms.contains_key(PLATFORM) {
        return fail("update_manifest", "latest.json sem a plataforma windows-x86_64");
    }
    Ok(m)
}

/// The update channel from the setting: trimmed; empty = not configured.
pub fn channel(setting: &str) -> Option<String> {
    let t = setting.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// The notes of an installed update, shown once on the first boot of that version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notes {
    pub version: String,
    pub notes: String,
}

/// Shows the notes only for the version that was installed (`current`).
pub fn whats_new(stored: Option<Notes>, current: &str) -> Option<Notes> {
    stored.filter(|n| n.version == current)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// "nao-configurado" | "atualizado" | "disponivel"
    pub state: &'static str,
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "event", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DownloadEvent {
    Started { content_length: Option<u64> },
    Progress { chunk: usize },
    Finished,
}

fn notes_path(data: &Path) -> PathBuf {
    data.join(NOTES_FILE)
}

fn save_notes(data: &Path, n: &Notes) -> AppResult<()> {
    let text = serde_json::to_string(n).map_err(|e| AppError::new("update", e.to_string()))?;
    std::fs::write(notes_path(data), text)?;
    Ok(())
}

fn load_notes(data: &Path) -> Option<Notes> {
    serde_json::from_str(&std::fs::read_to_string(notes_path(data)).ok()?).ok()
}

#[tauri::command(async)]
pub async fn update_check(app: AppHandle, s: State<'_, Arc<AppState>>, pending: State<'_, Pending>) -> AppResult<UpdateStatus> {
    let current = app.package_info().version.to_string();
    let setting = lk(&s.settings).update_endpoint.clone();
    let Some(url) = channel(&setting) else {
        *lk(&pending.0) = None;
        return Ok(UpdateStatus { state: "nao-configurado", current, version: None, notes: None });
    };
    let endpoint = tauri::Url::parse(&url).map_err(|e| AppError::new("update_endpoint", e.to_string()))?;
    // on Windows the plugin exits the process after starting the installer: the same shutdown as a normal exit runs first
    let h = app.clone();
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| AppError::new("update", e.to_string()))?
        .on_before_exit(move || crate::graceful_shutdown(&h))
        .build()
        .map_err(|e| AppError::new("update", e.to_string()))?;
    let found = updater.check().await.map_err(|e| AppError::new("update", e.to_string()))?;
    match found {
        Some(u) => {
            let status = UpdateStatus { state: "disponivel", current, version: Some(u.version.clone()), notes: u.body.clone() };
            *lk(&pending.0) = Some(u);
            Ok(status)
        }
        None => {
            *lk(&pending.0) = None;
            Ok(UpdateStatus { state: "atualizado", current, version: None, notes: None })
        }
    }
}

#[tauri::command(async)]
pub async fn update_install(s: State<'_, Arc<AppState>>, pending: State<'_, Pending>, on_event: Channel<DownloadEvent>) -> AppResult<()> {
    let Some(update) = lk(&pending.0).take() else {
        return fail("no_update", "verifique atualizações antes de instalar");
    };
    let version = update.version.clone();
    let notes = update.body.clone().unwrap_or_default();
    // saved before the install: on Windows the process exits inside `download_and_install`, so nothing after it runs.
    // The notes show on the first boot of `version`.
    save_notes(&s.data, &Notes { version, notes })?;
    let mut started = false;
    update
        .download_and_install(
            |chunk, total| {
                if !started {
                    started = true;
                    let _ = on_event.send(DownloadEvent::Started { content_length: total });
                }
                let _ = on_event.send(DownloadEvent::Progress { chunk });
            },
            || {
                let _ = on_event.send(DownloadEvent::Finished);
            },
        )
        .await
        .map_err(|e| AppError::new("update", e.to_string()))?;
    Ok(())
}

#[tauri::command]
pub fn whats_new_get(app: AppHandle, s: State<'_, Arc<AppState>>) -> Option<Notes> {
    whats_new(load_notes(&s.data), &app.package_info().version.to_string())
}

#[tauri::command]
pub fn whats_new_seen(s: State<'_, Arc<AppState>>) {
    let _ = std::fs::remove_file(notes_path(&s.data));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_json_has_the_updater_format() {
        let v = latest_json("1.0.0", "Novidades", "SIG", "https://ex.org/Regente_1.0.0_x64-setup.exe");
        assert_eq!(v["version"], "1.0.0");
        assert_eq!(v["notes"], "Novidades");
        assert_eq!(v["platforms"]["windows-x86_64"]["signature"], "SIG");
        assert_eq!(v["platforms"]["windows-x86_64"]["url"], "https://ex.org/Regente_1.0.0_x64-setup.exe");
    }

    #[test]
    fn manifest_written_by_latest_json_parses_back() {
        let text = latest_json("1.2.3", "texto", "SIG", "https://ex.org/a.exe").to_string();
        let m = parse_manifest(&text).unwrap();
        assert_eq!(m.version, "1.2.3");
        assert_eq!(m.notes, "texto");
        assert_eq!(m.platforms[PLATFORM], Platform { signature: "SIG".into(), url: "https://ex.org/a.exe".into() });
    }

    #[test]
    fn manifest_without_windows_or_version_is_refused() {
        let no_windows = r#"{"version":"1.0.0","platforms":{"linux-x86_64":{"signature":"s","url":"u"}}}"#;
        assert!(parse_manifest(no_windows).is_err());
        let no_version = r#"{"version":" ","platforms":{"windows-x86_64":{"signature":"s","url":"u"}}}"#;
        assert!(parse_manifest(no_version).is_err());
        assert!(parse_manifest("não é json").is_err());
    }

    #[test]
    fn empty_channel_means_not_configured() {
        assert_eq!(channel(""), None);
        assert_eq!(channel("   "), None);
        assert_eq!(channel(" https://ex.org/latest.json "), Some("https://ex.org/latest.json".into()));
    }

    #[test]
    fn whats_new_shows_only_for_the_installed_version() {
        let stored = Some(Notes { version: "1.1.0".into(), notes: "x".into() });
        assert!(whats_new(stored.clone(), "1.1.0").is_some());
        assert!(whats_new(stored, "1.0.0").is_none(), "an older build does not get the new notes");
        assert!(whats_new(None, "1.1.0").is_none());
    }

    #[test]
    fn notes_roundtrip_through_the_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_notes(dir.path()).is_none());
        save_notes(dir.path(), &Notes { version: "2.0.0".into(), notes: "tudo novo".into() }).unwrap();
        assert_eq!(load_notes(dir.path()), Some(Notes { version: "2.0.0".into(), notes: "tudo novo".into() }));
    }
}
