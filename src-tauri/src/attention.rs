//! Estado de atenção das missões: bloqueada (gate aberto, painel perguntando ou pedindo permissão),
//! pronta para integrar (todos os ajudantes entregaram e nenhum painel trabalha) e os motivos
//! (pergunta, permissão, falha). Calculado a partir dos painéis, dos gates e das entregas; emitido como `mission-state`.
use crate::error::AppResult;
use crate::runtime::AppState;
use crate::status::PaneStatus;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MissionAttention {
    pub workspace_id: String,
    pub mission_id: String,
    pub blocked: bool,
    pub ready: bool,
    /// pergunta | permissao | falha, na ordem da prioridade do rail
    pub reasons: Vec<&'static str>,
}

/// O que a missão tem agora. `statuses`: o estado de cada painel aberto da missão (`permissao` é um `pergunta` de permissão).
#[derive(Debug, Default)]
pub struct Snapshot<'a> {
    pub statuses: &'a [&'a str],
    /// gates abertos da missão
    pub open_gates: usize,
    /// missão marcada como bloqueada pelo usuário ou pelo agente
    pub marked_blocked: bool,
    pub helpers: usize,
    pub helpers_delivered: usize,
}

/// Pura: bloqueada, pronta e motivos a partir do snapshot.
pub fn compute(workspace_id: &str, mission_id: &str, snap: &Snapshot) -> MissionAttention {
    let has = |st: &str| snap.statuses.contains(&st);
    let mut reasons = vec![];
    if has("pergunta") {
        reasons.push("pergunta");
    }
    if has("permissao") {
        reasons.push("permissao");
    }
    if has("erro") {
        reasons.push("falha");
    }
    let asking = has("pergunta") || has("permissao");
    let working = ["trabalhando", "iniciando", "acordando"].iter().any(|s| has(s));
    MissionAttention {
        workspace_id: workspace_id.into(),
        mission_id: mission_id.into(),
        blocked: snap.open_gates > 0 || asking || snap.marked_blocked,
        // precisa de ao menos um ajudante: missão só de piloto não vira "pronta" sozinha
        ready: snap.helpers > 0 && snap.helpers_delivered == snap.helpers && !working,
        reasons,
    }
}

impl AppState {
    /// Estado de atenção de cada missão do workspace (missões arquivadas não entram).
    pub fn mission_attention(&self, workspace_id: &str) -> AppResult<Vec<MissionAttention>> {
        let missions = self.db.list_missions(workspace_id)?;
        let panes = self.db.open_panes(workspace_id)?;
        let gates = self.db.open_gates(Some(workspace_id))?;
        let mut out = vec![];
        for m in missions.iter().filter(|m| m.status != "arquivada") {
            let mine: Vec<_> = panes.iter().filter(|p| p.mission_id.as_deref() == Some(m.id.as_str())).collect();
            let statuses: Vec<&str> = mine.iter().map(|p| self.attention_status(&p.id, &p.status)).collect();
            // ajudantes e entregas vêm de todos os painéis da missão: um ajudante que entregou e já fechou conta
            let helpers: Vec<_> = self.db.mission_panes(&m.id)?.into_iter().filter(|p| p.parent_id.is_some()).collect();
            let mut delivered = 0;
            for h in &helpers {
                if self.db.has_handoff_from(&h.id)? {
                    delivered += 1;
                }
            }
            let snap = Snapshot {
                statuses: &statuses,
                open_gates: gates.iter().filter(|g| g.mission_id.as_deref() == Some(m.id.as_str())).count(),
                marked_blocked: m.status == "bloqueada",
                helpers: helpers.len(),
                helpers_delivered: delivered,
            };
            out.push(compute(workspace_id, &m.id, &snap));
        }
        Ok(out)
    }

    fn asks_permission(&self, id: &str) -> bool {
        self.rt().permission_ask.contains(id)
    }

