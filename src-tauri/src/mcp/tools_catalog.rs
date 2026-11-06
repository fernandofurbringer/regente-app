use super::tools_panes::plain_arg;
use super::{blocking, caller_row, check_texts, finish, not_archived, note, require_top, RegenteMcp};
use crate::catalog::{self, AgentDef, HarnessArgs};
use crate::error::{AppError, AppResult};
use crate::marketplace;
use crate::orchestrator::clean_text;
use crate::squads;
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::util::home_dir;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, JsonSchema)]
pub struct NameArg {
    /// Nome do agente ou da skill.
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvokeArgs {
    /// Nome do agente (ver agent_list).
    pub name: String,
    /// Pedido para o agente; vai no fim do prompt, na seção "Pedido".
    pub request: String,
    /// Esforço (sobrescreve o do agente).
    pub effort: Option<String>,
    /// Modelo (sobrescreve o do agente).
    pub model: Option<String>,
    /// CLI (sobrescreve o do agente).
    pub cli: Option<String>,
    /// Skills permitidas (lista vazia = nenhuma; omitido = as do papel e do agente).
    pub allowed_skills: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentSaveArgs {
    /// Nome do agente: minúsculas, dígitos e hífen.
    pub name: String,
    /// Descrição de uma linha.
    pub description: String,
    /// CLI padrão do agente.
    pub cli: Option<String>,
    /// Modelo padrão do agente.
    pub model: Option<String>,
    /// Esforço padrão do agente.
    pub effort: Option<String>,
    /// Papel do painel: executor, reviewer, scout, planner, piloto...
    pub role: Option<String>,
    /// Skills do agente (nomes).
    pub skills: Option<Vec<String>>,
    /// Prompt do agente.
    pub body: String,
}

/// The prompt of an agent created over MCP (the 4000-char cap is for the other text fields).
const AGENT_BODY_MAX: usize = 20_000;

#[derive(Deserialize, JsonSchema)]
pub struct CatalogListArgs {
    /// Filtra por tipo: "agent" ou "skill" (omitido = os dois).
    pub kind: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CatalogInstallArgs {
    /// Fonte no formato owner/repo (ver marketplace_search).
    pub source: String,
    /// Nome do item no catálogo.
    pub name: String,
    /// Destino: claude, codex ou regente.
    pub target: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CatalogUninstallArgs {
    /// Nome do item instalado.
    pub name: String,
    /// Tipo: skill ou agent.
    pub kind: String,
    /// Destino: claude, codex ou regente.
    pub target: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Texto procurado no nome e na descrição.
    pub q: String,
    /// Tipo: skill ou agent (omitido = os dois).
    pub kind: Option<String>,
}

fn agent_summary(a: &AgentDef) -> Value {
    json!({"name": a.name, "description": a.description, "cli": a.cli, "model": a.model, "effort": a.effort, "role": a.role, "source": a.source})
}

/// Installs and removals from MCP only reach the Regente folder, and never a seeded original's name.
/// Claude and Codex folders are changed from the Regente UI.
fn mcp_guard(data: &std::path::Path, name: &str, target: &str) -> AppResult<()> {
    if target != "regente" {
        return crate::error::fail("mcp_target", "para claude/codex use a UI do Regente");
    }
    if catalog::is_original(data, name) {
        return crate::error::fail("reserved_name", format!("{name} é um original do Regente: o nome está reservado"));
    }
    Ok(())
}

#[tool_router(router = catalog_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Agentes do catálogo (originais, do Regente, do Claude), com CLI, modelo e papel.
    #[tool(description = "Agentes do catálogo (originais, do Regente, do Claude), com CLI, modelo e papel")]
    async fn agent_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "agent_list", &parts);
        let st = self.state.clone();
        let r = blocking(move || -> AppResult<Value> {
            let agents: Vec<Value> = catalog::list_agents(&st.data, &home_dir()).iter().map(agent_summary).collect();
            Ok(json!({"agents": agents}))
        })
        .await;
        finish(r)
    }

    /// Um agente completo, com o prompt.
    #[tool(description = "Um agente completo, com o prompt")]
    async fn agent_get(&self, Parameters(a): Parameters<NameArg>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "agent_get", &parts);
        let r = catalog::get_agent(&self.state.data, &home_dir(), &a.name).map(|g| json!(g)).ok_or_else(|| AppError::new("unknown_agent", format!("agente desconhecido: {}", a.name)));
        finish(r)
    }

