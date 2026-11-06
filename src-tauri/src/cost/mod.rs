pub mod claude;
pub mod codex;
pub mod gemini;
pub mod opencode;
pub mod prices;
pub mod waste;

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Parte de `cache_write` gravada com TTL de 1 hora (cobrada a 2x entrada). Sempre <= `cache_write`.
    pub cache_write_1h: u64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.cache_write_1h += o.cache_write_1h;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSnapshot {
    pub usage: Usage,
    pub cost_usd: f64,
    pub model: Option<String>,
    pub session_id: Option<String>,
    /// Só o Claude: mensagens do assistente (únicas) e entrada da primeira (ver `claude::ClaudeAgg`).
    pub turns: u32,
    pub first_turn_tokens: Option<u64>,
}

use prices::Prices;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub enum Source {
    Claude { config_dir: PathBuf, session_id: String },
    /// `session_id`: known session (restore) — its rollout is matched by id, at any date.
    Codex { home: PathBuf, cwd: String, since: SystemTime, session_id: Option<String> },
    Gemini { dir: PathBuf, since: SystemTime },
    /// OpenCode: sessão principal mais nova da pasta `cwd` criada a partir de `since`, no SQLite `db`.
    OpenCode { db: PathBuf, cwd: String, since: SystemTime },
    /// Claude em workspace remoto: o JSONL é lido por ssh a cada `remote::COST_EVERY`. Só Claude.
    Remote { alias: String, session_id: String },
}

/// Read position of one tailed file; `partial` holds the bytes of an incomplete last line.
#[derive(Default)]
struct TailFile {
    offset: u64,
    partial: Vec<u8>,
}

impl TailFile {
    /// Complete new lines since the last call (at most 8 MiB read per call).
    // ponytail: 8 MiB per poll bounds memory/latency; a huge backlog is consumed over several polls.
    fn new_lines(&mut self, path: &Path) -> Vec<String> {
        let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0; // file truncated/recreated
            self.partial.clear();
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return vec![];
        }
        let mut bytes = Vec::new();
        let _ = (&mut f).take(8 << 20).read_to_end(&mut bytes);
        self.take(&bytes)
    }

    /// Feeds bytes read from the file (at `offset`); returns the complete lines.
    fn take(&mut self, bytes: &[u8]) -> Vec<String> {
        self.offset += bytes.len() as u64;
        self.partial.extend_from_slice(bytes);
        // decode only complete lines: a multi-byte char split across polls stays intact in `partial`
        let Some(cut) = self.partial.iter().rposition(|b| *b == b'\n') else { return vec![] };
        let complete: Vec<u8> = self.partial.drain(..=cut).collect();
        String::from_utf8_lossy(&complete).lines().map(String::from).collect()
    }
}

pub struct Tailer {
    pub source: Source,
    pub path: Option<PathBuf>,
    main: TailFile,
    subs: HashMap<PathBuf, TailFile>,
    claude: claude::ClaudeAgg,
    codex: codex::CodexAgg,
    gemini_mtime: Option<SystemTime>,
    last: CostSnapshot,
    /// Remote source: the next allowed read.
    next_remote: Option<std::time::Instant>,
    /// Remote source: consecutive failed reads (drives the backoff).
    fails: u32,
}

