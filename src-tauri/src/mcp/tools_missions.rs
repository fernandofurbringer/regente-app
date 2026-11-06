use super::{blocking, caller_row, finish, mission_in_ws, note, own_mission, require_top, RegenteMcp};
use crate::error::{fail, AppResult};
use crate::missions;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateArgs {
    /// Nome da missão.
    pub name: String,
    /// Modo: "livre" (padrão). Só o modo livre existe nesta fase.
    pub mode: Option<String>,
    /// Criar uma worktree git própria para a missão (padrão: não).
    pub worktree: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetArgs {
    /// Id da missão (padrão: a sua).
    pub id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct StatusArgs {
    /// Novo estado: ativa, pronta, bloqueada ou concluida.
    pub status: String,
    /// Motivo (usado com "bloqueada").
    pub reason: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct BriefArgs {
    /// Texto novo do brief da missão. Sem ele, só lê.
    pub set: Option<String>,
}

#[tool_router(router = missions_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Cria uma missão no workspace (não abre painéis).
    #[tool(description = "Cria uma missão no workspace (não abre painéis)")]
    async fn mission_create(&self, Parameters(a): Parameters<CreateArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "mission_create", &parts);
        let st = self.state.clone();
        finish(
            blocking(move || {
                let me = caller_row(&st, &parts)?;
                let mode = a.mode.as_deref().unwrap_or("livre");
                if mode != "livre" {
                    return fail("mode_unavailable", "só o modo livre existe nesta fase");
                }
                let ws = st.db.get_workspace(&me.workspace_id)?;
                // a worktree needs git: this runs off the async runtime
                let (m, _cwd) = missions::new_livre(&ws, &a.name, a.worktree.unwrap_or(false))?;
                st.db.insert_mission(&m)?;
                crate::worktree_cache::acelerar(&st, &ws, &m);
                Ok(json!(m))
            })
            .await,
        )
    }

    /// A missão (a sua ou a informada), com tarefas, painéis abertos e totais de uso.
    #[tool(description = "A missão (a sua ou a informada), com tarefas, painéis abertos e totais de uso")]
    async fn mission_get(&self, Parameters(a): Parameters<GetArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "mission_get", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let m = match a.id {
                Some(id) => mission_in_ws(s, &me.workspace_id, &id)?,
                None => s.db.get_mission(&own_mission(&me)?)?,
            };
            let open: Vec<Value> = s
                .list_panes(&me.workspace_id)?
                .into_iter()
                .filter(|p| p.mission_id.as_deref() == Some(m.id.as_str()))
                .map(|p| json!({"id": p.id, "name": p.name, "status": p.status, "role": p.role}))
                .collect();
            let totals = s
                .db
                .mission_totals(&me.workspace_id)?
                .into_iter()
                .find(|(id, _, _, _)| *id == m.id)
                .map(|(_, tokens, cache, cost)| json!({"tokens": tokens, "cacheTokens": cache, "costUsd": cost}))
                .unwrap_or_else(|| json!({"tokens": 0, "cacheTokens": 0, "costUsd": 0.0}));
            Ok(json!({"mission": m, "tasks": s.db.tasks(&m.id)?, "openPanes": open, "totals": totals}))
        })())
    }

    /// Missões do workspace.
    #[tool(description = "Missões do workspace")]
    async fn mission_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "mission_list", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            Ok(json!({"missions": s.db.list_missions(&me.workspace_id)?}))
        })())
    }

    /// Muda o estado da sua missão (ativa, pronta, bloqueada ou concluida).
    #[tool(description = "Muda o estado da sua missão (ativa, pronta, bloqueada ou concluida)")]
    async fn mission_set_status(&self, Parameters(a): Parameters<StatusArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "mission_set_status", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            require_top(&me)?;
            let mut m = s.db.get_mission(&own_mission(&me)?)?;
            if m.status == "arquivada" {
                return fail("archived", "missão arquivada não pode ser alterada");
            }
            if !["ativa", "pronta", "bloqueada", "concluida"].contains(&a.status.as_str()) {
                return fail("bad_status", "estado: ativa, pronta, bloqueada ou concluida");
            }
            m.status = a.status;
            m.status_reason = a.reason;
            s.db.update_mission(&m)?;
            Ok(json!(m))
        })())
    }

    /// Integra a worktree da sua missão na branch base (a missão vira concluida, ou bloqueada com o motivo).
    #[tool(description = "Integra a worktree da sua missão na branch base (a missão vira concluida, ou bloqueada com o motivo)")]
    async fn mission_integrate(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "mission_integrate", &parts);
        let st = self.state.clone();
        finish(
            blocking(move || {
                let me = caller_row(&st, &parts)?;
                require_top(&me)?;
                let m = missions::integrate_mission(&st, &own_mission(&me)?)?;
                Ok(json!(m))
            })
            .await,
        )
    }

    /// Lê (ou troca, com `set`) o brief da sua missão.
    #[tool(description = "Lê (ou troca, com set) o brief da sua missão")]
    async fn mission_brief(&self, Parameters(a): Parameters<BriefArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "mission_brief", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            if a.set.is_some() {
                require_top(&me)?;
            }
            let mut m = s.db.get_mission(&own_mission(&me)?)?;
            if let Some(text) = a.set {
                if text.len() > 4096 {
                    return fail("brief_too_big", "brief acima de 4 KB");
                }
                m.brief = Some(text);
                s.db.update_mission(&m)?;
            }
            Ok(json!({"brief": m.brief}))
        })())
    }
}
