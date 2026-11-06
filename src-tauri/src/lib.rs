pub mod economia;
pub mod autoajuste;
pub mod hygiene;
pub mod agentic;
pub mod base_skill;
pub mod attention;
pub mod briefing;
pub mod budget;
pub mod catalog;
pub mod cronjobs;
pub mod cli_version;
pub mod commands;
pub mod cost;
#[cfg(feature = "codeindex")]
pub mod codeindex;
pub mod db;
pub mod diag;
pub mod dictation;
pub mod error;
pub mod files;
pub mod fswatch;
pub mod resources;
pub mod github;
pub mod gitflow;
pub mod git;
pub mod hibernate;
pub mod hookcli;
pub mod jarvis;
pub mod marketplace;
pub mod media;
pub mod mcp;
pub mod memory;
// ▼ trilha C (M1c)
pub mod embed;
pub mod modelos;
pub mod voz_local;
pub mod whisper_local;
// ▲ trilha C (M1c)
pub mod missions;
pub mod mobile;
pub mod notify;
pub mod orchestrator;
pub mod prompts;
pub mod providers;
pub mod recipes;
pub mod redline;
pub mod remote;
pub mod review;
pub mod pty;
pub mod queue;
pub mod ring;
pub mod runtime;
pub mod search;
#[cfg(windows)]
pub mod session_end;
pub mod sessions;
pub mod settings;
pub mod shot;
pub mod spill;
pub mod squads;
pub mod status;
pub mod stt;
pub mod statusline;
pub mod transcript;
pub mod update;
pub mod util;
pub mod vault;
pub mod worktrees;
// ▼ trilha A
pub mod ajustes;
pub mod compactacao;
pub mod compilacao;
pub mod troca_conta;
pub mod conectores;
pub mod perfil;
pub mod planexec;
// ▲ trilha A
// ▼ trilha B
pub mod checkpoints;
pub mod instrucoes;
// ▲ trilha B
// ▼ trilha C
pub mod projeto;
pub mod testes;
pub mod formato;
pub mod artefatos;
pub mod execmatch;
pub mod execucoes;
pub mod trim;
pub mod contexto;
pub mod verificacao;
pub mod worktree_cache;
pub mod fases;
pub mod compare;
pub mod away;
// ▲ trilha C

use runtime::{AppState, Emit};
use std::sync::Arc;
use tauri::webview::PageLoadEvent;
use tauri::{Emitter, Manager, Runtime, Url, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// Everything that ends before the process does: the app state, the global hotkeys, a Shot recording, local whisper,
/// the phone server, and the log. Used on `ExitRequested` and by the updater, which exits the process itself after
/// starting the installer (so nothing after `download_and_install` runs on Windows).
pub fn graceful_shutdown(app: &tauri::AppHandle) {
    if let Some(s) = app.try_state::<Arc<AppState>>() {
        s.shutdown();
    }
    dictation::shutdown(app);
    shot::shutdown(app);
    stt::stop_local();
    if let Some(m) = app.try_state::<mobile::Mobile>() {
        m.stop();
    }
    util::lk(&LOG_GUARD).take();
}

/// First argument that is not a flag (path given to `Regente.exe <path>` or by a file association).
pub fn path_arg(args: &[String]) -> Option<String> {
    args.iter().skip(1).find(|a| !a.starts_with('-')).cloned()
}

/// Main-frame navigation stays on the app origin; other http(s) links open in the user's browser.
fn app_origin(url: &Url) -> bool {
    let host = url.host_str().unwrap_or("");
    url.scheme() == "tauri"
        || (matches!(url.scheme(), "http" | "https") && host == "tauri.localhost")
        || (cfg!(debug_assertions) && url.scheme() == "http" && host == "localhost" && url.port() == Some(1420))
}

fn nav_guard<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("nav")
        .on_navigation(|_webview, url| {
            if app_origin(url) {
                return true;
            }
            if matches!(url.scheme(), "http" | "https") {
                let _ = tauri_plugin_opener::open_url(url.as_str(), None::<&str>);
            }
            false
        })
        .build()
}

/// Guard do log não-bloqueante: `App::run` encerra o processo sem rodar destrutores, então o guard é
/// descartado explicitamente em `RunEvent::Exit` para não perder as últimas linhas.
static LOG_GUARD: std::sync::Mutex<Option<tracing_appender::non_blocking::WorkerGuard>> = std::sync::Mutex::new(None);