impl Tailer {
    pub fn new(source: Source) -> Self {
        Self { source, path: None, main: Default::default(), subs: HashMap::new(), claude: Default::default(), codex: Default::default(), gemini_mtime: None, last: CostSnapshot::default(), next_remote: None, fails: 0 }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.source, Source::Remote { .. })
    }

    /// A sessão Claude foi copiada byte a byte de `from` para `to` (troca de conta): segue lendo os arquivos novos de
    /// onde parou, sem reler o que já contou.
    pub fn relocate_claude(&mut self, from: &Path, to: &Path) {
        if let Source::Claude { config_dir, .. } = &mut self.source {
            *config_dir = to.to_path_buf();
        }
        let re = |p: &Path| p.strip_prefix(from).map(|r| to.join(r)).unwrap_or_else(|_| p.to_path_buf());
        self.path = self.path.as_deref().map(re);
        self.subs = std::mem::take(&mut self.subs).into_iter().map(|(k, v)| (re(&k), v)).collect();
    }

    /// The next poll runs right away (the pane just exited: its last tokens must be read).
    pub fn reset_remote(&mut self) {
        self.next_remote = None;
    }

    /// Rate limits seen in the Codex rollout (input for the Redline in F3).
    pub fn rate_limits(&self) -> Option<&serde_json::Value> {
        self.codex.rate_limits.as_ref()
    }

    /// Reads what is new. `Some` when totals changed.
    pub fn poll(&mut self, prices: &Prices, claimed: &HashSet<PathBuf>) -> Option<CostSnapshot> {
        if let Source::Remote { .. } = self.source {
            let now = std::time::Instant::now();
            if self.next_remote.is_some_and(|t| now < t) {
                return None;
            }
            self.next_remote = Some(now + crate::remote::COST_EVERY);
        }
        if self.path.is_none() {
            self.path = match &self.source {
                Source::Claude { config_dir, session_id } => claude::find_session_file(config_dir, session_id),
                Source::Codex { home, cwd, since, session_id } => codex::find_rollout(home, cwd, *since, claimed, session_id.as_deref()),
                Source::Gemini { dir, since } => gemini::find_chat(dir, *since, claimed),
                Source::OpenCode { db, cwd, since } => opencode::find_session(db, cwd, *since, claimed),
                // ponytail: subagent transcripts are not read remotely yet (main session only)
                Source::Remote { alias, session_id } => crate::remote::claude_session_file(alias, session_id).ok().flatten().map(PathBuf::from),
            };
        }
        let path = self.path.clone()?;
        let snap = match &self.source {
            Source::Remote { alias, .. } => match crate::remote::remote_tail(alias, &path.to_string_lossy(), self.main.offset) {
                Ok((size, bytes)) => {
                    self.fails = 0;
                    if size < self.main.offset {
                        // arquivo encolheu (rotação): relê do começo
                        self.main.offset = 0;
                        self.main.partial.clear();
                    } else {
                        for l in self.main.take(&bytes) {
                            self.claude.apply_line(&l);
                        }
                    }
                    self.claude.snapshot(prices)
                }
                Err(_) => {
                    self.fails += 1;
                    self.next_remote = Some(std::time::Instant::now() + crate::remote::cost_backoff(self.fails));
                    return None;
                }
            },
            Source::Gemini { .. } => {
                let m = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if m == self.gemini_mtime {
                    return None;
                }
                self.gemini_mtime = m;
                match std::fs::read_to_string(&path).ok().and_then(|t| gemini::snapshot(&t, prices)) {
                    Some(s) => s,
                    None => {
                        self.gemini_mtime = None; // half-written file: retry on the next poll
                        return None;
                    }
                }
            }
            Source::OpenCode { .. } => {
                // o caminho sintético é <banco>/<id da sessão> (ver cost/opencode.rs)
                let sid = path.file_name()?.to_string_lossy().into_owned();
                // banco ocupado ou ilegível: tenta de novo na próxima leitura
                opencode::snapshot(path.parent()?, &sid, prices)?
            }
            Source::Claude { .. } => {
                for l in self.main.new_lines(&path) {
                    self.claude.apply_line(&l);
                }
                for sub in claude::subagent_files(&path) {
                    for l in self.subs.entry(sub.clone()).or_default().new_lines(&sub) {
                        self.claude.apply_line(&l);
                    }
                }
                self.claude.snapshot(prices)
            }
            Source::Codex { .. } => {
                for l in self.main.new_lines(&path) {
                    self.codex.apply_line(&l);
                }
                self.codex.snapshot(prices)
            }
        };
        if snap == self.last {
            return None;
        }
        self.last = snap.clone();
        Some(snap)
    }
}

#[cfg(test)]
mod tailer_tests {
    use super::*;
    use std::io::Write;

    fn prices() -> Prices {
        serde_json::from_str(crate::settings::PRICES_DEFAULT).unwrap()
    }

    #[test]
    fn claude_tailer_reads_incrementally_and_waits_partial_lines() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("projects").join("D--p");
        std::fs::create_dir_all(&proj).unwrap();
        let file = proj.join("sid.jsonl");
        let prices = prices();
        let mut t = Tailer::new(Source::Claude { config_dir: d.path().into(), session_id: "sid".into() });
        let none = HashSet::new();
        assert!(t.poll(&prices, &none).is_none(), "file does not exist yet");

        let a = r#"{"type":"assistant","message":{"id":"A","model":"claude-haiku-5-5","usage":{"input_tokens":100,"output_tokens":10}}}"#;
        std::fs::write(&file, format!("{a}\n")).unwrap();
        assert_eq!(t.poll(&prices, &none).unwrap().usage.input, 100);
        assert!(t.poll(&prices, &none).is_none(), "nothing new");

        let b = r#"{"type":"assistant","message":{"id":"B","model":"claude-haiku-5-5","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        write!(f, "{}", &b[..20]).unwrap();
        assert!(t.poll(&prices, &none).is_none(), "partial line ignored");
        writeln!(f, "{}", &b[20..]).unwrap();
        assert_eq!(t.poll(&prices, &none).unwrap().usage.input, 101);
    }

    #[test]
    fn relocated_tailer_keeps_its_place_in_the_copied_session() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("contaA"), d.path().join("contaB"));
        let line = |id: &str, n: u32| format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-haiku-5-5\",\"usage\":{{\"input_tokens\":{n},\"output_tokens\":1}}}}}}