    /// Abre um painel de agente filho com o prompt do agente e o pedido. Devolve o recibo (painel, harness, skills).
    #[tool(description = "Abre um painel de agente filho com o prompt do agente e o pedido. Devolve o recibo (painel, harness, skills)")]
    async fn agent_invoke(&self, Parameters(a): Parameters<InvokeArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "agent_invoke", &parts);
        let st = self.state.clone();
        let r = blocking(move || -> AppResult<Value> {
            let s = &st;
            let me = caller_row(s, &parts)?;
            not_archived(s, &me)?;
            let home = home_dir();
            let agent = catalog::get_agent(&s.data, &home, &a.name).ok_or_else(|| AppError::new("unknown_agent", format!("agente desconhecido: {}", a.name)))?;
            // these values reach the CLI command line
            for v in [a.cli.as_deref(), a.model.as_deref(), a.effort.as_deref()].into_iter().flatten() {
                plain_arg(v)?;
            }
            let arg = HarnessArgs { cli: a.cli.clone(), model: a.model.clone(), effort: a.effort.clone() };
            // roster of the caller's squad mission, if the agent is in it: it beats the argument and the agent default
            let squad = me.mission_id.as_deref().and_then(|m| s.db.get_mission(m).ok()).and_then(|m| m.squad_id).and_then(|id| squads::get(&s.data, &id));
            let roster = squad.as_ref().and_then(|sq| squads::roster_harness(sq, &a.name));
            // a escolha do usuário vence: o time (squad) que ele montou e, por baixo, o slot do perfil para o papel do
            // agente (executor → implementar, revisor → revisar…). O pedido do orquestrador e o arquivo do agente só
            // valem para o que nenhum dos dois disser.
            let (perfil, _, _) = crate::perfil::efetivo(s, &me.workspace_id, me.mission_id.as_deref())?;
            let slot = crate::perfil::slot_args(crate::perfil::slot_do_agente(&perfil, agent.role.as_deref()));
            let topo = match &roster {
                Some(r) => catalog::sobre(r, &slot),
                None => slot,
            };
            let harness = catalog::resolve_harness(Some(&topo), &arg, Some(&agent), &HarnessArgs::default(), "claude");
            let role = agent.role.clone().unwrap_or_else(|| "agente".into());
            if super::protected_role(&role) {
                return crate::error::fail("protected_role", format!("o papel {role} é reservado ao app"));
            }
            // the squad's roster may cap how many of this agent run at once in the mission
            if let Some(max) = squad.as_ref().and_then(|sq| sq.roster.iter().find(|r| r.agent == a.name)).and_then(|r| r.max) {
                let live = s.list_panes(&me.workspace_id)?.iter().filter(|p| p.mission_id == me.mission_id && p.agent.as_deref() == Some(a.name.as_str())).count();
                if live >= max as usize {
                    return crate::error::fail("roster_max", format!("o time já tem {max} \"{}\" aberto(s) nesta missão", a.name));
                }
            }
            super::check_spawn_budget(s, &me, 1)?;
            let permission = super::cap_perm(&me, Some(Permission::Auto))?;
            let prompt = format!(
                "{}\n\n## Pedido\n{}\n\nAo terminar: mcp__regente__handoff_submit.",
                clean_text(&agent.body),
                clean_text(&a.request)
            );
            let skills = catalog::pane_skills(Some(&role), Some(&agent), a.allowed_skills.as_deref());
            let spec = PaneSpec {
                workspace_id: me.workspace_id.clone(),
                mission_id: me.mission_id.clone(),
                kind: PaneKind::Agent,
                name: Some(agent.name.clone()),
                cli: Some(harness.cli.clone()),
                model: harness.model.clone(),
                effort: harness.effort.clone(),
                permission,
                cwd: me.cwd.clone(),
                prompt: Some(prompt),
                parent_id: Some(me.id.clone()),
                role: Some(role),
                agent: Some(agent.name.clone()),
                allowed_skills: a.allowed_skills.clone(),
                ..Default::default()
            };
            let row = s.spawn_pane(spec)?;
            Ok(json!({
                "pane": {"id": row.id, "name": row.name},
                "harness": {"cli": harness.cli, "model": harness.model, "effort": harness.effort, "source": harness.source},
                "skills": skills,
            }))
        })
        .await;
        finish(r)
    }

    /// Cria ou substitui um agente do Regente (vale sobre o original de mesmo nome).
    #[tool(description = "Cria ou substitui um agente do Regente (vale sobre o original de mesmo nome)")]
    async fn agent_save(&self, Parameters(a): Parameters<AgentSaveArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "agent_save", &parts);
        let r = (|| -> AppResult<Value> {
            require_top(&caller_row(&self.state, &parts)?)?;
            if catalog::get_agent(&self.state.data, &home_dir(), &a.name).is_some() {
                return crate::error::fail("exists", "já existe um agente com este nome (o MCP só cria; alterar é pela UI)");
            }
            check_texts(&[a.description.as_str(), a.role.as_deref().unwrap_or("")])?;
            if a.role.as_deref().is_some_and(super::protected_role) {
                return crate::error::fail("protected_role", "papel reservado ao app (piloto, maestro ou chefe)");
            }
            if a.body.chars().count() > AGENT_BODY_MAX {
                return crate::error::fail("too_big", "o prompt do agente passa de 20000 caracteres");
            }
            for v in [a.cli.as_deref(), a.model.as_deref(), a.effort.as_deref()].into_iter().flatten() {
                plain_arg(v)?;
            }
            let agent = AgentDef {
                name: a.name.clone(),
                description: a.description.clone(),
                cli: a.cli.clone(),
                model: a.model.clone(),
                effort: a.effort.clone(),
                role: a.role.clone(),
                skills: a.skills.clone().unwrap_or_default(),
                body: a.body.clone(),
                ..Default::default()
            };
            let path = catalog::save_agent(&self.state.data, &agent)?;
            Ok(json!({"path": path}))
        })();
        finish(r)
    }

    /// Agentes e skills do catálogo local (filtro opcional por tipo).
    #[tool(description = "Agentes e skills do catálogo local (filtro opcional por tipo)")]
    async fn catalog_list(&self, Parameters(a): Parameters<CatalogListArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "catalog_list", &parts);
        let st = self.state.clone();
        let r = blocking(move || -> AppResult<Value> {
            let home = home_dir();
            let want = |k: &str| a.kind.as_deref().is_none_or(|x| x == k);
            let mut out = json!({});
            if want("agent") {
                out["agents"] = catalog::list_agents(&st.data, &home).iter().map(agent_summary).collect::<Vec<_>>().into();
            }
            if want("skill") {
                out["skills"] = json!(catalog::list_skills(&st.data, &home));
            }
            Ok(out)
        })
        .await;
        finish(r)
    }

    /// Instala um item do marketplace (skill ou agente) no Claude, no Codex ou no Regente.
    #[tool(description = "Instala um item do marketplace (skill ou agente) no Claude, no Codex ou no Regente")]
    async fn catalog_install(&self, Parameters(a): Parameters<CatalogInstallArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "catalog_install", &parts);
        let s = &self.state;
        let r: AppResult<Value> = async {
            require_top(&caller_row(s, &parts)?)?;
            mcp_guard(&s.data, &a.name, &a.target)?;
            let client = marketplace::client()?;
            let items = marketplace::refresh(&s.data, &client, false).await?;
            let item = items.into_iter().find(|i| i.source == a.source && i.name == a.name).ok_or_else(|| AppError::new("unknown_item", "item não encontrado no catálogo"))?;
            let path = marketplace::install(&item, &a.target, &s.data, &home_dir(), &client).await?;
            Ok(json!({"path": path}))
        }
        .await;
        finish(r)
    }

    /// Remove um item instalado (vai para a Lixeira).
    #[tool(description = "Remove um item instalado (vai para a Lixeira)")]
    async fn catalog_uninstall(&self, Parameters(a): Parameters<CatalogUninstallArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "catalog_uninstall", &parts);
        let r = caller_row(&self.state, &parts).and_then(|me| require_top(&me)).and_then(|()| mcp_guard(&self.state.data, &a.name, &a.target)).and_then(|()| marketplace::uninstall(&a.name, &a.kind, &a.target, &self.state.data, &home_dir()));
        finish(r.map(|()| json!({"ok": true})))
    }

    /// Procura no marketplace (fontes GitHub configuradas) por nome ou descrição.
    #[tool(description = "Procura no marketplace (fontes GitHub configuradas) por nome ou descrição")]
    async fn marketplace_search(&self, Parameters(a): Parameters<SearchArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "marketplace_search", &parts);
        let s = &self.state;
        let r: AppResult<Value> = async {
            let items = marketplace::refresh(&s.data, &marketplace::client()?, false).await?;
            Ok(json!(marketplace::search(&items, &a.q, a.kind.as_deref())))
        }
        .await;
        finish(r)
    }

    /// Conteúdo do SKILL.md de uma skill (originais, do Regente, do Claude ou do Codex).
    #[tool(description = "Conteúdo do SKILL.md de uma skill (originais, do Regente, do Claude ou do Codex)")]
    async fn skill_get(&self, Parameters(a): Parameters<NameArg>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "skill_get", &parts);
        let s = &self.state;
        let r = catalog::find_skill(&s.data, &home_dir(), &a.name)
            .ok_or_else(|| AppError::new("unknown_skill", format!("skill desconhecida: {}", a.name)))
            .and_then(|sk| {
                let content = std::fs::read_to_string(std::path::Path::new(&sk.dir).join("SKILL.md"))?;
                Ok(json!({"name": sk.name, "source": sk.source, "content": content}))
            });
        finish(r)
    }
}