    /// Estado do painel para a atenção: o da runtime quando há processo; senão o guardado.
    /// Pergunta de permissão (`permission_prompt` do hook) vira `permissao`.
    fn attention_status(&self, id: &str, stored: &str) -> &'static str {
        match self.status_of(id) {
            Some(PaneStatus::Asking) if self.asks_permission(id) => "permissao",
            Some(st) => st.as_str(),
            None => match stored {
                "pergunta" => "pergunta",
                "erro" => "erro",
                "trabalhando" => "trabalhando",
                "iniciando" => "iniciando",
                "acordando" => "acordando",
                _ => "ocioso",
            },
        }
    }

    /// Emite `mission-state` das missões cujo estado mudou desde a última vez (chamado no ticker de 1 s).
    /// Recalcula só quando algo mudou (painel, gate ou entrega) ou a cada 10 s, e só nos workspaces com painéis abertos.
    pub fn tick_mission_states(&self) {
        let dirty = self.attention_dirty.load(Ordering::Relaxed);
        let last = self.rt().attention_full_at;
        if !should_recompute(dirty, last, Instant::now()) {
            return;
        }
        // zera antes de calcular: uma mudança durante o cálculo fica marcada para o próximo tick
        self.attention_dirty.store(false, Ordering::Relaxed);
        self.rt().attention_full_at = Some(Instant::now());
        let Ok(mut workspaces) = self.db.workspaces_with_open_panes() else { return };
        // um workspace sem painel aberto ainda pode ter missão com estado publicado (último painel fechou, missão arquivada)
        for a in self.rt().mission_states.values() {
            if !workspaces.contains(&a.workspace_id) {
                workspaces.push(a.workspace_id.clone());
            }
        }
        for ws in workspaces {
            let Ok(states) = self.mission_attention(&ws) else { continue };
            let mut changed = vec![];
            {
                let mut rt = self.rt();
                // missão que saiu da lista (arquivada): estado zerado uma vez e some do cache
                let gone: Vec<String> = rt.mission_states.values().filter(|a| a.workspace_id == ws && !states.iter().any(|s| s.mission_id == a.mission_id)).map(|a| a.mission_id.clone()).collect();
                for id in gone {
                    rt.mission_states.remove(&id);
                    changed.push(compute(&ws, &id, &Snapshot::default()));
                }
                for a in states {
                    if rt.mission_states.get(&a.mission_id) != Some(&a) {
                        rt.mission_states.insert(a.mission_id.clone(), a.clone());
                        changed.push(a);
                    }
                }
            }
            for a in changed {
                self.emit("mission-state", serde_json::json!(a));
            }
        }
    }
}

/// Recálculo completo da atenção de tempos em tempos, mesmo sem mudança registrada.
const FULL_RECOMPUTE_EVERY: Duration = Duration::from_secs(10);