/// Perfil do WebView2 preso por outra instância: a trava existe e não abre para escrita (o processo que a tem a segura).
fn webview_profile_busy(dir: &std::path::Path) -> bool {
    let lock = dir.join("EBWebView").join("lockfile");
    lock.exists() && std::fs::OpenOptions::new().write(true).open(&lock).is_err()
}

/// O WebView2 engole alguns atalhos (Ctrl+Shift+N, F5, Ctrl+R…) antes da página: desliga os atalhos de navegador
/// para que os do Regente cheguem ao app e a interface não recarregue por acidente.
#[cfg(windows)]
fn sem_atalhos_do_navegador(webview: &tauri::Webview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows_core::Interface;
    let _ = webview.with_webview(|pw| {
        // SAFETY: chamadas COM na thread da janela, sobre o controlador que o próprio Tauri entrega
        unsafe {
            if let Ok(s3) = pw.controller().CoreWebView2().and_then(|c| c.Settings()).and_then(|s| s.cast::<ICoreWebView2Settings3>()) {
                let _ = s3.SetAreBrowserAcceleratorKeysEnabled(false);
            }
        }
    });
}

#[cfg(not(windows))]
fn sem_atalhos_do_navegador(_: &tauri::Webview) {}

/// Último nível aplicado à WebView principal (true = memória baixa): evita chamar o COM a cada evento de janela.
static WEBVIEW_BAIXA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Janela minimizada ou escondida: a WebView2 entra em "memória baixa" (o Chromium descarta caches e encolhe os
/// processos da tela); ao voltar, nível normal. É o "carregar quando usa, soltar quando ausente" aplicado à interface.
#[cfg(windows)]
fn memoria_da_webview(w: &tauri::WebviewWindow, baixa: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL};
    use windows_core::Interface;
    let nivel = if baixa { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
    let _ = w.with_webview(move |pw| {
        // SAFETY: chamadas COM na thread da janela, sobre o controlador que o próprio Tauri entrega; WebView2 antiga sem
        // a interface _19 só não muda nada
        unsafe {
            if let Ok(c19) = pw.controller().CoreWebView2().and_then(|c| c.cast::<ICoreWebView2_19>()) {
                let _ = c19.SetMemoryUsageTargetLevel(nivel);
            }
        }
    });
}

#[cfg(not(windows))]
fn memoria_da_webview(_: &tauri::WebviewWindow, _: bool) {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // com REGENTE_HOME (teste, instância isolada), o WebView2 usa <REGENTE_HOME>/webview: não divide o perfil do usuário.
    // A variável vale para todas as janelas, inclusive as criadas pelo JS (que não recebem data_directory).
    let webview_dir = std::env::var_os("REGENTE_HOME").map(|h| std::path::PathBuf::from(h).join("webview"));
    if let Some(dir) = &webview_dir {
        // SAFETY: antes de qualquer thread do app e de qualquer janela existir
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", dir) };
    }
    let busy_dir = webview_dir.filter(|d| webview_profile_busy(d));
    let data = util::data_dir();
    *util::lk(&LOG_GUARD) = Some(diag::init_logging(&data.join("logs")));
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        tracing::error!("panic: {} ({loc})", info.payload_as_str().unwrap_or("?"));
        prev(info);
    }));
    tracing::info!("Regente {} iniciando", env!("CARGO_PKG_VERSION"));
    // instância isolada (REGENTE_HOME) não usa a trava de instância única: roda ao lado do Regente do dia a dia
    // sem entregar a ele os argumentos e sair. Mesmo identificador, dados separados.
    let builder = tauri::Builder::default();
    let builder = if std::env::var_os("REGENTE_HOME").is_some() {
        builder
    } else {
        builder
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let Some(w) = app.get_webview_window("main").or_else(|| app.webview_windows().into_values().next()) else { return };
            if let Some(p) = path_arg(&args) {
                // `regente .` em outro terminal: resolve relativo ao cwd DE QUEM chamou (join mantém absolutos)
                let p = std::path::Path::new(&cwd).join(&p).to_string_lossy().to_string();
                let _ = app.emit_to(w.label(), "open-path", p);
            }
            let _ = w.unminimize();
            let _ = w.set_focus();
        }))
    };
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(dictation::plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(nav_guard())
        // a reloaded page or a closed window leaves dead channels behind: drop their sinks
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                if let Some(s) = webview.try_state::<Arc<AppState>>() {
                    s.panes.detach_owner(webview.label());
                }
                sem_atalhos_do_navegador(webview);
            }
        })
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, WindowEvent::Resized(_) | WindowEvent::Focused(_)) {
                let baixa = window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(true);
                if WEBVIEW_BAIXA.swap(baixa, std::sync::atomic::Ordering::Relaxed) != baixa {
                    if let Some(w) = window.app_handle().get_webview_window("main") {
                        memoria_da_webview(&w, baixa);
                    }
                }
            }
            if let WindowEvent::Focused(_) = event {
                if let Some(s) = window.try_state::<Arc<AppState>>() {
                    let focused = dictation::regente_focused(window.app_handle());
                    s.window_focused.store(focused, std::sync::atomic::Ordering::Relaxed);
                    // aviso que chegou com a janela fora de foco: o foco de volta mostra "ir para o painel"
                    if focused {
                        let panes = s.take_pending_notices();
                        if let Some(last) = panes.last() {
                            let _ = window.app_handle().emit("notify-pending", serde_json::json!({"paneId": last, "paneIds": panes}));
                        }
                    }
                }
            }
            if let WindowEvent::Destroyed = event {
                // the main window closing takes the detached ones with it; each runs its own unsaved-changes prompt
                if window.label() == "main" {
                    for w in window.app_handle().webview_windows().into_values() {
                        let _ = w.close();
                    }
                }
                if let Some(s) = window.try_state::<Arc<AppState>>() {
                    s.panes.detach_owner(window.label());
                }
            }
        })
        .setup(move |app| {
            let h = app.handle().clone();
            let emit: Emit = Arc::new(move |ev: &str, payload: serde_json::Value| {
                if ev == "os-notify" {
                    notify::show_os_notice(&h, &payload);
                }
                let _ = h.emit(ev, payload);
            });
            let state = match std::env::current_exe()
                .map_err(Into::into)
                .and_then(|exe| AppState::new(data.clone(), exe, emit))
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("falha ao iniciar o estado: {e}");
                    util::lk(&LOG_GUARD).take(); // flush do log antes de sair
                    app.dialog()
                        .message(format!("Não foi possível iniciar: {e}"))
                        .title("Regente")
                        .kind(MessageDialogKind::Error)
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            if let Some(dir) = &busy_dir {
                tracing::error!("perfil do WebView2 em uso por outra instância: {}", dir.display());
                app.dialog()
                    .message(format!("O perfil do WebView2 desta instância ({}) está em uso por outro Regente que ficou aberto. Feche esse processo (Regente ou msedgewebview2 desse perfil) pelo Gerenciador de Tarefas e abra o Regente de novo.", dir.display()))
                    .title("Regente")
                    .kind(MessageDialogKind::Error)
                    .blocking_show();
            }
            // without MCP the app still works (no orchestration tools, no hooks)
            match mcp::start(state.clone()) {
                Ok(info) => {
                    let _ = state.mcp.set(info);
                }
                Err(e) => tracing::warn!("servidor MCP indisponível, seguindo sem orquestração: {e}"),
            }
            runtime::start_ticker(state.clone());
            notify::on_notice_click(app.handle());
            fswatch::start(state.clone());
            // pastas de missões arquivadas com arquivos em uso: no boot nada as segura, sai em segundo plano
            {
                let s = state.clone();
                let _ = std::thread::Builder::new().name("regente-arquivadas".into()).spawn(move || commands::retry_archived_worktrees(&s));
            }
            // ▼ trilha C (M1c)
            embed::iniciar(state.clone());
            // ▲ trilha C (M1c)
            state.window_focused.store(dictation::regente_focused(app.handle()), std::sync::atomic::Ordering::Relaxed);
            app.manage(state.clone());
            app.manage(dictation::Dictation::default());
            app.manage(shot::Shot::default());
            // the phone server stays off until the user starts it in Settings → Celular
            app.manage(mobile::Mobile::default());
            app.manage(update::Pending::default());
            // the Shot hotkeys are registered only when the user turned them on (off by default)
            if let Err(e) = shot::init(app.handle(), &state.data) {
                tracing::warn!("atalhos de captura indisponíveis: {}", e.to_string());
            }
            // the global dictation hotkey is registered only when the user turned it on (off by default)
            if let Err(e) = dictation::sync(app.handle(), &stt::load(&state.data)) {
                tracing::warn!("ditado global indisponível: {}", e.to_string());
            }
            if let Err(e) = dictation::build_tray(app.handle()) {
                tracing::warn!("bandeja indisponível, seguindo sem ela: {e}");
            }
            redline::start(state.clone(), app.handle().clone());
            // instância isolada (REGENTE_HOME): o título diz que é teste, para ninguém confundir com o Regente do dia a dia
            if let (Some(home), Some(w)) = (std::env::var_os("REGENTE_HOME"), app.get_webview_window("main")) {
                let _ = w.set_title(&format!("Regente (teste isolado: {})", std::path::Path::new(&home).file_name().unwrap_or_default().to_string_lossy()));
            }
            #[cfg(windows)]
            if let Some(w) = app.get_webview_window("main") {
                if let Ok(h) = w.hwnd() {
                    session_end::install(app.handle(), h.0 as isize);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info, commands::startup_path, commands::cli_version_check, commands::sessions_list, commands::session_resume, commands::brief_suggest,
            commands::workspace_open, commands::workspace_list, commands::workspace_remove,
            commands::pane_list, commands::pane_spawn, commands::pane_attach, commands::pane_detach, commands::subagent_items, commands::exec_lista, commands::exec_get, commands::exec_fixar, commands::exec_desligar, commands::exec_desligado, commands::exec_painel, commands::exec_promover, commands::exec_fechar_concluidas,
            commands::pane_write, commands::pane_resize, commands::pane_close, commands::pane_rename,
            commands::pane_reorder, commands::mission_attention, commands::mcp_servers_list, commands::workspace_sensitive_set, commands::base_skill_status, commands::base_skill_install, commands::base_skill_remove, commands::pane_group_list, commands::pane_group_create, commands::pane_group_update, commands::pane_group_delete, commands::pane_set_group, commands::pane_hibernate, commands::pane_wake, commands::pane_set_resident,
            commands::pane_frame, commands::pane_details, commands::pane_history, commands::restore_list, commands::restore_apply,
            commands::mission_create, commands::mission_list, commands::mission_update, commands::worktrees_list, commands::mission_adopt, commands::mission_worktree_state,
            commands::mission_integrate, commands::mission_archive, commands::mission_diff_files, commands::mission_file_versions, commands::mission_revert_file, commands::panes_visible, commands::git_status, commands::diff_text, commands::git_commit_suggest, commands::git_commit, commands::git_commit_prefill, commands::git_pr_suggest, commands::git_pr_open, commands::git_pr_base, commands::git_pr_prefill, commands::spawn_cancel, commands::mission_totals, commands::recent_missions,
            commands::providers_get, commands::providers_save, commands::providers_detect, commands::route_test,
            commands::account_add, commands::account_remove, commands::prices_get, commands::prices_save,
            commands::settings_get, commands::settings_set, commands::terminal_font_from_wt,
            commands::vault_names, commands::vault_set, commands::vault_delete,
            commands::usage_rows, commands::usage_totals,
            commands::fs_list, commands::fs_index, commands::fs_git_marcas, commands::fs_git_base, commands::resources_snapshot, commands::fs_read, commands::fs_write, commands::fs_create,
            commands::fs_rename, commands::fs_copy, commands::perfil_preview, commands::fs_watch, commands::fs_unwatch, commands::fs_resolver_link, commands::fs_read_externo, commands::fs_delete, commands::image_check, commands::remote_sessions, commands::ssh_aliases,
            commands::diag_generate,
            commands::cron_list, commands::cron_save, commands::cron_delete, commands::cron_run_now, commands::cron_preview,
            commands::pane_focus, commands::pane_hibernate_reasons, commands::pane_compact, commands::pane_fresh_start,
            commands::catalog_agents, commands::catalog_skills, commands::catalog_save_agent,
            commands::marketplace_search, commands::marketplace_install, commands::marketplace_installed, commands::marketplace_uninstall,
            commands::gate_answer, commands::plan_get, commands::plan_resume, commands::gates_open,
            commands::tasks_list, commands::unseen_handoffs, commands::mission_mark_seen, commands::squads_list,
            commands::catalog_skill_text,
            commands::squad_save, commands::squad_former_brief,
            commands::memory_list, commands::memory_search, commands::memory_update, commands::memory_delete, commands::memory_toggle,
            commands::recipes_list, commands::recipe_save, commands::recipe_delete,commands::prompts_list, commands::prompts_save, commands::prompt_paste, commands::workspace_branch, commands::git_head, commands::git_is_repo, commands::git_init, commands::git_branches, commands::git_sync_state, commands::git_switch, commands::git_pull, commands::git_sync,commands::pane_transcript, commands::export_markdown, commands::search_panes, commands::search_pane_history,commands::pane_switch_model, commands::pane_switch_account, commands::workspace_last_account, commands::conectores_list, commands::conector_save, commands::conector_context7, commands::conector_toggle, commands::conector_delete, commands::conectores_importaveis, commands::conector_import, commands::gh_issue,
            commands::agentic_get, commands::agentic_save, commands::decisor_test, commands::decisions_list,
            commands::voice_get, commands::voice_save, commands::voice_transcribe,
            commands::voice_global_finish, commands::voice_global_cancel, commands::voice_history_list, commands::voice_history_clear,
            commands::media_get, commands::media_save, commands::media_test, commands::media_usage,
            commands::jarvis_get, commands::jarvis_save, commands::jarvis_write_confirmed,
            commands::shot_get, commands::shot_save, commands::shot_full, commands::shot_region_begin, commands::shot_region_commit,
            commands::shot_region_cancel, commands::shot_overlay_image, commands::shot_list,
            commands::shot_save_edit, commands::shot_edit_get, commands::shot_delete, commands::shot_read,
            commands::shot_record_begin, commands::shot_record_stop, commands::shot_extract,
            commands::redline_now, commands::redline_current, commands::redline_history, commands::redline_prefs, commands::redline_set_prefs, commands::redline_open,
            mobile::mobile_status, mobile::mobile_start, mobile::mobile_stop, mobile::mobile_pair_qr, mobile::mobile_revoke,
            mobile::mobile_show_code, mobile::mobile_approve,
            update::update_check, update::update_install, update::whats_new_get, update::whats_new_seen,
            // ▼ trilha A
            commands::perfis_get, commands::perfis_save, commands::perfil_efetivo, commands::mission_set_profile, commands::perfil_restore,
            commands::ajustes_list, commands::ajustes_undo, commands::ajustes_aplicar, commands::ajustes_ignorar, commands::autoajuste_get, commands::autoajuste_set_modo, commands::autoajuste_rodar, commands::autoajuste_zerar_aprendizado, commands::waste_summary, commands::code_index_stats, commands::code_index_set, commands::hygiene_scan, commands::economy_report, commands::quality_guard, commands::pane_perfil, commands::perfil_read_denies, commands::pane_idle_secs, commands::perfil_patch_remove,
            verificacao::mission_verify_get, verificacao::mission_verify_set, verificacao::mission_verify_run, verificacao::mission_verify_cancel, verificacao::mission_verify_enviar,
            // ▲ trilha A
            // ▼ trilha B
            commands::handoff_list,
            checkpoints::checkpoint_take, checkpoints::checkpoint_list, checkpoints::checkpoint_diff, checkpoints::checkpoint_restore,
            commands::memory_list_status, commands::memory_set_status, commands::memory_set_pinned, commands::memory_add_global,
            commands::instrucoes_listar, commands::instrucoes_sugerir, commands::instrucoes_aplicar,
            // ▲ trilha B
            // ▼ trilha C
            projeto::projeto_get, fases::mission_phases, projeto::projeto_save, projeto::projeto_trust, projeto::projeto_trusted, projeto::projeto_glossario,
            embed::embed_status, embed::embed_install, embed::embed_set_ativo, embed::embed_remove, embed::embed_cancel,
            voz_local::voz_local_status, voz_local::voz_local_instalar, voz_local::voz_local_cancelar, voz_local::voz_local_remover, voz_local::voz_local_ouvir,
            whisper_local::whisper_local_status, whisper_local::whisper_local_instalar, whisper_local::whisper_local_cancelar, whisper_local::whisper_local_remover,
            compare::compare_start, compare::compare_get, compare::compare_keep, compare::compare_espaco,
            away::away_data, away::away_resumir,
            // ▲ trilha C
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // no app handle yet, so no dialog: log, flush and exit
            tracing::error!("falha ao montar o Regente: {e}");
            util::lk(&LOG_GUARD).take();
            std::process::exit(1)
        })
        .run(|app, ev| match ev {
            tauri::RunEvent::ExitRequested { .. } => graceful_shutdown(app),
            tauri::RunEvent::Exit => {
                util::lk(&LOG_GUARD).take(); // flush do log
            }
            _ => {}
        });
}
