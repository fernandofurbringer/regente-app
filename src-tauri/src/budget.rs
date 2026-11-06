//! Orçamento de custo (Configurações → Custos e orçamento): aviso ao chegar a 80% e a 100% do limite diário ou por missão,
//! uma vez por nível e por período. Com "pausar" ligado, a missão que gastou o limite não abre painel novo.
//! Funções puras; o disparo e o envio ficam em `runtime.rs` (`check_budgets`, `spawn_pane`).

use std::collections::HashSet;

/// Níveis de aviso, em porcentagem do limite.
pub const LEVELS: [u32; 2] = [80, 100];

/// Limite válido: número finito e maior que zero. Qualquer outro valor desliga o orçamento.
pub fn valid_limit(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

/// Níveis que o gasto já atingiu e que ainda não foram avisados. `scope` identifica o período (ex.: "dia:2026-10-15"
/// ou "missao:<id>"); cada nível marcado em `sent` não avisa de novo.
pub fn new_alerts(spent: f64, limit: f64, scope: &str, sent: &mut HashSet<String>) -> Vec<u32> {
    LEVELS
        .iter()
        .copied()
        .filter(|&lvl| spent >= limit * f64::from(lvl) / 100.0)
        .filter(|&lvl| sent.insert(format!("{scope}:{lvl}")))
        .collect()
}

/// Valor em US$ no padrão pt-BR. Gasto abaixo de US$ 0,01 não vira "US$ 0,00": mostra "<US$ 0,01".
pub fn usd(v: f64) -> String {
    if v > 0.0 && v < 0.01 {
        return "<US$ 0,01".into();
    }
    format!("US$ {v:.2}").replace('.', ",")
}

/// Limite de orçamento: com 4 casas quando é menor que US$ 0,01 (com 2 casas o aviso dizia "US$ 0,00").
pub fn usd_limit(v: f64) -> String {
    if v > 0.0 && v < 0.01 {
        return format!("US$ {v:.4}").replace('.', ",");
    }
    usd(v)
}

/// A missão já gastou o limite (com "pausar" ligado, novos painéis dela são recusados).
pub fn exhausted(spent: f64, limit: Option<f64>) -> bool {
    limit.is_some_and(|l| spent >= l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_positive_finite_limits_count() {
        assert_eq!(valid_limit(Some(5.0)), Some(5.0));
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(valid_limit(Some(bad)), None, "{bad}");
        }
        assert_eq!(valid_limit(None), None);
    }

    #[test]
    fn each_level_alerts_once_per_period() {
        let mut sent = HashSet::new();
        assert!(new_alerts(79.9, 100.0, "dia:1", &mut sent).is_empty(), "abaixo de 80%");
        assert_eq!(new_alerts(80.0, 100.0, "dia:1", &mut sent), vec![80]);
        assert!(new_alerts(90.0, 100.0, "dia:1", &mut sent).is_empty(), "80% já avisado");
        assert_eq!(new_alerts(100.0, 100.0, "dia:1", &mut sent), vec![100]);
        assert!(new_alerts(150.0, 100.0, "dia:1", &mut sent).is_empty(), "100% já avisado");
        // outro período (outro dia ou outra missão) avisa de novo
        assert_eq!(new_alerts(85.0, 100.0, "dia:2", &mut sent), vec![80]);
        assert_eq!(new_alerts(85.0, 100.0, "missao:a", &mut sent), vec![80]);
    }

    #[test]
    fn jumping_past_both_levels_warns_for_both_once() {
        let mut sent = HashSet::new();
        assert_eq!(new_alerts(120.0, 100.0, "missao:a", &mut sent), vec![80, 100]);
        assert!(new_alerts(130.0, 100.0, "missao:a", &mut sent).is_empty());
    }

    #[test]
    fn exhausted_at_the_limit_only() {
        assert!(!exhausted(99.9, Some(100.0)));
        assert!(exhausted(100.0, Some(100.0)));
        assert!(!exhausted(500.0, None), "sem limite nunca pausa");
    }

    #[test]
    fn money_in_the_budget_texts_is_never_a_misleading_zero() {
        assert_eq!(usd(0.004), "<US$ 0,01");
        assert_eq!(usd(0.0), "US$ 0,00");
        assert_eq!(usd(12.345), "US$ 12,35");
        assert_eq!(usd_limit(0.005), "US$ 0,0050");
        assert_eq!(usd_limit(2.0), "US$ 2,00");
    }
}
