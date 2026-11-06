use crate::error::{fail, AppError, AppResult};
use crate::ring::Ring;
use crate::spill::{Spill, SpillWriter};
use std::path::PathBuf;
use crate::util::lk;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const RING_BYTES: usize = 2 * 1024 * 1024;
const DSR: &[u8] = b"\x1b[6n";
const CPR: &[u8] = b"\x1b[1;1R";

/// What to run in a pane. Built by `providers::build_spawn`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnPlan {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
    pub session_id: Option<String>,
    pub route_label: Option<String>,
    /// Bytes to paste (bracketed) once the pane first goes idle; not part of the persisted plan.
    #[serde(skip)]
    pub initial_input: Option<Vec<u8>>,
}

const INHERITED_SESSION_VARS: [&str; 10] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// Receives output bytes; returns `false` when the sink is dead (it gets removed).
pub type Sink = Box<dyn Fn(&[u8]) -> bool + Send>;
pub type ExitFn = Arc<dyn Fn(&str, u32) + Send + Sync>;

struct Buffers {
    ring: Ring,
    pending: Vec<u8>,
    /// (sink id, owner = webview label, sink)
    sinks: Vec<(u64, String, Sink)>,
}

struct Live {
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    bufs: Mutex<Buffers>,
    /// Disk history of the output that left the ring (set by `enable_spill`); written off the read thread.
    spill: Mutex<Option<SpillWriter>>,
    started: Instant,
    last_output: Mutex<Option<Instant>>,
    /// Last write to the pane (user input or a delivered prompt): the idle clock of auto-hibernation.
    last_input: Mutex<Instant>,
    typed: AtomicBool,
    plan: SpawnPlan,
}

type LiveMap = Arc<Mutex<HashMap<String, Arc<Live>>>>;

/// Failed exits (code != 0): uptime and last output, taken once by `on_exit` (the live entry is gone by then).
type Exited = Arc<Mutex<HashMap<String, (Duration, Vec<u8>)>>>;

pub struct PaneManager {
    live: LiveMap,
    exited: Exited,
    next_sink: AtomicU64,
    on_exit: ExitFn,
}

fn flush(l: &Live) {
    let mut b = lk(&l.bufs);
    if b.pending.is_empty() {
        return;
    }
    let data = std::mem::take(&mut b.pending);
    b.sinks.retain(|(_, _, s)| s(&data));
}

/// True if the chunk has output beyond CSI/OSC control sequences (conhost's own preamble is not output).
fn visible(chunk: &[u8]) -> bool {
    let mut i = 0;
    while i < chunk.len() {
        match chunk[i] {
            0x1b if chunk.get(i + 1) == Some(&b'[') => {
                i += 2;
                while i < chunk.len() && !(0x40..=0x7e).contains(&chunk[i]) {
                    i += 1;
                }
                i += 1;
            }
            0x1b if chunk.get(i + 1) == Some(&b']') => {
                i += 2;
                while i < chunk.len() && chunk[i] != 0x07 && !(chunk[i] == 0x1b && chunk.get(i + 1) == Some(&b'\\')) {
                    i += 1;
                }
                i += if chunk.get(i) == Some(&0x1b) { 2 } else { 1 };
            }
            c if c >= 0x20 => return true,
            _ => i += 1,
        }
    }
    false
}

fn read_loop(l: Arc<Live>, mut reader: Box<dyn Read + Send>) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut dsr_done = false;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut chunk = buf[..n].to_vec();
        // ConPTY blocks the child until it gets a CPR for its startup DSR. Rust always answers it
        // and strips it, so xterm never sees (and re-answers on replay) it.
        if !dsr_done {
            if let Some(pos) = chunk.windows(DSR.len()).position(|w| w == DSR) {
                dsr_done = true;
                if let Some(w) = lk(&l.writer).as_mut() {
                    let _ = w.write_all(CPR);
                }
                chunk.drain(pos..pos + DSR.len());
            }
        }
        let evicted = {
            let mut b = lk(&l.bufs);
            let evicted = b.ring.push_evicted(&chunk);
            if !b.sinks.is_empty() {
                b.pending.extend_from_slice(&chunk);
            }
            evicted
        };
        // o que saiu do buffer em memória vai para a thread de escrita (nunca espera disco aqui)
        if !evicted.is_empty() {
            if let Some(sp) = lk(&l.spill).as_ref() {
                sp.push(evicted);
            }
        }
        if visible(&chunk) {
            *lk(&l.last_output) = Some(Instant::now());
        }
    }
}

