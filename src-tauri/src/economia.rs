//! Painel de economia e guarda de qualidade (RM-23, H1-14). Só agrega o que já está no banco.
//! Todo número de "economia" é estimativa: só aparece com amostra mínima em cada grupo.

use crate::db::{Db, EcoPane};
use crate::error::AppResult;
use crate::runtime::AppState;
use crate::util::now_ms;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Amostra mínima por grupo antes de mostrar qualquer estimativa.
pub const MIN_AMOSTRAS: u32 = 5;
const SEMANA_MS: i64 = 7 * 86_400_000;
/// Queda de aceitação maior que 10 pontos, ou escalonamento acima de 15%, depois de um ajuste.
const QUEDA_MAX: f64 = 0.10;
const ESCALONAMENTO_MAX: f64 = 0.15;
const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Grupo {
    pub profile: Option<String>,
    pub task_type: Option<String>,
    pub model: Option<String>,
    pub mission_id: Option<String>,
    pub panes: u32,
    pub cost_usd: f64,
    pub tokens: i64,
    /// cache_r / (cache_r + cache_w + input)
    pub cache_hit_ratio: Option<f64>,
    /// (input + cache_r + cache_w) / turnos
    pub ctx_medio_turno: Option<f64>,
    pub primeiro_turno_medio: Option<f64>,
    pub compactacoes: i64,
    pub recomecos: i64,
    pub escalonamentos: i64,
}