");
        let proj_a = a.join("projects").join("D--p");
        std::fs::create_dir_all(proj_a.join("sid").join("subagents")).unwrap();
        std::fs::write(proj_a.join("sid.jsonl"), line("A", 100)).unwrap();
        std::fs::write(proj_a.join("sid").join("subagents").join("agent-1.jsonl"), line("S1", 7)).unwrap();
        let prices = prices();
        let none = HashSet::new();
        let mut t = Tailer::new(Source::Claude { config_dir: a.clone(), session_id: "sid".into() });
        assert_eq!(t.poll(&prices, &none).unwrap().usage.input, 107);
        // troca de conta: a sessão vai byte a byte para B e continua lá
        crate::troca_conta::copiar_sessao(&proj_a.join("sid.jsonl"), &b).unwrap();
        t.relocate_claude(&a, &b);
        assert!(t.poll(&prices, &none).is_none(), "nada é contado de novo");
        let proj_b = b.join("projects").join("D--p");
        let mut f = std::fs::OpenOptions::new().append(true).open(proj_b.join("sid.jsonl")).unwrap();
        write!(f, "{}", line("B", 5)).unwrap();
        let mut g = std::fs::OpenOptions::new().append(true).open(proj_b.join("sid").join("subagents").join("agent-1.jsonl")).unwrap();
        write!(g, "{}", line("S2", 3)).unwrap();
        assert_eq!(t.poll(&prices, &none).unwrap().usage.input, 115);
    }

    #[test]
    fn claude_tailer_includes_subagents() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("projects").join("D--p");
        let subs = proj.join("sid").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let line = |id: &str, n: u32| format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-haiku-5-5\",\"usage\":{{\"input_tokens\":{n},\"output_tokens\":1}}}}}}\n");
        std::fs::write(proj.join("sid.jsonl"), line("A", 100)).unwrap();
        let prices = prices();
        let mut t = Tailer::new(Source::Claude { config_dir: d.path().into(), session_id: "sid".into() });
        let none = HashSet::new();
        assert_eq!(t.poll(&prices, &none).unwrap().usage.input, 100);
        std::fs::write(subs.join("agent-1.jsonl"), line("S1", 7)).unwrap();
        assert_eq!(t.poll(&prices, &none).unwrap().usage.input, 107);
        assert!(t.poll(&prices, &none).is_none());
    }

    #[test]
    fn same_message_id_in_main_and_subagent_counts_once() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("projects").join("D--p");
        let subs = proj.join("sid").join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        let line = |id: &str, n: u32| format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-haiku-5-5\",\"usage\":{{\"input_tokens\":{n},\"output_tokens\":1}}}}}}\n");
        std::fs::write(proj.join("sid.jsonl"), line("A", 100)).unwrap();
        std::fs::write(subs.join("agent-1.jsonl"), line("A", 100)).unwrap();
        let prices = prices();
        let mut t = Tailer::new(Source::Claude { config_dir: d.path().into(), session_id: "sid".into() });
        assert_eq!(t.poll(&prices, &HashSet::new()).unwrap().usage.input, 100);
    }

    #[test]
    fn big_file_is_consumed_over_several_polls() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("projects").join("D--p");
        std::fs::create_dir_all(&proj).unwrap();
        let mut w = std::io::BufWriter::new(std::fs::File::create(proj.join("sid.jsonl")).unwrap());
        let pad = "x".repeat(150);
        let mut n = 0u64;
        let mut bytes = 0usize;
        while bytes < 20 << 20 {
            let l = format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"m{n}\",\"model\":\"claude-haiku-5-5\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"pad\":\"{pad}\"}}}}\n");
            bytes += l.len();
            w.write_all(l.as_bytes()).unwrap();
            n += 1;
        }
        drop(w);
        let prices = prices();
        let mut t = Tailer::new(Source::Claude { config_dir: d.path().into(), session_id: "sid".into() });
        let none = HashSet::new();
        let (mut polls, mut last) = (0, None);
        while let Some(s) = t.poll(&prices, &none) {
            polls += 1;
            last = Some(s);
        }
        assert!(polls >= 3, "{polls}");
        assert_eq!(last.unwrap().usage.input, n);
    }

    #[test]
    fn gemini_half_written_file_is_retried() {
        let d = tempfile::tempdir().unwrap();
        let chats = d.path().join("tmp").join("h").join("chats");
        std::fs::create_dir_all(&chats).unwrap();
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        let f = chats.join("session-1.json");
        std::fs::write(&f, "{\"sessionId\":").unwrap();
        let prices = prices();
        let mut t = Tailer::new(Source::Gemini { dir: d.path().into(), since });
        let none = HashSet::new();
        assert!(t.poll(&prices, &none).is_none());
        std::fs::write(&f, include_str!("../../tests/fixtures/gemini.json")).unwrap();
        assert_eq!(t.poll(&prices, &none).unwrap().usage.cache_read, 1900);
    }
}
