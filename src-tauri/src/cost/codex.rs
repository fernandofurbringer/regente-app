use super::prices::Prices;
use super::{CostSnapshot, Usage};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Default)]
pub struct CodexAgg {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub usage: Usage,
    pub rate_limits: Option<Value>,
}

impl CodexAgg {
    pub fn apply_line(&mut self, line: &str) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let p = &v["payload"];
        match v["type"].as_str() {
            Some("session_meta") => {
                self.session_id = p["id"].as_str().map(String::from);
                self.cwd = p["cwd"].as_str().map(String::from);
            }
            Some("turn_context") => {
                if let Some(m) = p["model"].as_str() {
                    self.model = Some(m.into());
                }
            }
            Some("event_msg") if p["type"] == "token_count" => {
                let t = &p["info"]["total_token_usage"];
                if t.is_object() {
                    let n = |k: &str| t[k].as_u64().unwrap_or(0);
                    let cached = n("cached_input_tokens");
                    self.usage = Usage { input: n("input_tokens").saturating_sub(cached), output: n("output_tokens"), cache_read: cached, ..Default::default() };
                }
                if p["rate_limits"].is_object() {
                    self.rate_limits = Some(p["rate_limits"].clone());
                }
            }
            _ => {}
        }
    }

    pub fn snapshot(&self, prices: &Prices) -> CostSnapshot {
        let model = self.model.clone().unwrap_or_default();
        CostSnapshot { usage: self.usage, cost_usd: prices.cost(&model, &self.usage), model: self.model.clone(), session_id: self.session_id.clone(), ..Default::default() }
    }
}

pub fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// (id, cwd) from a rollout's first line.
pub fn first_meta(path: &Path) -> Option<(String, String)> {
    let mut line = String::new();
    // só a primeira linha interessa (session_meta): lê no máximo 64 KB, sem varrer o rollout inteiro
    BufReader::new(std::fs::File::open(path).ok()?.take(64 * 1024)).read_line(&mut line).ok()?;
    let mut a = CodexAgg::default();
    a.apply_line(&line);
    Some((a.session_id?, a.cwd?))
}

