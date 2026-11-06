use super::prices::Prices;
use super::{CostSnapshot, Usage};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Sum of a Claude session. The same `message.id` repeats across lines; the last one wins.
#[derive(Default)]
pub struct ClaudeAgg {
    per_msg: HashMap<String, (String, Usage)>,
    last_model: Option<String>,
    /// Entrada (input + cache) da primeira mensagem vista: o prefixo fixo da sessão.
    first_tokens: Option<u64>,
}

impl ClaudeAgg {
    pub fn apply_line(&mut self, line: &str) {
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if v["type"] != "assistant" {
            return;
        }
        let m = &v["message"];
        let (Some(id), Some(u)) = (m["id"].as_str(), m["usage"].as_object()) else { return };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        // `cache_creation_input_tokens` soma as duas TTLs; o detalhe de 1 hora vem em `cache_creation` (ausente = tudo 5 min)
        let c1h = u.get("cache_creation").and_then(|c| c["ephemeral_1h_input_tokens"].as_u64()).unwrap_or(0);
        let usage = Usage { input: n("input_tokens"), output: n("output_tokens"), cache_read: n("cache_read_input_tokens"), cache_write: n("cache_creation_input_tokens"), cache_write_1h: c1h };
        let model = m["model"].as_str().unwrap_or_default().to_string();
        if !model.is_empty() && !model.starts_with('<') {
            self.last_model = Some(model.clone());
        }
        // mensagens sintéticas do CLI (modelo `<...>`) não são turnos; o 1º turno precisa de entrada
        if model.starts_with('<') {
            return;
        }
        if !self.per_msg.contains_key(id) && self.first_tokens.is_none() && usage.input > 0 {
            self.first_tokens = Some(usage.input + usage.cache_write + usage.cache_read);
        }
        self.per_msg.insert(id.to_string(), (model, usage));
    }

    pub fn snapshot(&self, prices: &Prices) -> CostSnapshot {
        let mut total = Usage::default();
        let mut cost = 0.0;
        for (model, u) in self.per_msg.values() {
            total.add(u);
            cost += prices.cost(model, u);
        }
        CostSnapshot { usage: total, cost_usd: cost, model: self.last_model.clone(), session_id: None, turns: self.per_msg.len() as u32, first_turn_tokens: self.first_tokens }
    }
}

/// `<config>/projects/*/<session>.jsonl`
pub fn find_session_file(config_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let base = glob::Pattern::escape(&config_dir.join("projects").to_string_lossy());
    let pattern = PathBuf::from(base).join("*").join(format!("{}.jsonl", glob::Pattern::escape(session_id)));
    glob::glob(&pattern.to_string_lossy()).ok()?.flatten().next()
}

/// Subagent transcripts live next to the session file: `<project>/<session>/subagents/*.jsonl`.
// ponytail: `/clear` (new session id) is not followed yet; resolve the successor session if it matters.
pub fn subagent_files(session_file: &Path) -> Vec<PathBuf> {
    let dir = session_file.with_extension("").join("subagents");
    let pattern = PathBuf::from(glob::Pattern::escape(&dir.to_string_lossy())).join("*.jsonl");
    glob::glob(&pattern.to_string_lossy()).map(|g| g.flatten().collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PRICES_DEFAULT;

    #[test]
    fn dedupes_by_message_id_and_prices_per_model() {
        let prices: Prices = serde_json::from_str(PRICES_DEFAULT).unwrap();
        let mut agg = ClaudeAgg::default();
        for l in include_str!("../../tests/fixtures/claude.jsonl").lines() {
            agg.apply_line(l);
        }
        let s = agg.snapshot(&prices);
        assert_eq!(s.usage, Usage { input: 15, output: 150, cache_read: 5000, cache_write: 1000, cache_write_1h: 0 });
        // A: (10*4 + 100*20 + 2000*0.2 + 1000*8)/1e6 = 0.01044 ; B: (5*2 + 50*10 + 3000*0.2)/1e6 = 0.00111
        assert!((s.cost_usd - 0.01155).abs() < 1e-9, "{}", s.cost_usd);
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-5-5"));
    }

    #[test]
    fn same_message_id_three_times_counts_once_and_1h_cache_costs_2x_input() {
        let prices: Prices = serde_json::from_str(PRICES_DEFAULT).unwrap();
        let mut agg = ClaudeAgg::default();
        let line = |out: u64, c1h: u64| {
            format!(r#"{{"type":"assistant","message":{{"id":"msg_X","model":"claude-sonnet-5-5","usage":{{"input_tokens":10,"output_tokens":{out},"cache_creation_input_tokens":1000,"cache_creation":{{"ephemeral_5m_input_tokens":{},"ephemeral_1h_input_tokens":{c1h}}}}}}}}}"#, 1000 - c1h)
        };
        agg.apply_line(&line(1, 0));
        agg.apply_line(&line(2, 0));
        agg.apply_line(&line(7, 400));
        let s = agg.snapshot(&prices);
        assert_eq!(s.usage, Usage { input: 10, output: 7, cache_write: 1000, cache_write_1h: 400, ..Default::default() });
        // sonnet: input 3 (ou o que estiver na tabela), 5m = 1.25x, 1h = 2x input; checa só a diferença do 1h
        let p = prices.find("claude-sonnet-5-5").unwrap();
        let expected = (10.0 * p.input + 7.0 * p.output + 600.0 * p.cache_write + 400.0 * p.input * 2.0) / 1e6;
        assert!((s.cost_usd - expected).abs() < 1e-12, "{} vs {expected}", s.cost_usd);
    }

    #[test]
    fn synthetic_and_zero_input_messages_are_not_turns() {
        let mut agg = ClaudeAgg::default();
        let line = |id: &str, model: &str, input: u64| format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#);
        agg.apply_line(&line("s1", "<synthetic>", 0));
        agg.apply_line(&line("z1", "claude-haiku-5-5", 0));
        let s = agg.snapshot(&Prices::default());
        assert_eq!((s.turns, s.first_turn_tokens), (1, None), "só a z1 conta como turno, sem 1º turno com entrada zero");
        agg.apply_line(&line("r1", "claude-haiku-5-5", 40));
        let s = agg.snapshot(&Prices::default());
        assert_eq!((s.turns, s.first_turn_tokens), (2, Some(40)));
    }

    #[test]
    fn counts_unique_turns_and_the_first_turn_prefix() {
        let mut agg = ClaudeAgg::default();
        let line = |id: &str, input: u64, cw: u64| format!(r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-haiku-5-5","usage":{{"input_tokens":{input},"output_tokens":1,"cache_creation_input_tokens":{cw},"cache_read_input_tokens":7}}}}}}"#);
        agg.apply_line(&line("m1", 10, 100));
        agg.apply_line(&line("m1", 10, 100)); // mesmo message.id repetido: não conta de novo
        agg.apply_line(&line("m2", 500, 0));
        let prices: Prices = serde_json::from_str(crate::settings::PRICES_DEFAULT).unwrap();
        let s = agg.snapshot(&prices);
        assert_eq!(s.turns, 2);
        // primeiro turno: input 10 + cache write 100 + cache read 7
        assert_eq!(s.first_turn_tokens, Some(117));
    }

    #[test]
    fn finds_session_file_in_any_project_dir() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("projects").join("D--x");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("abc.jsonl"), "").unwrap();
        assert_eq!(find_session_file(d.path(), "abc"), Some(proj.join("abc.jsonl")));
        assert_eq!(find_session_file(d.path(), "zzz"), None);
    }
}
