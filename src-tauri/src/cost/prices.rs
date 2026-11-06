use super::Usage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PriceEntry {
    #[serde(rename = "match")]
    pub pattern: String,
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

/// US$ per 1M tokens. The first entry whose `match` is contained in the model name wins.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Prices {
    pub models: Vec<PriceEntry>,
}

impl Prices {
    pub fn find(&self, model: &str) -> Option<&PriceEntry> {
        let m = model.to_lowercase();
        self.models.iter().find(|e| !e.pattern.is_empty() && m.contains(&e.pattern.to_lowercase()))
    }

    pub fn cost(&self, model: &str, u: &Usage) -> f64 {
        self.find(model).map_or(0.0, |p| {
            // cache de 5 min = o resto da escrita; o de 1 hora custa 2x a entrada
            let w5 = u.cache_write.saturating_sub(u.cache_write_1h);
            (u.input as f64 * p.input + u.output as f64 * p.output + u.cache_read as f64 * p.cache_read + w5 as f64 * p.cache_write + u.cache_write_1h as f64 * p.input * 2.0) / 1_000_000.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PRICES_DEFAULT;
    #[test]
    fn specific_match_wins() {
        let p: Prices = serde_json::from_str(PRICES_DEFAULT).unwrap();
        assert_eq!(p.find("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(p.find("claude-opus-4-8").unwrap().input, 5.0);
        assert_eq!(p.find("claude-haiku-5-5").unwrap().input, 0.1);
        assert!(p.find("modelo-desconhecido").is_none());
    }
    #[test]
    fn cost_formula() {
        let p = Prices { models: vec![PriceEntry { pattern: "x".into(), input: 1.0, output: 2.0, cache_read: 0.5, cache_write: 4.0 }] };
        let u = Usage { input: 1_000_000, output: 500_000, cache_read: 2_000_000, cache_write: 250_000, ..Default::default() };
        assert!((p.cost("x-1", &u) - (1.0 + 1.0 + 1.0 + 1.0)).abs() < 1e-9);
        assert_eq!(p.cost("y", &u), 0.0);
    }
}