/// Custo médio por tarefa: painéis `precisao` contra `economico`/`equilibrado` do mesmo tipo. Estimativa.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Economia {
    pub task_type: String,
    pub amostras_precisao: u32,
    pub amostras_eco: u32,
    pub custo_precisao: f64,
    pub custo_eco: f64,
    /// custo_precisao - custo_eco (positivo = o perfil econômico poupa)
    pub economia_estimada: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EconomyReport {
    pub grupos: Vec<Grupo>,
    pub acerto_cache: Option<f64>,
    pub contexto_medio_turno: Option<f64>,
    pub economia: Vec<Economia>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualityGuard {
    pub ok: bool,
    pub motivo: String,
}

#[derive(Default)]
struct Acc {
    panes: u32,
    cost: f64,
    tok_in: i64,
    tok_out: i64,
    cache_r: i64,
    cache_w: i64,
    turns: i64,
    primeiro: (f64, u32),
    comp: i64,
    fresh: i64,
    esc: i64,
}

impl Acc {
    fn soma(&mut self, p: &EcoPane) {
        self.panes += 1;
        self.cost += p.cost_usd;
        self.tok_in += p.tok_in;
        self.tok_out += p.tok_out;
        self.cache_r += p.cache_r;
        self.cache_w += p.cache_w;
        self.turns += p.turns;
        if let Some(f) = p.first_turn_tokens {
            self.primeiro = (self.primeiro.0 + f as f64, self.primeiro.1 + 1);
        }
        self.comp += p.compactions;
        self.fresh += p.fresh_starts;
        self.esc += p.escalations;
    }
}

/// `num / den` só quando o denominador existe (sem divisão por zero).
fn razao(num: f64, den: f64) -> Option<f64> {
    (den > 0.0).then(|| num / den)
}

fn acerto(cache_r: i64, cache_w: i64, input: i64) -> Option<f64> {
    razao(cache_r as f64, (cache_r + cache_w + input) as f64)
}

fn ctx_por_turno(input: i64, cache_r: i64, cache_w: i64, turns: i64) -> Option<f64> {
    razao((input + cache_r + cache_w) as f64, turns as f64)
}

/// Agrega os painéis: grupos (perfil, tipo, modelo, missão), acerto de cache, contexto médio e economia estimada.
pub fn agrega(panes: &[EcoPane]) -> EconomyReport {
    let mut grupos: BTreeMap<(Option<String>, Option<String>, Option<String>, Option<String>), Acc> = BTreeMap::new();
    let mut todos = Acc::default();
    for p in panes {
        let chave = (p.profile.clone(), p.task_type.clone(), p.model.clone(), p.mission_id.clone());
        grupos.entry(chave).or_default().soma(p);
        todos.soma(p);
    }
    let grupos = grupos
        .into_iter()
        .map(|((profile, task_type, model, mission_id), a)| Grupo {
            profile,
            task_type,
            model,
            mission_id,
            panes: a.panes,
            cost_usd: a.cost,
            tokens: a.tok_in + a.tok_out,
            cache_hit_ratio: acerto(a.cache_r, a.cache_w, a.tok_in),
            ctx_medio_turno: ctx_por_turno(a.tok_in, a.cache_r, a.cache_w, a.turns),
            primeiro_turno_medio: razao(a.primeiro.0, a.primeiro.1 as f64),
            compactacoes: a.comp,
            recomecos: a.fresh,
            escalonamentos: a.esc,
        })
        .collect();
    EconomyReport {
        grupos,
        acerto_cache: acerto(todos.cache_r, todos.cache_w, todos.tok_in),
        contexto_medio_turno: ctx_por_turno(todos.tok_in, todos.cache_r, todos.cache_w, todos.turns),
        economia: economia(panes),
    }
}

/// Por tipo de tarefa: média de custo de `precisao` contra a de `economico`/`equilibrado`, com ≥ 5 amostras cada.
fn economia(panes: &[EcoPane]) -> Vec<Economia> {
    let mut por_tipo: BTreeMap<&str, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for p in panes {
        let Some(t) = p.task_type.as_deref() else { continue };
        match p.profile.as_deref() {
            Some("precisao") => por_tipo.entry(t).or_default().0.push(p.cost_usd),
            Some("economico" | "equilibrado") => por_tipo.entry(t).or_default().1.push(p.cost_usd),
            _ => {}
        }
    }
    por_tipo
        .into_iter()
        .filter_map(|(t, (precisao, eco))| {
            let (np, ne) = (precisao.len() as u32, eco.len() as u32);
            if np < MIN_AMOSTRAS || ne < MIN_AMOSTRAS {
                return None;
            }
            let (cp, ce) = (media(&precisao), media(&eco));
            Some(Economia { task_type: t.to_string(), amostras_precisao: np, amostras_eco: ne, custo_precisao: cp, custo_eco: ce, economia_estimada: cp - ce })
        })
        .collect()
}

fn media(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// Aceitação das entregas: fração com `ok` (padrão true). None sem amostra mínima.
// ponytail: X1b (taxa de verificação) ainda não existe; quando existir, entra aqui junto da aceitação.
pub fn aceitacao(entregas: &[(i64, Option<String>)]) -> Option<f64> {
    if (entregas.len() as u32) < MIN_AMOSTRAS {
        return None;
    }
    let ok = entregas.iter().filter(|(_, d)| d.as_deref().and_then(|j| serde_json::from_str::<Value>(j).ok()).and_then(|v| v["ok"].as_bool()).unwrap_or(true)).count();
    Some(ok as f64 / entregas.len() as f64)
}

/// Taxa de escalonamento: painéis com ao menos um escalonamento entre os que tiveram turnos. None sem amostra.
pub fn escalonamento(paineis: &[(i64, i64)]) -> Option<f64> {
    let com_turno: Vec<&(i64, i64)> = paineis.iter().filter(|(t, _)| *t > 0).collect();
    if (com_turno.len() as u32) < MIN_AMOSTRAS {
        return None;
    }
    let escalados = com_turno.iter().filter(|(_, e)| *e > 0).count();
    Some(escalados as f64 / com_turno.len() as f64)
}

/// Nome pt-BR do `kind` de um ajuste, para as mensagens da guarda.
pub fn rotulo_ajuste(kind: &str) -> &str {
    match kind {
        "compactacao" => "compactação",
        "recomeco" => "recomeço",
        "escalonamento" => "escalonamento",
        "perfil_patch" => "troca de perfil",
        "indice_codigo" => "índice de código",
        "testes_relacionados" => "testes relacionados",
        "limites_saida" => "limites de saída",
        outro => outro,
    }
}

/// Veredito da guarda. `antes`/`depois`: aceitação (queda máxima de 10 pontos). `esc_antes`/`esc`: escalonamento
/// nos 7 dias antes e depois (alta máxima de 15 pontos sobre o período anterior; sem período anterior, sobre zero).
/// Limites são estritos.
pub fn avaliar(antes: Option<f64>, depois: Option<f64>, esc_antes: Option<f64>, esc: Option<f64>, rotulo: &str) -> QualityGuard {
    if let (Some(a), Some(d)) = (antes, depois) {
        if a - d > QUEDA_MAX + EPS {
            return QualityGuard { ok: false, motivo: format!("Qualidade caiu depois de ativar {rotulo}: aceitação de {:.0}% para {:.0}%.", a * 100.0, d * 100.0) };
        }
    }
    if let Some(e) = esc {
        let base = esc_antes.unwrap_or(0.0);
        if e - base > ESCALONAMENTO_MAX + EPS {
            return QualityGuard { ok: false, motivo: format!("Qualidade caiu depois de ativar {rotulo}: {:.0}% dos painéis escalaram de ajudante (eram {:.0}%).", e * 100.0, base * 100.0) };
        }
    }
    if depois.is_none() && esc.is_none() {
        return QualityGuard { ok: true, motivo: "Ainda sem dados suficientes depois do último ajuste.".into() };
    }
    QualityGuard { ok: true, motivo: "Qualidade estável depois do último ajuste.".into() }
}

pub fn report(db: &Db, since: i64, ws: Option<&str>) -> AppResult<EconomyReport> {
    Ok(agrega(&db.economy_panes(since, ws)?))
}

/// Guarda de qualidade do workspace: compara os 7 dias depois do último ajuste do feed com os 7 antes.
pub fn quality(s: &AppState, ws: &str) -> AppResult<QualityGuard> {
    let Some(ajuste) = s.db.list_ajustes(Some(ws), 1)?.into_iter().next() else {
        return Ok(QualityGuard { ok: true, motivo: "Nenhum ajuste registrado ainda.".into() });
    };
    let t0 = ajuste.created_at;
    let fim_depois = (t0 + SEMANA_MS).min(now_ms());
    let antes = aceitacao(&s.db.quality_handoffs(ws, t0 - SEMANA_MS, t0)?);
    let depois = aceitacao(&s.db.quality_handoffs(ws, t0, fim_depois)?);
    let esc_antes = escalonamento(&s.db.quality_panes(ws, t0 - SEMANA_MS, t0)?);
    let esc = escalonamento(&s.db.quality_panes(ws, t0, fim_depois)?);
    Ok(avaliar(antes, depois, esc_antes, esc, rotulo_ajuste(&ajuste.kind)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(profile: Option<&str>, task: Option<&str>, custo: f64) -> EcoPane {
        EcoPane { profile: profile.map(Into::into), task_type: task.map(Into::into), model: Some("opus".into()), mission_id: None, cost_usd: custo, tok_in: 100, tok_out: 10, cache_r: 800, cache_w: 100, turns: 4, compactions: 0, fresh_starts: 0, escalations: 0, first_turn_tokens: Some(1000) }
    }

    #[test]
    fn cache_ratio_and_context_per_turn() {
        let r = agrega(&[pane(Some("equilibrado"), Some("impl"), 1.0)]);
        let g = &r.grupos[0];
        // 800 / (800 + 100 + 100) = 0.8
        assert!((g.cache_hit_ratio.unwrap() - 0.8).abs() < 1e-9);
        // (100 + 800 + 100) / 4 = 250
        assert_eq!(g.ctx_medio_turno, Some(250.0));
        assert_eq!(g.primeiro_turno_medio, Some(1000.0));
    }

    #[test]
    fn zero_denominators_give_none_not_nan() {
        let vazio = EcoPane { turns: 0, tok_in: 0, cache_r: 0, cache_w: 0, first_turn_tokens: None, ..pane(None, None, 0.0) };
        let r = agrega(&[vazio]);
        assert_eq!(r.grupos[0].cache_hit_ratio, None);
        assert_eq!(r.grupos[0].ctx_medio_turno, None);
        assert_eq!(r.acerto_cache, None);
        assert_eq!(agrega(&[]).contexto_medio_turno, None);
    }

    #[test]
    fn economy_needs_five_samples_per_side() {
        let mut ps: Vec<EcoPane> = (0..5).map(|_| pane(Some("precisao"), Some("impl"), 2.0)).collect();
        ps.extend((0..4).map(|_| pane(Some("economico"), Some("impl"), 1.0)));
        assert!(agrega(&ps).economia.is_empty(), "4 amostras econômicas não bastam");
        ps.push(pane(Some("equilibrado"), Some("impl"), 1.5));
        let e = &agrega(&ps).economia[0];
        assert_eq!((e.amostras_precisao, e.amostras_eco), (5, 5));
        assert!((e.custo_precisao - 2.0).abs() < 1e-9);
        // economico (4 x 1.0) + equilibrado (1.5): média 1.1
        assert!((e.custo_eco - 1.1).abs() < 1e-9);
        assert!((e.economia_estimada - 0.9).abs() < 1e-9);
    }

    #[test]
    fn panes_without_task_type_are_not_compared() {
        let ps: Vec<EcoPane> = (0..10).map(|_| pane(Some("precisao"), None, 2.0)).collect();
        assert!(agrega(&ps).economia.is_empty());
    }

    #[test]
    fn acceptance_counts_ok_flag_and_needs_samples() {
        let poucas: Vec<(i64, Option<String>)> = (0..4).map(|i| (i, None)).collect();
        assert_eq!(aceitacao(&poucas), None);
        let mut ent: Vec<(i64, Option<String>)> = (0..4).map(|i| (i, None)).collect();
        ent.push((9, Some(r#"{"ok":false}"#.into())));
        assert_eq!(aceitacao(&ent), Some(0.8));
    }

    #[test]
    fn escalation_rate_ignores_panes_without_turns() {
        let ps = vec![(3, 1), (2, 0), (1, 0), (4, 0), (5, 0), (0, 3)];
        // 5 com turno, 1 escalou
        assert_eq!(escalonamento(&ps), Some(0.2));
    }

    #[test]
    fn guard_thresholds_are_strict() {
        // queda de exatamente 10 pontos passa; 11 não
        assert!(avaliar(Some(0.9), Some(0.8), None, None, "perfil_patch").ok);
        let caiu = avaliar(Some(0.9), Some(0.78), None, None, "troca de perfil");
        assert!(!caiu.ok);
        assert!(caiu.motivo.contains("90% para 78%"), "{}", caiu.motivo);
        // sem período anterior, escalonamento de exatamente 15% passa; acima não
        assert!(avaliar(None, None, None, Some(0.15), "x").ok);
        assert!(!avaliar(None, None, None, Some(0.16), "x").ok);
    }

    #[test]
    fn escalation_is_compared_with_the_seven_days_before() {
        // 30% antes e 40% depois: alta de 10 pontos, passa
        assert!(avaliar(None, None, Some(0.30), Some(0.40), "x").ok);
        // 10% antes e 30% depois: alta de 20 pontos, cai (mesmo acima de 15% em termos absolutos)
        let subiu = avaliar(None, None, Some(0.10), Some(0.30), "x");
        assert!(!subiu.ok);
        assert!(subiu.motivo.contains("eram 10%"), "{}", subiu.motivo);
    }

    #[test]
    fn kinds_have_portuguese_labels() {
        assert_eq!(rotulo_ajuste("compactacao"), "compactação");
        assert_eq!(rotulo_ajuste("recomeco"), "recomeço");
        assert_eq!(rotulo_ajuste("perfil_patch"), "troca de perfil");
        assert_eq!(rotulo_ajuste("algo_novo"), "algo_novo", "desconhecido passa como veio");
    }

    #[test]
    fn guard_without_data_is_ok() {
        let g = avaliar(None, None, None, None, "x");
        assert!(g.ok);
        assert!(g.motivo.contains("Ainda sem dados"));
    }
}