/// Pura: recalcula agora? Quando algo mudou ou faz 10 s do último recálculo completo.
pub fn should_recompute(dirty: bool, last_full: Option<Instant>, now: Instant) -> bool {
    dirty || last_full.is_none_or(|t| now.duration_since(t) >= FULL_RECOMPUTE_EVERY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap<'a>(statuses: &'a [&'a str]) -> Snapshot<'a> {
        Snapshot { statuses, ..Default::default() }
    }

    #[test]
    fn a_pane_asking_blocks_and_gives_its_reason() {
        let a = compute("w", "m", &snap(&["pergunta"]));
        assert!(a.blocked && !a.ready);
        assert_eq!(a.reasons, vec!["pergunta"]);
        let p = compute("w", "m", &snap(&["permissao", "ocioso"]));
        assert!(p.blocked);
        assert_eq!(p.reasons, vec!["permissao"]);
    }

    #[test]
    fn an_open_gate_blocks_without_any_pane_asking() {
        let a = compute("w", "m", &Snapshot { open_gates: 1, ..Default::default() });
        assert!(a.blocked);
        assert!(a.reasons.is_empty());
    }

    #[test]
    fn error_is_a_failure_reason_not_a_block() {
        let a = compute("w", "m", &snap(&["erro"]));
        assert_eq!(a.reasons, vec!["falha"]);
        assert!(!a.blocked);
    }

    #[test]
    fn ready_needs_every_helper_delivered_and_nobody_working() {
        let idle = ["ocioso", "ocioso"];
        let done = Snapshot { statuses: &idle, helpers: 2, helpers_delivered: 2, ..Default::default() };
        assert!(compute("w", "m", &done).ready);
        let partial = Snapshot { statuses: &idle, helpers: 2, helpers_delivered: 1, ..Default::default() };
        assert!(!compute("w", "m", &partial).ready);
        let busy = Snapshot { statuses: &["trabalhando"], helpers: 2, helpers_delivered: 2, ..Default::default() };
        assert!(!compute("w", "m", &busy).ready);
    }

    #[test]
    fn a_mission_without_helpers_is_never_ready_by_itself() {
        assert!(!compute("w", "m", &snap(&["ocioso"])).ready);
    }

    #[test]
    fn a_closed_helper_with_a_handoff_makes_its_mission_ready() {
        // o ajudante já fechou; a entrega dele conta (ajudantes e entregas vêm de todos os painéis da missão)
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).unwrap();
        let ws = s.db.upsert_workspace("D:\\proj").unwrap();
        s.db.insert_mission(&crate::db::Mission {
            id: "m".into(), workspace_id: ws.id.clone(), name: "m".into(), mode: "livre".into(), status: "ativa".into(), status_reason: None,
            worktree_path: None, branch: None, base_branch: None, pinned: false, created_at: 0, archived_at: None, squad_id: None, recipe_id: None,
            brief: None, style: None, profile: None, plan_json: None, verify_json: None, adopted: false,
        })
        .unwrap();
        let pane = |id: &str, parent: Option<&str>| crate::db::PaneRow {
            id: id.into(), workspace_id: ws.id.clone(), mission_id: Some("m".into()), kind: "agent".into(), name: id.into(),
            cwd: "C:/".into(), status: "ocioso".into(), parent_id: parent.map(Into::into), ..Default::default()
        };
        s.db.insert_pane(&pane("piloto", None)).unwrap();
        s.db.insert_pane(&pane("ajudante", Some("piloto"))).unwrap();
        s.db.close_pane("ajudante").unwrap();
        s.db.insert_handoff(&crate::db::Handoff {
            id: "h1".into(), mission_id: Some("m".into()), from_pane: "ajudante".into(), to_pane: None, summary: "ok".into(),
            data_json: None, files_json: None, created_at: 0, seen: false,
        })
        .unwrap();
        let states = s.mission_attention(&ws.id).unwrap();
        assert!(states[0].ready, "{states:?}");
    }

    #[test]
    fn the_attention_tick_recomputes_only_when_dirty_or_every_ten_seconds() {
        let now = Instant::now();
        assert!(should_recompute(false, None, now), "primeiro cálculo sempre roda");
        assert!(should_recompute(true, Some(now), now), "mudança registrada recalcula");
        assert!(!should_recompute(false, Some(now), now), "sem mudança e recente: pula");
        assert!(should_recompute(false, Some(now), now + Duration::from_secs(10)), "a cada 10 s, mesmo sem mudança");
    }

    type Events = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;

    /// Estado com o registro dos eventos emitidos e uma missão `m` com um painel `p` em `status`.
    fn mission_with_pane(status: &str) -> (std::sync::Arc<AppState>, Events, tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        let ev: Events = Default::default();
        let e2 = ev.clone();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), std::sync::Arc::new(move |n: &str, v: serde_json::Value| crate::util::lk(&e2).push((n.to_string(), v)))).unwrap();
        let ws = s.db.upsert_workspace("D:\\proj").unwrap();
        s.db.insert_mission(&crate::db::Mission {
            id: "m".into(), workspace_id: ws.id.clone(), name: "m".into(), mode: "livre".into(), status: "ativa".into(), status_reason: None,
            worktree_path: None, branch: None, base_branch: None, pinned: false, created_at: 0, archived_at: None, squad_id: None, recipe_id: None,
            brief: None, style: None, profile: None, plan_json: None, verify_json: None, adopted: false,
        })
        .unwrap();
        s.db.insert_pane(&crate::db::PaneRow {
            id: "p".into(), workspace_id: ws.id.clone(), mission_id: Some("m".into()), kind: "agent".into(), name: "p".into(),
            cwd: "C:/".into(), status: status.into(), ..Default::default()
        })
        .unwrap();
        (s, ev, d, ws.id)
    }

    /// Último `mission-state` emitido para `m`.
    fn last_state(ev: &Events) -> serde_json::Value {
        crate::util::lk(ev).iter().filter(|(n, v)| n == "mission-state" && v["missionId"] == "m").map(|(_, v)| v.clone()).last().unwrap()
    }

    #[test]
    fn a_workspace_whose_last_pane_closed_clears_the_mission() {
        let (s, ev, _d, _ws) = mission_with_pane("pergunta");
        s.tick_mission_states();
        assert_eq!(last_state(&ev)["blocked"], true);
        s.db.close_pane("p").unwrap();
        s.emit("pane-closed", serde_json::json!({}));
        s.tick_mission_states();
        let a = last_state(&ev);
        assert_eq!((a["blocked"].clone(), a["ready"].clone(), a["reasons"].clone()), (false.into(), false.into(), serde_json::json!([])), "{a}");
    }

    #[test]
    fn an_archived_mission_is_cleared_once_and_forgotten() {
        let (s, ev, _d, _ws) = mission_with_pane("pergunta");
        s.tick_mission_states();
        let mut m = s.db.get_mission("m").unwrap();
        m.status = "arquivada".into();
        s.db.update_mission(&m).unwrap();
        s.emit("mission-archived", serde_json::json!({"missionId": "m"}));
        s.tick_mission_states();
        assert_eq!(last_state(&ev)["blocked"], false);
        assert!(!s.rt().mission_states.contains_key("m"));
        s.emit("mission-archived", serde_json::json!({"missionId": "m"}));
        s.tick_mission_states();
        assert_eq!(crate::util::lk(&ev).iter().filter(|(n, _)| n == "mission-state").count(), 2, "bloqueada, depois zerada, e só");
    }

    #[test]
    fn marked_blocked_blocks() {
        assert!(compute("w", "m", &Snapshot { marked_blocked: true, ..Default::default() }).blocked);
    }
}
