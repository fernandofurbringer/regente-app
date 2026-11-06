use super::{caller_row, finish, note, own_mission, pane_in_ws, RegenteMcp};
use crate::db::TaskItem;
use crate::error::{fail, AppResult};
use crate::runtime::AppState;
use crate::util::{new_id, now_ms};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

const STATUSES: &[&str] = &["a_fazer", "fazendo", "feito"];

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateArgs {
    /// Título da tarefa.
    pub title: String,
    /// Estado: a_fazer (padrão), fazendo ou feito.
    pub status: Option<String>,
    /// Id do painel responsável (opcional).
    pub owner: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateArgs {
    /// Id da tarefa (da sua missão).
    pub id: String,
    /// Novo estado: a_fazer, fazendo ou feito.
    pub status: Option<String>,
    /// Novo título.
    pub title: Option<String>,
    /// Novo responsável (id do painel).
    pub owner: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct IdArgs {
    /// Id da tarefa (da sua missão).
    pub id: String,
}

fn check_status(st: &str) -> AppResult<()> {
    if STATUSES.contains(&st) {
        Ok(())
    } else {
        fail("bad_status", "estado: a_fazer, fazendo ou feito")
    }
}

/// The mission's pilot or maestro (unless it has a manual name) takes the first tasks' titles as its name.
fn name_pilot(s: &AppState, mission: &str) -> AppResult<()> {
    let panes = s.db.mission_panes(mission)?;
    let Some(pilot) = panes.iter().find(|p| p.closed_at.is_none() && matches!(p.role.as_deref(), Some("piloto" | "maestro")) && !p.name_manual) else {
        return Ok(());
    };
    let titles: Vec<String> = s.db.tasks(mission)?.into_iter().take(3).map(|t| t.title).collect();
    let name: String = titles.join(" · ").chars().take(40).collect();
    if name.is_empty() {
        return Ok(());
    }
    s.db.set_pane_name(&pilot.id, &name, false)?;
    s.emit("pane-renamed", json!({"id": pilot.id, "name": name}));
    Ok(())
}

/// A task of the caller's mission.
fn own_task(s: &AppState, mission: &str, id: &str) -> AppResult<TaskItem> {
    s.db.tasks(mission)?.into_iter().find(|t| t.id == id).ok_or_else(|| crate::error::AppError::new("task_not_found", "tarefa não encontrada nesta missão"))
}

#[tool_router(router = tasks_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Cria uma tarefa na sua missão. Se o piloto ainda não tem nome manual, ele ganha os títulos das primeiras tarefas.
    #[tool(description = "Cria uma tarefa na sua missão. Se o piloto ainda não tem nome manual, ele ganha os títulos das primeiras tarefas")]
    async fn task_create(&self, Parameters(a): Parameters<CreateArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "task_create", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let mission = own_mission(&me)?;
            let status = a.status.unwrap_or_else(|| "a_fazer".into());
            check_status(&status)?;
            if a.title.trim().is_empty() {
                return fail("bad_title", "o título não pode ficar vazio");
            }
            if let Some(owner) = &a.owner {
                pane_in_ws(s, &me.workspace_id, owner)?;
            }
            let now = now_ms();
            let t = TaskItem { id: new_id(), mission_id: mission.clone(), title: a.title.trim().to_string(), status, owner_pane: a.owner, created_at: now, updated_at: now };
            s.db.upsert_task(&t)?;
            name_pilot(s, &mission)?;
            s.emit("tasks-changed", json!({"missionId": mission}));
            Ok(json!(t))
        })())
    }

    /// Muda estado, título ou responsável de uma tarefa da sua missão.
    #[tool(description = "Muda estado, título ou responsável de uma tarefa da sua missão")]
    async fn task_update(&self, Parameters(a): Parameters<UpdateArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "task_update", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let mission = own_mission(&me)?;
            let mut t = own_task(s, &mission, &a.id)?;
            if let Some(st) = a.status {
                check_status(&st)?;
                t.status = st;
            }
            if let Some(title) = a.title {
                if title.trim().is_empty() {
                    return fail("bad_title", "o título não pode ficar vazio");
                }
                t.title = title.trim().to_string();
            }
            if let Some(owner) = a.owner {
                pane_in_ws(s, &me.workspace_id, &owner)?;
                t.owner_pane = Some(owner);
            }
            t.updated_at = now_ms();
            s.db.upsert_task(&t)?;
            s.emit("tasks-changed", json!({"missionId": mission}));
            Ok(json!(t))
        })())
    }

    /// Tarefas da sua missão.
    #[tool(description = "Tarefas da sua missão")]
    async fn task_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "task_list", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            Ok(json!({"tasks": s.db.tasks(&own_mission(&me)?)?}))
        })())
    }

    /// Apaga uma tarefa da sua missão.
    #[tool(description = "Apaga uma tarefa da sua missão")]
    async fn task_delete(&self, Parameters(a): Parameters<IdArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "task_delete", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let mission = own_mission(&me)?;
            own_task(s, &mission, &a.id)?;
            s.db.delete_task(&a.id)?;
            s.emit("tasks-changed", json!({"missionId": mission}));
            Ok(json!({"ok": true}))
        })())
    }
}
