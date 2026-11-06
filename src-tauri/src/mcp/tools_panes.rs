use super::{blocking, cap_perm, caller_row, check_spawn_budget, finish, may_control, not_archived, note, outranks, pane_in_ws, RegenteMcp};
use crate::db::PaneRow;
use crate::error::{fail, AppError, AppResult};
use crate::orchestrator::{bracketed, clean_text, last_lines, strip_ansi};
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// A value that goes onto a CLI command line: no control chars, quotes, `%` or a leading `-` (it would read as a flag).
pub(crate) fn plain_arg(v: &str) -> AppResult<()> {
    if v.starts_with('-') || v.chars().any(|c| c.is_control() || c == '"' || c == '%') {
        return fail("bad_arg", "valor com caracteres de controle, aspas, % ou começando por - não é permitido");
    }
    Ok(())
}

/// Pura: um ajudante não ganha MCP que o pai não tem. Pai `full` não restringe; pai `minimal` não dá servidor extra;
/// pai `custom` dá só a lista dele. Nome fora do permitido falha igual, sem dizer se o servidor existe.
pub(crate) fn child_mcp_check(parent_profile: &str, parent_servers: &[String], child_profile: &str, child_servers: &[String]) -> AppResult<()> {
    if parent_profile == "full" || parent_profile.is_empty() {
        return Ok(());
    }
    let allowed: &[String] = if parent_profile == "custom" { parent_servers } else { &[] };
    if child_profile == "full" || child_servers.iter().any(|n| !allowed.contains(n)) {
        return fail("bad_mcp_server", "servidor MCP indisponível para este painel");
    }
    Ok(())
}

/// One pane of a `pane_spawn`: in the caller's mission and folder; its handoff schema is kept on disk
/// (`panes/<id>/handoff.json`) so it survives a respawn.
fn spawn_one(s: &AppState, me: &PaneRow, a: &SpawnArgs, permission: Permission, profile: &str) -> AppResult<PaneRow> {
    let spec = PaneSpec {
        workspace_id: me.workspace_id.clone(),
        mission_id: me.mission_id.clone(),
        kind: PaneKind::Agent,
        name: a.name.clone(),
        cli: Some(a.cli.clone()),
        model: a.model.clone(),
        effort: a.effort.clone(),
        permission,
        cwd: me.cwd.clone(),
        prompt: a.prompt.clone(),
        parent_id: Some(me.id.clone()),
        role: Some(a.role.clone().unwrap_or_else(|| "ajudante".into())),
        mcp_profile: Some(profile.into()),
        mcp_servers: a.mcp_servers.clone(),
        allowed_skills: a.allowed_skills.clone(),
        ..Default::default()
    };
    let row = s.spawn_pane(spec)?;
    if let Some(schema) = &a.handoff_schema {
        let dir = s.data.join("panes").join(&row.id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("handoff.json"), schema.to_string())?;
        s.rt().handoff_schemas.insert(row.id.clone(), schema.clone());
    }
    Ok(row)
}

/// Output of a pane as text, without terminal escapes.
fn screen(s: &AppState, id: &str) -> String {
    strip_ansi(&String::from_utf8_lossy(&s.panes.snapshot(id)))
}

