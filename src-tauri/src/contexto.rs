//! Medidor de contexto por painel (RM-16a). A statusline manda o uso de contexto ao `/hook`; o app guarda o último
//! valor, emite `pane-context` (no máximo a cada 2 s por painel), guarda o pico em `ctx_peak` e conta as
//! compactações. Antes/depois de cada compactação fica em memória: a calibração do H2 (razão do resumo) sai daí.
use crate::runtime::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const EMITE_CADA: Duration = Duration::from_secs(2);
const CALIBRAGENS_MAX: usize = 100;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Medida {
    pub pct: Option<f64>,
    pub tokens: Option<i64>,
    pub janela: Option<i64>,
    /// Id do modelo que a statusline informou (o painel no "padrão do CLI" não tem modelo próprio).
    pub modelo: Option<String>,
    /// Quando chegou (a política de compactação só age com medida recente).
    pub quando: Option<Instant>,
}

#[derive(Default)]
pub struct CtxRt {
    /// Última medida de cada painel.
    pub atual: HashMap<String, Medida>,
    ultimo_emit: HashMap<String, Instant>,
    /// Medida antes da compactação em curso (PreCompact).
    antes_compact: HashMap<String, Medida>,
    /// Compactação feita, esperando a próxima statusline para ler o "depois".
    aguarda_depois: HashMap<String, Medida>,
    /// (painel, antes %, depois %) das compactações desta execução.
    pub calibragens: Vec<(String, f64, f64)>,
    /// Maior pico visto por painel nesta sessão: só sobe, então o banco é consultado só quando o pico sobe.
    pico: HashMap<String, i64>,
    /// Estado da política de compactação (H2): calibração, propostas, recomeços e efeitos.
    pub compact: crate::compactacao::Estado,
}

impl CtxRt {
    pub fn remove(&mut self, pane: &str) {
        self.atual.remove(pane);
        self.ultimo_emit.remove(pane);
        self.antes_compact.remove(pane);
        self.aguarda_depois.remove(pane);
        self.pico.remove(pane);
        self.compact.esquecer(pane);
    }
}

impl AppState {
    /// Trata os eventos do medidor. True = era evento de contexto (o `on_hook` para aqui).
    pub fn ctx_hook(&self, pane: &str, event: &str, payload: &Value) -> bool {
        match event {
            "statusline" => self.ctx_statusline(pane, payload),
            "PreCompact" => {
                let mut rt = self.rt();
                if let Some(m) = rt.ctx.atual.get(pane).cloned() {
                    rt.ctx.antes_compact.insert(pane.into(), m);
                }
            }
            "PostCompact" => {
                {
                    let mut rt = self.rt();
                    let antes = rt.ctx.antes_compact.remove(pane).or_else(|| rt.ctx.atual.get(pane).cloned());
                    if let Some(a) = antes {
                        rt.ctx.aguarda_depois.insert(pane.into(), a);
                    }
                }
                let _ = self.db.metric_bump(pane, "compactions");
            }
            _ => return false,
        }
        true
    }

    fn ctx_statusline(&self, pane: &str, payload: &Value) {
        let medida = Medida { pct: payload["ctxPct"].as_f64(), tokens: payload["ctxTokens"].as_i64(), janela: payload["windowSize"].as_i64(), modelo: payload["modelId"].as_str().map(str::to_string), quando: Some(Instant::now()) };
        let (emitir, antes) = {
            let mut rt = self.rt();
            rt.ctx.atual.insert(pane.into(), medida.clone());
            let antes = if medida.pct.is_some() { rt.ctx.aguarda_depois.remove(pane) } else { None };
            if let (Some(a), Some(depois)) = (&antes, medida.pct) {
                if let Some(antes_pct) = a.pct {
                    rt.ctx.calibragens.push((pane.into(), antes_pct, depois));
                    let excesso = rt.ctx.calibragens.len().saturating_sub(CALIBRAGENS_MAX);
                    rt.ctx.calibragens.drain(..excesso);
                }
            }
            let agora = Instant::now();
            let emitir = rt.ctx.ultimo_emit.get(pane).is_none_or(|t| agora.duration_since(*t) >= EMITE_CADA);
            if emitir {
                rt.ctx.ultimo_emit.insert(pane.into(), agora);
            }
            (emitir, antes)
        };
        if let Some(a) = antes {
            self.calibrar_razao(pane, &a, &medida);
        }
        if let Some(p) = medida.pct {
            let pico = p.round() as i64;
            let subiu = {
                let mut rt = self.rt();
                let c = rt.ctx.pico.entry(pane.into()).or_insert(0);
                if pico > *c {
                    *c = pico;
                    true
                } else {
                    false
                }
            };
            if subiu {
                let atual = self.db.pane_metrics(pane).ok().flatten().map_or(0, |m| m.ctx_peak);
                if pico > atual {
                    let _ = self.db.metric_set(pane, "ctx_peak", pico);
                }
            }
        }
        if emitir {
            let ws = self.db.get_pane(pane).map(|r| r.workspace_id).unwrap_or_default();
            self.emit("pane-context", json!({"workspaceId": ws, "paneId": pane, "ctxPct": medida.pct, "ctxTokens": medida.tokens, "windowSize": medida.janela}));
        }
    }

    /// Calibra a razão resumo/contexto com uma compactação medida: resumo = contexto depois − P (primeiro turno).
    /// Sem o prefixo medido, ou sem tokens dos dois lados, não calibra.
    fn calibrar_razao(&self, pane: &str, antes: &Medida, depois: &Medida) {
        let antes_tokens = antes.tokens.map(|t| t as f64).or_else(|| antes.pct.zip(antes.janela).map(|(p, j)| p / 100.0 * j as f64));
        let (Some(at), Some(dt)) = (antes_tokens, depois.tokens) else { return };
        let prefixo = self.db.pane_metrics(pane).ok().flatten().and_then(|m| m.first_turn_tokens).unwrap_or(0) as f64;
        if prefixo <= 0.0 {
            return;
        }
        let mut rt = self.rt();
        let atual = rt.ctx.compact.razao;
        let nova = crate::compactacao::calibrar(atual, at, dt as f64, prefixo);
        rt.ctx.compact.razao = nova;
        drop(rt);
        // o que foi calibrado sobrevive ao reinício (T1 aprende a razão); em `desligado` não grava
        if let Some(r) = nova {
            crate::autoajuste::guardar_r_resumo(self, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_forgets_the_pane() {
        let mut c = CtxRt::default();
        c.atual.insert("p".into(), Medida { pct: Some(50.0), ..Default::default() });
        c.aguarda_depois.insert("p".into(), Medida { pct: Some(70.0), ..Default::default() });
        c.compact.propostas.insert("p".into(), "compactar".into());
        c.remove("p");
        assert!(c.atual.is_empty() && c.aguarda_depois.is_empty() && c.compact.propostas.is_empty());
    }
}