/// With `session_id` (a restored pane): the rollout of that session, at any date — `codex resume` appends to the
/// original file, so `since`/`claimed` don't apply. Otherwise: the OLDEST unclaimed rollout from today/yesterday
/// created at/after `since` with the same cwd (ties: path order).
// ponytail: two panes started within the same ~100 ms in one folder can swap rollouts; match the first prompt if that happens.
pub fn find_rollout(home: &Path, cwd: &str, since: SystemTime, claimed: &HashSet<PathBuf>, session_id: Option<&str>) -> Option<PathBuf> {
    let base = PathBuf::from(glob::Pattern::escape(&home.join("sessions").to_string_lossy()));
    if let Some(sid) = session_id {
        // ponytail: relies on codex naming rollouts `rollout-<ts>-<id>.jsonl` (no first-line scan of every rollout per poll); scan all if that changes
        let pattern = base.join("*").join("*").join("*").join(format!("rollout-*{}*.jsonl", glob::Pattern::escape(sid)));
        return glob::glob(&pattern.to_string_lossy()).ok()?.flatten().find(|p| first_meta(p).is_some_and(|(id, _)| id == sid));
    }
    let now = chrono::Local::now();
    let days = [now, now - chrono::Duration::days(1)];
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for d in days {
        let pattern = base.join(d.format("%Y").to_string()).join(d.format("%m").to_string()).join(d.format("%d").to_string()).join("rollout-*.jsonl");
        for p in glob::glob(&pattern.to_string_lossy()).ok()?.flatten() {
            if claimed.contains(&p) {
                continue;
            }
            let Ok(c) = std::fs::metadata(&p).and_then(|m| m.created()) else { continue };
            if c < since || first_meta(&p).map(|(_, c)| norm(&c)) != Some(norm(cwd)) {
                continue;
            }
            if best.as_ref().is_none_or(|b| (c, &p) < (b.0, &b.1)) {
                best = Some((c, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PRICES_DEFAULT;
    use std::time::Duration;

    #[test]
    fn parses_totals_model_session_and_limits() {
        let prices: Prices = serde_json::from_str(PRICES_DEFAULT).unwrap();
        let mut a = CodexAgg::default();
        for l in include_str!("../../tests/fixtures/codex.jsonl").lines() {
            a.apply_line(l);
        }
        assert_eq!(a.usage, Usage { input: 4000, output: 900, cache_read: 8000, ..Default::default() });
        assert_eq!(a.session_id.as_deref(), Some("codex-sess-1"));
        assert_eq!(a.rate_limits.as_ref().unwrap()["primary"]["used_percent"], 13.0);
        // gpt-5: (4000*1.25 + 900*10 + 8000*0.125)/1e6
        assert!((a.snapshot(&prices).cost_usd - 0.015).abs() < 1e-9);
    }

    #[test]
    fn finds_rollout_by_cwd() {
        let d = tempfile::tempdir().unwrap();
        let now = chrono::Local::now();
        let dir = d.path().join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = include_str!("../../tests/fixtures/codex.jsonl");
        std::fs::write(dir.join("rollout-a.jsonl"), fixture).unwrap();
        std::fs::write(dir.join("rollout-b.jsonl"), fixture.replace("D:\\\\proj\\\\site", "D:\\\\outro")).unwrap();
        let since = SystemTime::now() - Duration::from_secs(60);
        let found = find_rollout(d.path(), "d:/proj/site/", since, &HashSet::new(), None).unwrap();
        assert!(found.ends_with("rollout-a.jsonl"));
        let claimed: HashSet<PathBuf> = [found].into();
        assert!(find_rollout(d.path(), "D:\\proj\\site", since, &claimed, None).is_none());
    }

    #[test]
    fn picks_oldest_unclaimed_and_ignores_older_than_since() {
        let d = tempfile::tempdir().unwrap();
        let now = chrono::Local::now();
        let dir = d.path().join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = include_str!("../../tests/fixtures/codex.jsonl");
        std::fs::write(dir.join("rollout-z.jsonl"), fixture).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let since = SystemTime::now();
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(dir.join("rollout-b.jsonl"), fixture).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(dir.join("rollout-a.jsonl"), fixture).unwrap();
        let f = find_rollout(d.path(), "D:\\proj\\site", since, &HashSet::new(), None).unwrap();
        assert!(f.ends_with("rollout-b.jsonl"), "{f:?}");
        let claimed: HashSet<PathBuf> = [f].into();
        assert!(find_rollout(d.path(), "D:\\proj\\site", since, &claimed, None).unwrap().ends_with("rollout-a.jsonl"));
    }

    #[test]
    fn known_session_is_found_by_id_at_any_date_ignoring_since_and_claimed() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("sessions").join("2020").join("01").join("02");
        let now = chrono::Local::now();
        let today = d.path().join("sessions").join(now.format("%Y").to_string()).join(now.format("%m").to_string()).join(now.format("%d").to_string());
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&today).unwrap();
        let fixture = include_str!("../../tests/fixtures/codex.jsonl");
        let target = old.join("rollout-2020-01-02T00-00-00-codex-sess-1.jsonl");
        std::fs::write(&target, fixture).unwrap();
        std::fs::write(today.join("rollout-x-codex-sess-2.jsonl"), fixture.replace("codex-sess-1", "codex-sess-2")).unwrap();
        let claimed: HashSet<PathBuf> = [target.clone()].into();
        let f = find_rollout(d.path(), "D:\\outro", SystemTime::now(), &claimed, Some("codex-sess-1"));
        assert_eq!(f, Some(target));
        assert!(find_rollout(d.path(), "D:\\proj\\site", SystemTime::now(), &HashSet::new(), Some("nao-existe")).is_none());
    }
}