/// A pane a pane tool acts on: the caller must control it, and it must be an AI pane (no terminal or browser).
pub(crate) fn reachable(s: &AppState, me: &PaneRow, row: &PaneRow, need_agent: bool) -> AppResult<()> {
    if !may_control(s, me, row) {
        return fail("forbidden", "o painel não é seu, nem de um filho seu, nem de um não-chefe da sua missão");
    }
    if need_agent && row.kind != "agent" {
        return fail("forbidden", "só painéis de IA recebem ou mostram texto por aqui");
    }
    Ok(())
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnArgs {
    /// CLI de IA da lista de provedores (ex.: claude, codex, gemini).
    pub cli: String,
    /// Modelo (opcional; o padrão do CLI se omitido).
    pub model: Option<String>,
    /// Esforço de raciocínio (opcional).
    pub effort: Option<String>,
    /// Prompt inicial, enviado quando a IA estiver pronta.
    pub prompt: Option<String>,
    /// Nome do painel (opcional; sem nome, um ajudante ganha um nome tirado do prompt).
    pub name: Option<String>,
    /// Papel: só "ajudante" (padrão). Piloto, maestro e chefes são abertos pelo app.
    pub role: Option<String>,
    /// Permissão: pend, plan ou auto. Padrão: a do seu painel; nunca acima da sua. danger só pela interface.
    pub permission: Option<String>,
    /// Perfil de MCP: full (padrão) ou minimal (só o servidor Regente).
    pub mcp_profile: Option<String>,
    /// Servidores MCP extras mantidos no perfil minimal.
    pub mcp_servers: Option<Vec<String>>,
    /// Skills permitidas (lista vazia = nenhuma; omitido = todas).
    pub allowed_skills: Option<Vec<String>>,
    /// JSON Schema que os dados da entrega (handoff_submit) precisam seguir.
    pub handoff_schema: Option<Value>,
    /// Quantos painéis abrir, de 1 a 8.
    pub count: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaneArg {
    /// Id do painel (dentro do seu workspace).
    pub pane: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WriteArgs {
    /// Id do painel que recebe o texto.
    pub pane: String,
    /// Texto a digitar no painel.
    pub text: String,
    /// Enviar Enter no fim (padrão: sim).
    pub enter: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    /// Id do painel.
    pub pane: String,
    /// Quantas linhas finais devolver (padrão 120).
    pub lines: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenameArgs {
    /// Id do painel.
    pub pane: String,
    /// Novo nome (fica como nome manual).
    pub name: String,
}

#[tool_router(router = panes_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Abre de 1 a 8 painéis de IA filhos, na missão e na pasta deste painel.
    #[tool(description = "Abre de 1 a 8 painéis de IA filhos, na missão e na pasta deste painel")]
    async fn pane_spawn(&self, Parameters(a): Parameters<SpawnArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_spawn", &parts);
        // the spawn is a process start: it runs off the async runtime
        let st = self.state.clone();
        finish(blocking(move || -> AppResult<Value> {
            let s = &st;
            let me = caller_row(s, &parts)?;
            not_archived(s, &me)?;
            // only helpers: the pilot, the chiefs and the maestro are opened by the app itself
            if a.role.as_deref().is_some_and(|r| r != "ajudante") {
                return fail("bad_role", "pane_spawn só abre ajudantes (papel \"ajudante\")");
            }
            let count = a.count.unwrap_or(1);
            if !(1..=8).contains(&count) {
                return fail("bad_count", "quantidade de painéis entre 1 e 8");
            }
            let requested: Option<Permission> = match a.permission.as_deref() {
                Some(p) => Some(serde_json::from_value(json!(p)).map_err(|_| AppError::new("bad_permission", "permissão inválida: use danger, auto, pend ou plan"))?),
                None => None,
            };
            let permission = cap_perm(&me, requested)?;
            check_spawn_budget(s, &me, count as usize)?;
            // RM-18: sem perfil pedido, o ajudante sobe com `perfil.mcp.ajudantes` (minimal por padrão)
            let profile = match a.mcp_profile.clone() {
                Some(p) => p,
                None => crate::perfil::efetivo(s, &me.workspace_id, me.mission_id.as_deref())?.0.mcp.ajudantes,
            };
            if profile != "full" && profile != "minimal" && profile != "custom" {
                return fail("bad_profile", "perfil de MCP: full, minimal ou custom");
            }
            let parent_servers: Vec<String> = me.mcp_servers.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
            child_mcp_check(&me.mcp_profile, &parent_servers, &profile, a.mcp_servers.as_deref().unwrap_or_default())?;
            if let Some(schema) = &a.handoff_schema {
                jsonschema::validator_for(schema).map_err(|e| AppError::new("bad_schema", format!("handoffSchema inválido: {e}")))?;
            }
            // these values reach the CLI command line: no control chars, quotes or `%`
            let checked = [a.model.as_deref(), a.effort.as_deref()].into_iter().flatten().chain(a.mcp_servers.iter().flatten().map(String::as_str)).chain(a.allowed_skills.iter().flatten().map(String::as_str));
            for v in checked {
                plain_arg(v)?;
            }
            let mut panes = vec![];
            let mut failed = None;
            for _ in 0..count {
                match spawn_one(s, &me, &a, permission, &profile) {
                    Ok(row) => panes.push(json!({"id": row.id, "name": row.name})),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            // a failure before the first pane is the tool's error; after it, the panes already open are reported
            let partial = match failed {
                Some(e) if panes.is_empty() => return Err(e),
                Some(e) => Some(e.to_string()),
                None => None,
            };
            let mut kept = vec!["regente".to_string()];
            kept.extend(a.mcp_servers.clone().unwrap_or_default());
            // only claude restricts its MCP servers (`--strict-mcp-config`); the other CLIs keep theirs
            let excluded = match (profile.as_str(), a.cli.as_str()) {
                ("minimal", "claude") => "todos os outros (claude: --strict-mcp-config)",
                ("minimal", _) => "nenhum: este CLI não restringe servidores MCP",
                _ => "nenhum",
            };
            let mut out = json!({"panes": panes, "mcp": {"profile": profile, "kept": kept, "excluded": excluded}});
            if let Some(msg) = partial {
                out["error"] = json!(msg);
            }
            Ok(out)
        }).await)
    }

    /// Digita texto num painel (Enter no fim por padrão). Painel hibernado é acordado e recebe o texto.
    #[tool(description = "Digita texto num painel (Enter no fim por padrão). Painel hibernado é acordado e recebe o texto")]
    async fn pane_write(&self, Parameters(a): Parameters<WriteArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_write", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let row = pane_in_ws(s, &me.workspace_id, &a.pane)?;
            reachable(s, &me, &row, true)?;
            if outranks(&row, &me) {
                return fail("forbidden", "o painel alvo tem permissão acima da sua");
            }
            let enter = a.enter.unwrap_or(true);
            let bytes = if enter || a.text.contains('\n') { bracketed(&a.text) } else { clean_text(&a.text).into_bytes() };
            s.write_input(&row.id, &bytes)?;
            Ok(json!({"ok": true}))
        })())
    }

    /// Últimas linhas da tela do painel. Painel hibernado devolve o quadro congelado.
    #[tool(description = "Últimas linhas da tela do painel. Painel hibernado devolve o quadro congelado")]
    async fn pane_read(&self, Parameters(a): Parameters<ReadArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_read", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let row = pane_in_ws(s, &me.workspace_id, &a.pane)?;
            reachable(s, &me, &row, true)?;
            let live = s.status_of(&row.id);
            let hibernated = live == Some(crate::status::PaneStatus::Hibernated);
            let status = live.map(|x| x.as_str().to_string()).unwrap_or_else(|| row.status.clone());
            let full = if hibernated { strip_ansi(&s.db.get_frame(&row.id)?.unwrap_or_default()) } else { screen(s, &row.id) };
            let mut out = json!({"status": status, "totalLines": full.lines().count(), "text": last_lines(&full, a.lines.unwrap_or(120))});
            if hibernated {
                out["hibernated"] = json!(true);
            }
            Ok(out)
        })())
    }

    /// Painéis abertos do workspace, com estado, papel, custo e pai.
    #[tool(description = "Painéis abertos do workspace, com estado, papel, custo e pai")]
    async fn pane_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_list", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let panes: Vec<Value> = s
                .list_panes(&me.workspace_id)?
                .into_iter()
                .map(|p| json!({"id": p.id, "name": p.name, "status": p.status, "role": p.role, "cli": p.cli, "model": p.model, "parentId": p.parent_id, "missionId": p.mission_id, "costUsd": p.cost_usd}))
                .collect();
            Ok(json!({"panes": panes}))
        })())
    }

    /// Fecha um painel do workspace.
    #[tool(description = "Fecha um painel do workspace")]
    async fn pane_close(&self, Parameters(a): Parameters<PaneArg>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_close", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let row = pane_in_ws(s, &me.workspace_id, &a.pane)?;
            reachable(s, &me, &row, false)?;
            s.close_pane(&row.id)?;
            Ok(json!({"ok": true}))
        })())
    }

    /// Dá um nome manual a um painel.
    #[tool(description = "Dá um nome manual a um painel")]
    async fn pane_rename(&self, Parameters(a): Parameters<RenameArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_rename", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let row = pane_in_ws(s, &me.workspace_id, &a.pane)?;
            // renaming types `/rename` into the pane, so it is a write into it: the same rules as pane_write
            reachable(s, &me, &row, true)?;
            if outranks(&row, &me) {
                return fail("forbidden", "o painel alvo tem permissão acima da sua");
            }
            s.rename_pane(&row.id, &a.name)?;
            Ok(json!({"ok": true}))
        })())
    }

    /// Hiberna um painel (guarda a sessão e o quadro atual para retomar depois).
    #[tool(description = "Hiberna um painel (guarda a sessão e o quadro atual para retomar depois)")]
    async fn pane_hibernate(&self, Parameters(a): Parameters<PaneArg>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_hibernate", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let row = pane_in_ws(s, &me.workspace_id, &a.pane)?;
            reachable(s, &me, &row, false)?;
            s.hibernate(&row.id, &screen(s, &row.id))?;
            Ok(json!({"ok": true}))
        })())
    }

    /// Estado de um painel: status, última pergunta, tempo no ar, tokens e custo.
    #[tool(description = "Estado de um painel: status, última pergunta, tempo no ar, tokens e custo")]
    async fn pane_status(&self, Parameters(a): Parameters<PaneArg>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "pane_status", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let row = pane_in_ws(s, &me.workspace_id, &a.pane)?;
            let status = s.status_of(&row.id).map(|x| x.as_str().to_string()).unwrap_or_else(|| row.status.clone());
            let uptime = s.panes.activity(&row.id).map(|(started, _)| started.elapsed().as_secs()).unwrap_or(0);
            let last_question = s.rt().last_question.get(&row.id).cloned();
            Ok(json!({
                "status": status,
                "lastQuestion": last_question,
                "uptimeSec": uptime,
                "tokens": {"in": row.tok_in, "out": row.tok_out, "cacheRead": row.tok_cache_r, "cacheWrite": row.tok_cache_w},
                "costUsd": row.cost_usd,
            }))
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::child_mcp_check;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_full_parent_does_not_restrict_its_helpers() {
        assert!(child_mcp_check("full", &[], "full", &names(&["qualquer"])).is_ok());
        assert!(child_mcp_check("", &[], "full", &[]).is_ok());
    }

    #[test]
    fn a_minimal_parent_gives_its_helpers_no_extra_server() {
        assert!(child_mcp_check("minimal", &[], "minimal", &[]).is_ok());
        assert!(child_mcp_check("minimal", &[], "custom", &[]).is_ok());
        assert_eq!(child_mcp_check("minimal", &[], "minimal", &names(&["github"])).unwrap_err().code(), "bad_mcp_server");
        assert_eq!(child_mcp_check("minimal", &[], "full", &[]).unwrap_err().code(), "bad_mcp_server", "full é escalada");
    }

    #[test]
    fn a_custom_parent_gives_only_its_own_list_and_unknown_names_fail_the_same_way() {
        let parent = names(&["github", "fs"]);
        assert!(child_mcp_check("custom", &parent, "custom", &names(&["fs"])).is_ok());
        let unknown = child_mcp_check("custom", &parent, "custom", &names(&["inexistente"])).unwrap_err();
        let outside = child_mcp_check("custom", &parent, "custom", &names(&["github", "segredo-do-pai"])).unwrap_err();
        assert_eq!(unknown.code(), "bad_mcp_server");
        assert_eq!(unknown.to_string(), child_mcp_check("custom", &parent, "custom", &names(&["outro-nome"])).unwrap_err().to_string());
        assert_eq!(outside.code(), "bad_mcp_server");
        assert_eq!(child_mcp_check("custom", &parent, "full", &[]).unwrap_err().code(), "bad_mcp_server");
    }
}
