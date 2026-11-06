//! Servidor MCP do Regente de ponta a ponta: o caminho de `mcp::start` (HTTP real, porta livre, token)
//! e, sob demanda, um Claude Code real que chama `app_health` e cujos hooks chegam ao app.

use regente_lib::mcp;
use regente_lib::providers::{PaneKind, PaneSpec, Permission};
use regente_lib::runtime::{start_ticker, AppState};
use regente_lib::status::PaneStatus;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn state(dir: &std::path::Path, exe: PathBuf) -> Arc<AppState> {
    AppState::new(dir.into(), exe, Arc::new(|_: &str, _: Value| {})).unwrap()
}

/// The `data:` line of an SSE response that carries a JSON-RPC answer.
fn sse_result(body: &str) -> Value {
    let line = body.lines().filter_map(|l| l.strip_prefix("data:")).find(|d| d.contains("\"id\"")).expect("sse data line");
    serde_json::from_str(line.trim()).unwrap()
}

#[tokio::test]
async fn mcp_start_serves_app_health_over_http() {
    let d = tempfile::tempdir().unwrap();
    let s = state(d.path(), PathBuf::from("regente.exe"));
    let info = mcp::start(s.clone()).unwrap();
    let mcp_url = info.mcp_url();
    // a pane's token, as plan_for registers it; the pane row must exist (a caller with no row is refused)
    s.db.insert_pane(&regente_lib::db::PaneRow { id: "P1".into(), workspace_id: "w".into(), kind: "agent".into(), name: "p1".into(), permission: "pend".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), ..Default::default() }).unwrap();
    s.rt().tokens.insert("e2e-token".into(), "P1".into());
    let auth = "Bearer e2e-token".to_string();
    let accept = "application/json, text/event-stream";
    let c = reqwest::Client::new();

    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}}});
    let r = c.post(&mcp_url).header("Authorization", &auth).header("Accept", accept).json(&init).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let sid = r.headers()["mcp-session-id"].to_str().unwrap().to_string();
    sse_result(&r.text().await.unwrap());

    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "app_health", "arguments": {}}});
    let r = c.post(&mcp_url).header("Authorization", &auth).header("Accept", accept).header("mcp-session-id", &sid).json(&call).send().await.unwrap();
    let v = sse_result(&r.text().await.unwrap());
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("\"ok\": true") && text.contains("\"panes\": 0"), "{text}");

    // the same server rejects anyone without the token
    let r = c.post(&mcp_url).json(&call).send().await.unwrap();
    assert_eq!(r.status(), 401);
    s.shutdown();
}

/// Real Claude Code: spends tokens and needs `claude` on PATH and logged in.
/// Run with: `cargo test --test claude_mcp_e2e -- --ignored --nocapture`
#[test]
#[ignore = "usa o Claude Code real (gasta tokens; precisa do claude instalado e logado)"]
fn claude_calls_app_health_and_hooks_arrive() {
    claude_e2e(None);
}

/// Same run, but the pane is a role pane, so Claude also gets `--plugin-dir` with the role skill.
/// Run with: `cargo test --test claude_mcp_e2e claude_loads_plugin_dir -- --ignored --nocapture`
#[test]
#[ignore = "usa o Claude Code real (gasta tokens; precisa do claude instalado e logado)"]
fn claude_loads_plugin_dir_and_calls_app_health() {
    claude_e2e(Some("executor"));
}

