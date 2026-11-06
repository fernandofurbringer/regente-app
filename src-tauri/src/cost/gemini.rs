use super::prices::Prices;
use super::{CostSnapshot, Usage};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Gemini rewrites the whole chat JSON on every turn, so we re-read the whole file. None = invalid/partial JSON.
pub fn snapshot(text: &str, prices: &Prices) -> Option<CostSnapshot> {
    let v = serde_json::from_str::<Value>(text).ok()?;
    let mut usage = Usage::default();
    let mut cost = 0.0;
    let mut model = None;
    for m in v["messages"].as_array().into_iter().flatten().filter(|m| m["type"] == "gemini") {
        let t = &m["tokens"];
        let n = |k: &str| t[k].as_u64().unwrap_or(0);
        let u = Usage { input: n("input").saturating_sub(n("cached")), output: n("output") + n("thoughts"), cache_read: n("cached"), ..Default::default() };
        let name = m["model"].as_str().unwrap_or_default();
        cost += prices.cost(name, &u);
        usage.add(&u);
        if !name.is_empty() {
            model = Some(name.to_string());
        }
    }
    Some(CostSnapshot { usage, cost_usd: cost, model, session_id: v["sessionId"].as_str().map(String::from), ..Default::default() })
}

/// The OLDEST unclaimed chat created at/after `since` (ties: path order).
// ponytail: does not check the cwd (projectHash = sha256(cwd)); two Gemini panes started within the same ~100 ms can swap. Compare the hash if that happens.
pub fn find_chat(gemini_dir: &Path, since: SystemTime, claimed: &HashSet<PathBuf>) -> Option<PathBuf> {
    let base = PathBuf::from(glob::Pattern::escape(&gemini_dir.join("tmp").to_string_lossy()));
    let pattern = base.join("*").join("chats").join("session-*.json");
    glob::glob(&pattern.to_string_lossy())
        .ok()?
        .flatten()
        .filter(|p| !claimed.contains(p))
        .filter_map(|p| std::fs::metadata(&p).and_then(|m| m.created()).ok().filter(|c| *c >= since).map(|c| (c, p)))
        .min()
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PRICES_DEFAULT;
    #[test]
    fn sums_gemini_messages() {
        let prices: Prices = serde_json::from_str(PRICES_DEFAULT).unwrap();
        let s = snapshot(include_str!("../../tests/fixtures/gemini.json"), &prices).unwrap();
        assert_eq!(s.usage, Usage { input: 1100, output: 350, cache_read: 1900, ..Default::default() });
        // gemini-3: (1100*2 + 350*12 + 1900*0.2)/1e6
        assert!((s.cost_usd - 0.00678).abs() < 1e-9, "{}", s.cost_usd);
        assert_eq!(s.session_id.as_deref(), Some("g1"));
    }
    #[test]
    fn invalid_json_is_none() {
        assert_eq!(snapshot("{", &Prices::default()), None);
    }
    #[test]
    fn find_chat_picks_oldest_created_after_since() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("tmp").join("h").join("chats");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session-old.json"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let since = SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("session-b.json"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(dir.join("session-a.json"), "{}").unwrap();
        let f = find_chat(d.path(), since, &HashSet::new()).unwrap();
        assert!(f.ends_with("session-b.json"));
        let claimed: HashSet<PathBuf> = [f].into();
        assert!(find_chat(d.path(), since, &claimed).unwrap().ends_with("session-a.json"));
    }
}
