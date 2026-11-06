use regente_lib::pty::{PaneManager, SpawnPlan};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn plan(args: &[&str]) -> SpawnPlan {
    SpawnPlan {
        program: "cmd.exe".into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        cwd: std::env::temp_dir().to_string_lossy().into(),
        ..Default::default()
    }
}

fn manager() -> (Arc<PaneManager>, mpsc::Receiver<(String, u32)>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let m = Arc::new(PaneManager::new(Arc::new(move |id: &str, code: u32| {
        let _ = tx.lock().unwrap().send((id.to_string(), code));
    })));
    (m, rx)
}

/// Drops OSC sequences (`ESC ] … BEL` or `ESC ] … ESC \`).
fn strip_osc(b: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b && b.get(i + 1) == Some(&b']') {
            i += 2;
            while i < b.len() && b[i] != 0x07 && !(b[i] == 0x1b && b.get(i + 1) == Some(&b'\\')) {
                i += 1;
            }
            i += if b.get(i) == Some(&0x1b) { 2 } else { 1 };
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[test]
fn echo_without_sink_answers_dsr_and_exits() {
    let (m, rx) = manager();
    m.spawn("p1", &plan(&["/c", "echo regente"]), 80, 24).unwrap();
    let (id, code) = rx.recv_timeout(Duration::from_secs(15)).expect("process did not exit (DSR not answered?)");
    assert_eq!((id.as_str(), code), ("p1", 0));
    assert!(!m.is_alive("p1"));
}

#[test]
fn late_sink_gets_replay_then_live_output() {
    let (m, rx) = manager();
    m.spawn("p2", &plan(&["/c", "echo um & ping -n 2 127.0.0.1 >nul & echo dois"]), 80, 24).unwrap();
    let t0 = Instant::now();
    while !String::from_utf8_lossy(&m.snapshot("p2")).contains("um") {
        assert!(t0.elapsed() < Duration::from_secs(10), "no output");
        std::thread::sleep(Duration::from_millis(20));
    }
    let got = Arc::new(Mutex::new(Vec::<u8>::new()));
    let g = got.clone();
    m.attach("p2", Box::new(move |b: &[u8]| { g.lock().unwrap().extend_from_slice(b); true })).unwrap();
    assert!(String::from_utf8_lossy(&got.lock().unwrap()).contains("um"), "no replay");
    loop {
        m.flush_all();
        if rx.try_recv().is_ok() { break; }
        assert!(t0.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(16));
    }
    // cmd puts the running command line in the window title (OSC), which also contains "um"
    let all = strip_osc(&got.lock().unwrap());
    assert!(all.contains("dois"), "no live output");
    assert_eq!(all.matches("um").count(), 1, "replay duplicated or lost output");
    assert!(!all.contains("\x1b[6n"), "startup DSR leaked to the sink");
}

#[test]
fn write_reaches_interactive_shell() {
    let (m, rx) = manager();
    m.spawn("p3", &plan(&["/q"]), 80, 24).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    m.write("p3", b"echo ola-regente\r").unwrap();
    let t0 = Instant::now();
    while !String::from_utf8_lossy(&m.snapshot("p3")).contains("ola-regente") {
        assert!(t0.elapsed() < Duration::from_secs(10), "echo not seen");
        std::thread::sleep(Duration::from_millis(20));
    }
    m.write("p3", b"exit\r").unwrap();
    rx.recv_timeout(Duration::from_secs(15)).expect("did not exit");
}

#[test]
fn kill_ends_process() {
    let (m, rx) = manager();
    m.spawn("p4", &plan(&["/q"]), 80, 24).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    m.kill("p4").unwrap();
    rx.recv_timeout(Duration::from_secs(10)).expect("kill did not end the process");
}

#[test]
fn detach_stops_delivery() {
    let (m, rx) = manager();
    m.spawn("p5", &plan(&["/q"]), 80, 24).unwrap();
    let n = Arc::new(Mutex::new(0usize));
    let n2 = n.clone();
    let sid = m.attach("p5", Box::new(move |b: &[u8]| { *n2.lock().unwrap() += b.len(); true })).unwrap();
    // a second sink stays attached: proves output keeps flowing, just not to the detached one
    let other = Arc::new(Mutex::new(Vec::<u8>::new()));
    let o2 = other.clone();
    m.attach("p5", Box::new(move |b: &[u8]| { o2.lock().unwrap().extend_from_slice(b); true })).unwrap();
    m.detach("p5", sid);
    std::thread::sleep(Duration::from_millis(300));
    let before = *n.lock().unwrap();
    m.write("p5", b"echo depois\r").unwrap();
    let t0 = Instant::now();
    while !String::from_utf8_lossy(&other.lock().unwrap()).contains("depois") {
        assert!(t0.elapsed() < Duration::from_secs(10), "attached sink got no output");
        m.flush_all();
        std::thread::sleep(Duration::from_millis(20));
    }
    m.flush_all();
    assert_eq!(*n.lock().unwrap(), before);
    m.kill("p5").unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
}

#[test]
fn detach_owner_drops_only_that_owners_sinks() {
    let (m, rx) = manager();
    m.spawn("p7", &plan(&["/q"]), 80, 24).unwrap();
    let sink = |buf: &Arc<Mutex<Vec<u8>>>| {
        let b = buf.clone();
        Box::new(move |d: &[u8]| { b.lock().unwrap().extend_from_slice(d); true }) as regente_lib::pty::Sink
    };
    let (gone, kept) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
    m.attach_owned("p7", "janela-a", sink(&gone)).unwrap();
    m.attach_owned("p7", "janela-a", sink(&gone)).unwrap();
    m.attach_owned("p7", "main", sink(&kept)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    m.flush_all();
    m.detach_owner("janela-a");
    let before = gone.lock().unwrap().len();
    m.write("p7", b"echo depois\r").unwrap();
    let t0 = Instant::now();
    while !String::from_utf8_lossy(&kept.lock().unwrap()).contains("depois") {
        assert!(t0.elapsed() < Duration::from_secs(10), "kept sink got no output");
        m.flush_all();
        std::thread::sleep(Duration::from_millis(20));
    }
    m.flush_all();
    assert_eq!(gone.lock().unwrap().len(), before);
    m.kill("p7").unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
}

#[test]
fn inherited_claude_session_markers_do_not_reach_panes() {
    // Regente aberto de dentro de um terminal do Claude Code: o claude do painel desligaria o
    // transcript (e o medidor de custo ficaria em zero).
    unsafe { std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1") };
    let (m, rx) = manager();
    m.spawn("p8", &plan(&["/c", "echo [%CLAUDE_CODE_CHILD_SESSION%] & ping -n 2 127.0.0.1 >nul"]), 80, 24).unwrap();
    let t0 = Instant::now();
    let out = loop {
        let s = strip_osc(&m.snapshot("p8"));
        if s.contains("[1]") || s.contains("[%CLAUDE_CODE_CHILD_SESSION%]") { break s; }
        assert!(t0.elapsed() < Duration::from_secs(10), "no output");
        std::thread::sleep(Duration::from_millis(20));
    };
    unsafe { std::env::remove_var("CLAUDE_CODE_CHILD_SESSION") };
    assert!(out.contains("[%CLAUDE_CODE_CHILD_SESSION%]"), "marker leaked: {out}");
    rx.recv_timeout(Duration::from_secs(15)).unwrap();
}

#[test]
fn typed_ignores_escape_sequences() {
    let (m, rx) = manager();
    m.spawn("p6", &plan(&["/q"]), 80, 24).unwrap();
    m.write("p6", b"\x1b[I").unwrap();
    assert!(!m.typed_since_enter("p6"));
    m.write("p6", b"x").unwrap();
    assert!(m.typed_since_enter("p6"));
    m.kill("p6").unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
}
