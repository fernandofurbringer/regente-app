use super::{blocking, cap_perm, caller_row, check_doc, check_spawn_budget, check_texts, finish, not_archived, note, ok_json, require_top, tool_err, RegenteMcp};
use crate::error::{fail, AppError, AppResult};
use crate::providers::Permission;
use crate::squads::{self, SquadDef};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, JsonSchema)]
pub struct GateArgs {
    /// Título curto do gate (aparece para o usuário).
    pub title: String,
    /// O que foi feito e o que o usuário precisa decidir.
    pub summary: String,
    /// Opções de decisão (padrão: aprovar, ajustar).
    pub options: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SquadSpawnArgs {
    /// Id da squad (ver squad_list).
    pub squad_id: String,
    /// Brief do usuário para a missão.
    pub brief: String,
    /// Nome da missão (padrão: o nome da squad).
    pub name: Option<String>,
    /// Criar uma worktree git própria para a missão (padrão: não).
    pub worktree: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SquadGetArgs {
    /// Id da squad.
    pub id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SquadSaveArgs {
    /// A squad completa (ver squad_get para o formato).
    pub squad: SquadDef,
}

#[tool_router(router = squads_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Pausa a missão até o usuário decidir na UI (aprovar ou ajustar). Espera sem prazo e devolve {decision, note}.
    #[tool(description = "Pausa a missão até o usuário decidir na UI (aprovar ou ajustar). Espera a decisão e devolve {decision, note}")]
    async fn gate_request(&self, Parameters(a): Parameters<GateArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "gate_request", &parts);
        let s = self.state.clone();
        let me = match caller_row(&s, &parts).and_then(|me| not_archived(&s, &me).map(|()| me)) {
            Ok(r) => r,
            Err(e) => return tool_err(e),
        };
        let options = a.options.unwrap_or_else(|| vec!["aprovar".into(), "ajustar".into()]);
        let ticket = match squads::open_gate(&s, &me, a.title, a.summary, options) {
            Ok(t) => t,
            Err(e) => return tool_err(e),
        };
        // the ticket is held by this future: if the request is dropped, the waiter goes with it
        match ticket.decision().await {
            Some((decision, text)) => ok_json(&json!({"decision": decision, "note": text})),
            None => tool_err("gate cancelado"),
        }
    }

    /// Squads disponíveis: originais e do Regente.
    #[tool(description = "Squads disponíveis: originais e do Regente")]
    async fn squad_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "squad_list", &parts);
        finish(Ok(json!({"squads": squads::list(&self.state.data)})))
    }

    /// Uma squad completa (intake, time, fases e gates).
    #[tool(description = "Uma squad completa (intake, time, fases e gates)")]
    async fn squad_get(&self, Parameters(a): Parameters<SquadGetArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "squad_get", &parts);
        finish(squads::get(&self.state.data, &a.id).map(|s| json!(s)).ok_or_else(|| AppError::new("unknown_squad", format!("squad desconhecida: {}", a.id))))
    }

    /// Abre a missão de uma squad: só o maestro aparece, e ele chama o time por agent_invoke. Devolve a missão e o maestro.
    #[tool(description = "Abre a missão de uma squad: só o maestro aparece, e ele chama o time por agent_invoke. Devolve a missão e o maestro")]
    async fn squad_spawn(&self, Parameters(a): Parameters<SquadSpawnArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "squad_spawn", &parts);
        let st = self.state.clone();
        finish(
            blocking(move || {
                let me = caller_row(&st, &parts)?;
                require_top(&me)?;
                check_spawn_budget(&st, &me, 1)?;
                if a.brief.trim().is_empty() {
                    return fail("bad_brief", "o brief da missão não pode ser vazio");
                }
                let squad = squads::get(&st.data, &a.squad_id).ok_or_else(|| AppError::new("unknown_squad", format!("squad desconhecida: {}", a.squad_id)))?;
                let ws = st.db.get_workspace(&me.workspace_id)?;
                let answers = squads::autofill(&squad.intake, &a.brief);
                let name = a.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| squad.name.clone());
                let permission = cap_perm(&me, Some(Permission::Auto))?;
                let opts = crate::missions::MissionOpts { worktree: a.worktree.unwrap_or(false), parent: Some(me.id.clone()), ..Default::default() };
                let (m, row) = squads::start_squad_mission(&st, &ws, &squad, &name, &a.brief, answers, permission, &opts)?;
                Ok(json!({"missionId": m.id, "maestro": {"id": row.id, "name": row.name}}))
            })
            .await,
        )
    }

    /// Cria uma squad do Regente. Só cria: um id já existente (de qualquer fonte) é recusado; alterar é pela UI.
    #[tool(description = "Cria uma squad do Regente. Só cria: um id já existente é recusado (alterar é pela UI)")]
    async fn squad_save(&self, Parameters(a): Parameters<SquadSaveArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "squad_save", &parts);
        let data = self.state.data.clone();
        let sq = a.squad;
        let r = (|| -> AppResult<Value> {
            require_top(&caller_row(&self.state, &parts)?)?;
            if squads::get(&data, &sq.id).is_some() {
                return fail("exists", "já existe uma squad com este id (o MCP só cria)");
            }
            check_squad_caps(&sq)?;
            squads::save(&data, &sq)?;
            Ok(json!({"ok": true, "id": sq.id}))
        })();
        finish(r)
    }
}

/// Caps for a squad created over MCP: at most 12 phases and 16 roster entries, text fields up to 4000 chars, 64 KB.
fn check_squad_caps(s: &SquadDef) -> AppResult<()> {
    if s.phases.len() > 12 || s.roster.len() > 16 {
        return fail("too_big", "a squad tem no máximo 12 fases e 16 agentes no time");
    }
    let mut texts: Vec<&str> = vec![s.name.as_str(), s.description.as_str(), s.maestro_notes.as_str()];
    texts.extend(s.intake.iter().map(|q| q.q.as_str()));
    for ph in &s.phases {
        texts.push(&ph.name);
        texts.extend(ph.steps.iter().map(String::as_str));
        texts.extend(ph.gate.as_deref());
    }
    for r in &s.roster {
        texts.extend(r.role.as_deref());
    }
    check_texts(&texts)?;
    check_doc(s)
}