impl PaneManager {
    pub fn new(on_exit: ExitFn) -> Self {
        Self { live: Arc::default(), exited: Arc::default(), next_sink: AtomicU64::new(1), on_exit }
    }

    fn get(&self, id: &str) -> AppResult<Arc<Live>> {
        lk(&self.live).get(id).cloned().ok_or_else(|| AppError::new("not_running", format!("painel {id} não está rodando")))
    }

    pub fn spawn(&self, id: &str, plan: &SpawnPlan, cols: u16, rows: u16) -> AppResult<()> {
        let sp = |e: String| AppError::new("spawn_failed", e);
        // fast path; the Entry check below still covers the race
        if self.is_alive(id) {
            return fail("already_running", format!("painel {id} já está rodando"));
        }
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| sp(e.to_string()))?;
        let mut cmd = CommandBuilder::new(&plan.program);
        cmd.args(&plan.args);
        cmd.cwd(&plan.cwd);
        // Marcadores de sessão herdados quando o Regente é aberto de dentro do Claude Code: o
        // claude do painel se acharia sessão filha (sem transcript, medidor de custo em zero).
        for k in INHERITED_SESSION_VARS {
            cmd.env_remove(k);
        }
        // teto de compilação compartilhado: os `cargo` de todas as panes disputam as mesmas vagas
        for (k, v) in crate::compilacao::ambiente() {
            cmd.env(k, v);
        }
        for (k, v) in &plan.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| sp(format!("{}: {e}", plan.program)))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|e| sp(e.to_string()))?;
        let writer = pair.master.take_writer().map_err(|e| sp(e.to_string()))?;
        let live = Arc::new(Live {
            writer: Mutex::new(Some(writer)),
            master: Mutex::new(Some(pair.master)),
            killer: Mutex::new(child.clone_killer()),
            pid: child.process_id(),
            bufs: Mutex::new(Buffers { ring: Ring::new(RING_BYTES), pending: Vec::new(), sinks: Vec::new() }),
            spill: Mutex::new(None),
            started: Instant::now(),
            last_output: Mutex::new(None),
            last_input: Mutex::new(Instant::now()),
            typed: AtomicBool::new(false),
            plan: plan.clone(),
        });
        match lk(&self.live).entry(id.to_string()) {
            Entry::Occupied(_) => {
                let _ = child.kill();
                return fail("already_running", format!("painel {id} já está rodando"));
            }
            Entry::Vacant(v) => v.insert(live.clone()),
        };
        let undo = |e: std::io::Error| {
            let _ = lk(&live.killer).kill();
            lk(&self.live).remove(id);
            // close ConPTY so a reader thread that did start gets EOF (drop outside the mutexes)
            let w = lk(&live.writer).take();
            drop(w);
            let m = lk(&live.master).take();
            drop(m);
            AppError::from(e)
        };
        let reader_handle = {
            let l = live.clone();
            thread::Builder::new().name(format!("pty-read-{id}")).spawn(move || read_loop(l, reader)).map_err(&undo)?
        };
        let (map, exited, on_exit, wid, wlive) = (self.live.clone(), self.exited.clone(), self.on_exit.clone(), id.to_string(), live.clone());
        thread::Builder::new().name(format!("pty-wait-{id}")).spawn(move || {
            let (id, live) = (wid, wlive);
            let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
            // On Windows the reader only gets EOF after writer AND master are dropped.
            // Drop outside the mutexes (closing ConPTY can block).
            let w = lk(&live.writer).take();
            drop(w);
            let m = lk(&live.master).take();
            drop(m);
            let _ = reader_handle.join();
            flush(&live);
            if code != 0 {
                lk(&exited).insert(id.clone(), (live.started.elapsed(), lk(&live.bufs).ring.snapshot()));
            }
            {
                let mut map = lk(&map);
                if map.get(&id).is_some_and(|l| Arc::ptr_eq(l, &live)) {
                    map.remove(&id);
                }
            }
            on_exit(&id, code);
        }).map_err(&undo)?;
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> AppResult<()> {
        let l = self.get(id)?;
        if data.contains(&b'\r') {
            l.typed.store(false, Ordering::Relaxed);
        } else if data.first() != Some(&0x1b) && data.iter().any(|b| *b >= 0x20 && *b != 0x7f) {
            l.typed.store(true, Ordering::Relaxed);
        }
        *lk(&l.last_input) = Instant::now();
        let mut w = lk(&l.writer);
        match w.as_mut() {
            Some(w) => Ok(w.write_all(data)?),
            None => fail("not_running", "painel encerrando"),
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let l = self.get(id)?;
        if let Some(m) = lk(&l.master).as_ref() {
            m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| AppError::new("resize_failed", e.to_string()))?;
        }
        Ok(())
    }

    pub fn kill(&self, id: &str) -> AppResult<()> {
        // portable-pty 0.9 WinChildKiller::kill returns Err on success (inverted check): result intentionally ignored.
        if let Ok(l) = self.get(id) {
            let _ = lk(&l.killer).kill();
        }
        Ok(())
    }

    pub fn attach(&self, id: &str, sink: Sink) -> AppResult<u64> {
        self.attach_owned(id, "", sink)
    }

    /// Sends the ring replay to the new sink, then live output. Returns the sink id.
    /// `owner` (the webview label) lets `detach_owner` drop the sinks of a reloaded/closed window.
    pub fn attach_owned(&self, id: &str, owner: &str, sink: Sink) -> AppResult<u64> {
        let l = self.get(id)?;
        let sid = self.next_sink.fetch_add(1, Ordering::Relaxed);
        let mut b = lk(&l.bufs);
        let pending = std::mem::take(&mut b.pending);
        if !pending.is_empty() {
            b.sinks.retain(|(_, _, s)| s(&pending));
        }
        // always sent, even empty: the UI treats the first message as the replay
        let snap = b.ring.snapshot();
        if sink(&snap) {
            b.sinks.push((sid, owner.to_string(), sink));
        }
        Ok(sid)
    }

    pub fn detach(&self, id: &str, sink_id: u64) {
        if let Ok(l) = self.get(id) {
            let mut b = lk(&l.bufs);
            b.sinks.retain(|(s, _, _)| *s != sink_id);
        }
    }

    /// Drops every sink of a webview (page reload or window closed): its channels are dead.
    pub fn detach_owner(&self, owner: &str) {
        let lives: Vec<Arc<Live>> = lk(&self.live).values().cloned().collect();
        for l in lives {
            lk(&l.bufs).sinks.retain(|(_, o, _)| o != owner);
        }
    }

    pub fn flush_all(&self) {
        let lives: Vec<Arc<Live>> = lk(&self.live).values().cloned().collect();
        for l in lives {
            flush(&l);
        }
    }

    pub fn is_alive(&self, id: &str) -> bool {
        lk(&self.live).contains_key(id)
    }

    pub fn ids(&self) -> Vec<String> {
        lk(&self.live).keys().cloned().collect()
    }

    pub fn pid(&self, id: &str) -> Option<u32> {
        self.get(id).ok().and_then(|l| l.pid)
    }

    pub fn plan(&self, id: &str) -> Option<SpawnPlan> {
        self.get(id).ok().map(|l| l.plan.clone())
    }

    /// (spawn time, last output time)
    pub fn activity(&self, id: &str) -> Option<(Instant, Option<Instant>)> {
        self.get(id).ok().map(|l| (l.started, *lk(&l.last_output)))
    }

    /// When the pane was last written to (start included); None if it is not running.
    pub fn last_input(&self, id: &str) -> Option<Instant> {
        self.get(id).ok().map(|l| *lk(&l.last_input))
    }

    /// true if the user typed something since the last Enter (then `/rename` is not sent).
    pub fn typed_since_enter(&self, id: &str) -> bool {
        self.get(id).map(|l| l.typed.load(Ordering::Relaxed)).unwrap_or(false)
    }

    /// Output that leaves the in-memory ring from now on is appended to `dir` (see `spill`).
    pub fn enable_spill(&self, id: &str, dir: PathBuf) {
        if let Ok(l) = self.get(id) {
            *lk(&l.spill) = Some(SpillWriter::spawn(Spill::new(dir)));
        }
    }

    pub fn snapshot(&self, id: &str) -> Vec<u8> {
        self.get(id).map(|l| lk(&l.bufs).ring.snapshot()).unwrap_or_default()
    }

    /// Uptime and last output of a pane that exited with an error; consumed by the caller.
    pub fn take_exited(&self, id: &str) -> Option<(Duration, Vec<u8>)> {
        lk(&self.exited).remove(id)
    }
}

#[cfg(test)]
mod tests {
    use super::visible;
    #[test]
    fn visible_skips_control_sequences() {
        assert!(!visible(b"\x1b[?9001h\x1b[?1004h\x1b]0;cmd.exe\x07\x1b[?25h"));
        assert!(!visible(b"\x1b]0;title\x07"));
        assert!(!visible(b"\x1b]0;title\x1b\\"));
        assert!(visible(b"\x1b[32mok"));
    }
}