fn claude_e2e(role: Option<&str>) {
    // debug logs (MCP auth failures are logged at debug) go to the test output
    let _ = tracing_subscriber::fmt().with_env_filter("regente_lib=debug").with_test_writer().try_init();
    let data = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let git = std::process::Command::new("git").arg("init").arg("-q").current_dir(repo.path()).status().unwrap();
    assert!(git.success());
    // Normal permission mode. The Regente tools are pre-allowed by the pane's generated settings
    // (`allow_regente_tools`, default on), not by an extra CLI arg. The providers file lives in the test's own
    // data dir, so the user's Claude config is untouched and no bypass mode is involved.
    std::fs::write(data.path().join("providers.json"), regente_lib::settings::PROVIDERS_DEFAULT).unwrap();
    // the hook command runs this very binary (`regente --hook <Evento>`), not the test harness
    let events: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let log = events.clone();
    let s = AppState::new(
        data.path().into(),
        PathBuf::from(env!("CARGO_BIN_EXE_regente")),
        Arc::new(move |n: &str, v: Value| log.lock().unwrap().push(format!("{n} {v}"))),
    )
    .unwrap();
    s.mcp.set(mcp::start(s.clone()).unwrap()).unwrap();
    start_ticker(s.clone());
    let ws = s.db.upsert_workspace(&repo.path().to_string_lossy()).unwrap();
    let row = s
        .spawn_pane(PaneSpec {
            workspace_id: ws.id.clone(),
            kind: PaneKind::Agent,
            cli: Some("claude".into()),
            model: Some("claude-haiku-5-5".into()),
            effort: Some("low".into()),
            permission: Permission::Pend,
            cwd: repo.path().to_string_lossy().into(),
            mcp_profile: Some("minimal".into()),
            role: role.map(Into::into),
            ..Default::default()
        })
        .expect("painel do claude (claude instalado e no PATH?)");
    if role.is_some() {
        // the plugin folder reaches the CLI as `--plugin-dir <dir>`, and it holds the role skill
        // a .cmd shim runs through cmd.exe: the real command line is in REGENTE_CMDLINE (plan.env)
        let plan = s.panes.plan(&row.id).expect("plano do processo");
        let line = plan.env.iter().map(|(_, v)| v.clone()).chain(plan.args.iter().cloned()).collect::<Vec<_>>().join(" ");
        let dir = data.path().join("panes").join(&row.id).join("plugin");
        assert!(line.contains("--plugin-dir") && line.contains(&dir.to_string_lossy().to_string()), "{line}");
        assert!(dir.join(".claude-plugin/plugin.json").is_file());
        assert!(dir.join("skills/executar/SKILL.md").is_file());
    }
    let id = row.id.clone();
    // the ring is gone once the process exits: keep the output for the failure message
    let output: Arc<std::sync::Mutex<Vec<u8>>> = Arc::default();
    let out = output.clone();
    s.attach(&id, "e2e", Box::new(move |b: &[u8]| {
        out.lock().unwrap().extend_from_slice(b);
        true
    }))
    .unwrap();

    // A fresh folder opens with Claude's trust dialog ("No, exit" is preselected): accept it (Down + Enter).
    let plain = |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&b.lock().unwrap()).replace("\x1b[1C", " ");
    let t0 = Instant::now();
    while !(plain(&output).contains("trust") && plain(&output).contains("folder")) {
        assert!(t0.elapsed() < Duration::from_secs(60), "sem diálogo de confiança nem prompt em 60 s");
        std::thread::sleep(Duration::from_millis(200));
    }
    // wait until the dialog stopped redrawing, then Down and Enter as separate keystrokes
    let mut last = usize::MAX;
    while output.lock().unwrap().len() != last {
        last = output.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(800));
    }
    s.write_input(&id, b"\x1b[B").unwrap();
    std::thread::sleep(Duration::from_millis(800));
    s.write_input(&id, b"\r").unwrap();
    // Initial prompt only once the UI has settled (an idle pane with no new output for 3 s), pasted like the app does
    let (mut last_len, mut quiet_since) = (0usize, Instant::now());
    loop {
        let len = output.lock().unwrap().len();
        if len != last_len {
            (last_len, quiet_since) = (len, Instant::now());
        }
        if quiet_since.elapsed() > Duration::from_secs(3) && s.status_of(&id) == Some(PaneStatus::Idle) {
            break;
        }
        assert!(quiet_since.elapsed() < Duration::from_secs(90), "a interface não assentou em 90 s");
        std::thread::sleep(Duration::from_millis(200));
    }
    let prompt = "Chame a ferramenta mcp__regente__app_health e depois responda só FIM.";
    s.write_input(&id, &regente_lib::orchestrator::bracketed(prompt)).unwrap();

    let (mut worked, mut stopped) = (false, false);
    let t0 = Instant::now();
    loop {
        let hook = s.rt().hook_state.get(&id).map(|(st, _)| *st);
        worked |= hook == Some(PaneStatus::Working);
        stopped |= worked && hook == Some(PaneStatus::Idle);
        let screen = String::from_utf8_lossy(&s.panes.snapshot(&id)).to_string();
        // the tool call itself: the server emitted `mcp-call` for this pane (the FIM text alone proves nothing)
        let call = events.lock().unwrap().iter().any(|e| e.starts_with("mcp-call") && e.contains(&format!("\"pane\":\"{id}\"")));
        if stopped && screen.contains("FIM") && call {
            break;
        }
        let seen = String::from_utf8_lossy(&output.lock().unwrap()).to_string();
        assert!(
            t0.elapsed() < Duration::from_secs(120),
            "sem Stop + FIM + chamada app_health em 120 s (working={worked}, stop={stopped}, call={call}, hook={hook:?}, status={:?}, alive={}).\nevents: {:#?}\noutput tail: {}",
            s.status_of(&id),
            s.panes.is_alive(&id),
            events.lock().unwrap().iter().rev().take(15).collect::<Vec<_>>(),
            seen.chars().rev().take(1500).collect::<String>().chars().rev().collect::<String>()
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    s.close_pane(&id).unwrap();
    s.shutdown();
}
