use super::tools_panes::plain_arg;
use super::{blocking, cap_perm, caller_row, check_spawn_budget, check_texts, finish, note, not_archived, own_mission, RegenteMcp};
use crate::agentic::{self, AgenticConfig, TaskType};
use crate::catalog::{self, AgentDef, HarnessArgs};
use crate::db::{Decision, Mission, PaneRow};
use crate::error::{fail, AppError, AppResult};
use crate::orchestrator::{auto_name, bracketed, clean_text};
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::util::{home_dir, lk, new_id, now_ms};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// A helper's session is resumed when the helper closed less than 30 minutes ago.
const RESUME_WINDOW_MS: i64 = 30 * 60 * 1000;
/// Helpers still open in one mission, at most.
const MAX_LIVE_HELPERS: usize = 12;
const CHIEFS: [&str; 5] = ["scout", "builder", "reviewer", "controller", "cruzado"];

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DelegateArgs {
    /// Pedido para o ajudante (vai no fim do prompt, na seção "Pedido").
    pub request: String,
    /// Tipo de tarefa (ver a tabela "Quem faz cada tarefa"). Omitido: o Decisor classifica, se houver.
    pub task_type: Option<String>,
    /// Agente a usar (sobrescreve o do tipo).
    pub agent: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ChiefArgs {
    /// Chefe: scout, builder, reviewer, controller (só no estilo torre) ou cruzado (revisão cruzada: outra IA confere
    /// os achados do reviewer; só com o revisor cruzado ligado no perfil).
    #[serde(rename = "type")]
    pub chief: String,
    /// Pedido para o chefe.
    pub request: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct DecisionListArgs {
    /// Quantas decisões devolver (padrão 20).
    pub limit: Option<i64>,
}

/// What `delegate` knows before it asks the Decisor: checked first, so a refused caller costs nothing.
struct Gate {
    me: PaneRow,
    mission: Mission,

    cfg: AgenticConfig,
    agents: Vec<(String, String)>,
}

/// The type chosen for a delegation, and who chose it.
struct Picked {
    task: TaskType,
    decided_by: String,
    cost_usd: f64,
    decisor_agent: Option<String>,
}

/// The caller's mission, which must be agentic, with its style (the default style when the mission has none).
fn agentic_mission(s: &AppState, me: &PaneRow) -> AppResult<(Mission, String)> {
    let m = s.db.get_mission(&own_mission(me)?)?;
    if m.mode != "agentico" {
        return fail("not_agentic", "este painel não está em uma missão agêntica");
    }
    let style = m.style.clone().unwrap_or_else(|| agentic::load(&s.data).default_style);
    Ok((m, style))
}

fn type_ids(cfg: &AgenticConfig) -> String {
    cfg.types.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", ")
}

fn type_of<'a>(cfg: &'a AgenticConfig, id: &str) -> AppResult<&'a TaskType> {
    cfg.types.iter().find(|t| t.id == id).ok_or_else(|| AppError::new("unknown_task_type", format!("tipo desconhecido: {id}; tipos: {}", type_ids(cfg))))
}

/// The prompt of a helper or a chief: the agent's body (if any), then the request and the handoff reminder.
fn child_prompt(agent: Option<&AgentDef>, request: &str) -> String {
    let body = agent.map(|a| format!("{}\n\n", clean_text(&a.body))).unwrap_or_default();
    format!("{body}## Pedido\n{}\n\nAo terminar: mcp__regente__handoff_submit.", clean_text(request))
}

/// Who may delegate: the pilot, and any chief (in torre, only the controller).
fn gate(s: &AppState, parts: &http::request::Parts, request: &str) -> AppResult<Gate> {
    let me = caller_row(s, parts)?;
    not_archived(s, &me)?;
    let (mission, style) = agentic_mission(s, &me)?;
    let role = me.role.clone().unwrap_or_default();
    let allowed = if style == "torre" { role == "piloto" || role == "chefe:controller" } else { role == "piloto" || role.starts_with("chefe:") };
    if !allowed {
        return fail("not_allowed", "só o piloto (ou o chefe controller, no estilo torre) delega");
    }
    if request.trim().is_empty() {
        return fail("bad_request", "o pedido não pode ser vazio");
    }
    check_texts(&[request])?;
    let cfg = agentic::load(&s.data);
    let agents: Vec<(String, String)> = catalog::list_agents(&s.data, &home_dir()).into_iter().map(|g| (g.name, g.description)).collect();
    Ok(Gate { me, mission, cfg, agents })
}

/// Asks the Decisor for the type. Its spend is recorded even when the reply is unusable, and then the call fails.
async fn decisor_pick(s: &Arc<AppState>, g: &Gate, request: &str) -> AppResult<Picked> {
    // the vault is opened and read off the async runtime
    let secret = match g.cfg.decisor.secret.clone() {
        Some(n) => {
            let st = s.clone();
            Some(blocking(move || st.secret(&n)?.ok_or_else(|| AppError::new("missing_secret", format!("configure a chave '{n}' no Cofre")))).await?)
        }
        None => None,
    };
    let prices = lk(&s.prices).clone();
    let out = agentic::decide(&g.cfg.decisor, secret, &prices, &g.cfg.types, &g.agents, request).await?;
    let (st, mission_id, caller) = (s.clone(), g.mission.id.clone(), request.to_string());
    let out_c = out.clone();
    blocking(move || {
        agentic::account(&st.db, &out_c)?;
        if out_c.task_type.is_none() {
            st.db.insert_decision(&Decision {
                id: new_id(),
                mission_id: Some(mission_id),
                pane_id: None,
                request: agentic::clean_request(&caller),
                task_type: None,
                cli: None,
                model: Some(out_c.model.clone()),
                effort: None,
                agent: None,
                decided_by: out_c.model.clone(),
                cost_usd: out_c.cost_usd,
                receipt: "Decisor: resposta inválida".into(),
                created_at: now_ms(),
            })?;
        }
        Ok(())
    })
    .await?;
    let Some(id) = out.task_type.clone() else { return fail("decisor_invalid", "o Decisor não devolveu um tipo válido") };
    Ok(Picked { task: type_of(&g.cfg, &id)?.clone(), decided_by: out.model, cost_usd: out.cost_usd, decisor_agent: out.agent })
}

/// The sync part of `delegate`: agent, harness, resume, spawn, and the decision record.
fn delegate_finish(s: &AppState, g: Gate, pick: Picked, request: String, agent_arg: Option<String>) -> AppResult<Value> {
    let Gate { me, mission, cfg, agents, .. } = g;
    let task = pick.task;

    // the agent: the argument, the type's, then (with choose_agent) the Decisor's valid pick or the word match
    let chosen = if let Some(n) = agent_arg {
        Some(n)
    } else if task.agent != "auto" && !task.agent.is_empty() {
        Some(task.agent.clone())
    } else if cfg.choose_agent {
        pick.decisor_agent.filter(|n| agents.iter().any(|(x, _)| x == n)).or_else(|| agentic::auto_agent(&agents, &task, &request))
    } else {
        None
    };
    let agent_def = match &chosen {
        Some(n) => Some(catalog::get_agent(&s.data, &home_dir(), n).ok_or_else(|| AppError::new("unknown_agent", format!("agente desconhecido: {n}")))?),
        None => None,
    };
    let agent_name = agent_def.as_ref().map(|g| g.name.clone());

    let arg = HarnessArgs { cli: agentic::task_cli(&task), model: task.model.clone(), effort: task.effort.clone() };
    let (perfil, _, _) = crate::perfil::efetivo(s, &me.workspace_id, Some(&mission.id))?;
    let slot = agentic::perfil_harness(&perfil, &task);
    // o tipo que segue o perfil: o slot (CLI, modelo e esforço do usuário) vence o arquivo do agente; o esforço próprio
    // do tipo, se houver, continua valendo. Tipo com modelo próprio: o override dele vem antes de tudo.
    let roster = (task.model.is_none() && slot.model.is_some()).then(|| HarnessArgs { effort: task.effort.clone().or(slot.effort.clone()), ..slot.clone() });
    let harness = catalog::resolve_harness(roster.as_ref(), &arg, agent_def.as_ref(), &slot, "claude");
    for v in [Some(harness.cli.as_str()), harness.model.as_deref(), harness.effort.as_deref()].into_iter().flatten() {
        plain_arg(v)?;
    }
    let mut skills = catalog::pane_skills(Some("ajudante"), agent_def.as_ref(), None);
    for sk in &task.skills {
        if !skills.contains(sk) {
            skills.push(sk.clone());
        }
    }

    let live = s.db.mission_panes(&mission.id)?.iter().filter(|p| p.role.as_deref() == Some("ajudante") && p.closed_at.is_none()).count();
    if live >= MAX_LIVE_HELPERS {
        return fail("too_many_helpers", format!("a missão já tem {MAX_LIVE_HELPERS} ajudantes abertos; feche algum antes"));
    }

    // a helper of this type that closed recently, same CLI, with a session, and not alive: resume it.
    // The row is taken in one statement, so two delegates never resume the same session.
    let resumable = lk(&s.providers).clis.iter().any(|c| c.id == harness.cli && !c.session_resume.is_empty());
    let taken = s.db.take_helper_session(&task.id, &mission.id)?;
    let prev = taken.clone().filter(|h| {
        resumable && h.session_id.is_some() && h.cli.as_deref() == Some(harness.cli.as_str()) && now_ms() - h.closed_at < RESUME_WINDOW_MS && !s.panes.is_alive(&h.pane_id)
    });
    let prev_name = match &prev {
        Some(h) => Some(s.db.get_pane(&h.pane_id).map(|r| r.name).unwrap_or_else(|_| h.pane_id.clone())),
        None => None,
    };

    check_spawn_budget(s, &me, 1)?;
    let permission = cap_perm(&me, Some(Permission::Auto))?;
    let spawned = s.spawn_pane(PaneSpec {
        workspace_id: me.workspace_id.clone(),
        mission_id: Some(mission.id.clone()),
        kind: PaneKind::Agent,
        name: Some(auto_name(&request)),
        cli: Some(harness.cli.clone()),
        model: harness.model.clone(),
        effort: harness.effort.clone(),
        account: task.account.clone(),
        permission,
        cwd: me.cwd.clone(),
        prompt: Some(child_prompt(agent_def.as_ref(), &request)),
        parent_id: Some(me.id.clone()),
        role: Some("ajudante".into()),
        agent: agent_name.clone(),
        task_type: Some(task.id.clone()),
        allowed_skills: Some(skills.clone()),
        resume_session: prev.as_ref().and_then(|h| h.session_id.clone()),
        ..Default::default()
    });
    let row = match spawned {
        Ok(r) => r,
        Err(e) => {
            // the session was not used: it goes back for the next delegate
            if let Some(h) = &taken {
                s.db.put_helper_session(h)?;
            }
            return Err(e);
        }
    };
    // schema padrão do tipo (seed/handoff-schemas.json): a entrega do ajudante é validada
    if let Some(schema) = agentic::handoff_schema_padrao(&task.id) {
        crate::orchestrator::fixar_schema(s, &row.id, schema)?;
    }
    if prev.is_none() {
        // a session that was not resumed stays available (only a resumed one is consumed)
        if let Some(h) = &taken {
            s.db.put_helper_session(h)?;
        }
    }

    let model = harness.model.clone().unwrap_or_else(|| "padrão".into());
    let mut receipt = format!("Ajudante \"{}\" para {}: IA {}, modelo {}, agente {}", row.name, task.id, harness.cli, model, agent_name.as_deref().unwrap_or("nenhum"));
    if let Some(n) = &prev_name {
        receipt += &format!("; retomou a sessão do painel {n}");
    }
    receipt += ".";
    s.db.insert_decision(&Decision {
        id: new_id(),
        mission_id: Some(mission.id.clone()),
        pane_id: Some(row.id.clone()),
        request: agentic::clean_request(&request),
        task_type: Some(task.id.clone()),
        cli: Some(harness.cli.clone()),
        model: harness.model.clone(),
        effort: harness.effort.clone(),
        agent: agent_name.clone(),
        decided_by: pick.decided_by,
        cost_usd: pick.cost_usd,
        receipt: receipt.clone(),
        created_at: now_ms(),
    })?;
    Ok(json!({
        "receipt": receipt,
        "pane": {"id": row.id, "name": row.name},
        "taskType": task.id,
        "cli": harness.cli,
        "model": harness.model,
        "effort": harness.effort,
        "agent": agent_name,
        "resumed": prev_name.is_some(),
    }))
}

async fn delegate_run(st: Arc<AppState>, parts: http::request::Parts, a: DelegateArgs) -> AppResult<Value> {
    let (st1, p1, req1) = (st.clone(), parts.clone(), a.request.clone());
    let g = blocking(move || gate(&st1, &p1, &req1)).await?;
    let pick = match a.task_type.as_deref() {
        Some(id) => Picked { task: type_of(&g.cfg, id)?.clone(), decided_by: "piloto".into(), cost_usd: 0.0, decisor_agent: None },
        None if g.cfg.decisor.kind != "none" => decisor_pick(&st, &g, &a.request).await?,
        None => return fail("need_task_type", format!("informe taskType; tipos: {}", type_ids(&g.cfg))),
    };
    let (st2, request, agent) = (st.clone(), a.request, a.agent);
    blocking(move || delegate_finish(&st2, g, pick, request, agent)).await
}

/// The chief's pane for this mission and role, if one is open.
fn open_chief(s: &AppState, mission: &str, role: &str) -> AppResult<Option<PaneRow>> {
    Ok(s.db.mission_panes(mission)?.into_iter().find(|p| p.role.as_deref() == Some(role) && p.closed_at.is_none()))
}

fn chief_run(s: &AppState, parts: &http::request::Parts, a: ChiefArgs) -> AppResult<Value> {
    let me = caller_row(s, parts)?;
    not_archived(s, &me)?;
    let cruzado = a.chief == "cruzado";
    // o revisor cruzado também pode ser chamado pelo chefe reviewer (ele junta a segunda opinião ao veredito)
    let pode = me.role.as_deref() == Some("piloto") || (cruzado && me.role.as_deref() == Some("chefe:reviewer"));
    if !pode {
        return fail("not_allowed", "só o piloto chama os chefes (o revisor cruzado, também o chefe reviewer)");
    }
    let (mission, style) = agentic_mission(s, &me)?;
    if !CHIEFS.contains(&a.chief.as_str()) {
        return fail("unknown_chief", "chefe desconhecido: use scout, builder, reviewer, controller ou cruzado");
    }
    let torre = style == "torre";
    if cruzado {
        let (perfil, _, _) = crate::perfil::efetivo(s, &mission.workspace_id, Some(&mission.id))?;
        if perfil.papeis.cruzado.is_none() {
            return fail("cruzado_off", "a revisão cruzada está desligada no perfil (Configurações → Perfil → Quem faz o quê)");
        }
    } else if torre != (a.chief == "controller") {
        return fail("not_in_style", if torre { "no estilo torre só o controller despacha" } else { "o controller só existe no estilo torre" });
    }
    if a.request.trim().is_empty() {
        return fail("bad_request", "o pedido não pode ser vazio");
    }
    check_texts(&[a.request.as_str()])?;
    let role = format!("chefe:{}", a.chief);
    // one open chief per (mission, role): a second call while the first is opening waits its turn
    let key = (mission.id.clone(), role.clone());
    if !s.rt().chiefs_pending.insert(key.clone()) {
        return fail("chief_busy", "este chefe está sendo aberto; tente de novo");
    }
    let out = chief_open(s, &me, &mission, &role, &a);
    s.rt().chiefs_pending.remove(&key);
    out
}

/// Finds the open chief (and writes the request to it) or spawns one.
fn chief_open(s: &AppState, me: &PaneRow, mission: &Mission, role: &str, a: &ChiefArgs) -> AppResult<Value> {
    if let Some(open) = open_chief(s, &mission.id, role)? {
        s.write_input(&open.id, &bracketed(&a.request))?;
        return Ok(json!({"pane": {"id": open.id, "name": open.name}, "reused": true}));
    }
    let agent = catalog::get_agent(&s.data, &home_dir(), &format!("chefe-{}", a.chief)).ok_or_else(|| AppError::new("unknown_agent", format!("agente do chefe ausente: chefe-{}", a.chief)))?;
    // o papel do chefe no perfil da missão (Configurações → Perfil, ou o assistente da missão) decide CLI, modelo e
    // esforço; o arquivo do agente só vale para o que o perfil não disser
    let (perfil, _, _) = crate::perfil::efetivo(s, &mission.workspace_id, Some(&mission.id))?;
    let papel = crate::perfil::papel_of(&perfil, role).map(crate::perfil::slot_args);
    let harness = catalog::resolve_harness(papel.as_ref(), &HarnessArgs::default(), Some(&agent), &HarnessArgs::default(), "claude");
    for v in [Some(harness.cli.as_str()), harness.model.as_deref(), harness.effort.as_deref()].into_iter().flatten() {
        plain_arg(v)?;
    }
    check_spawn_budget(s, me, 1)?;
    let permission = cap_perm(me, Some(Permission::Auto))?;
    let row = s.spawn_pane(PaneSpec {
        workspace_id: me.workspace_id.clone(),
        mission_id: Some(mission.id.clone()),
        kind: PaneKind::Agent,
        name: Some(agent.name.clone()),
        cli: Some(harness.cli),
        model: harness.model,
        effort: harness.effort,
        permission,
        cwd: me.cwd.clone(),
        prompt: Some(child_prompt(Some(&agent), &a.request)),
        parent_id: Some(me.id.clone()),
        role: Some(role.into()),
        agent: Some(agent.name.clone()),
        ..Default::default()
    })?;
    Ok(json!({"pane": {"id": row.id, "name": row.name}, "reused": false}))
}

#[tool_router(router = agentic_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Delega um pedido a um ajudante do tipo certo. Sem tipo, o Decisor classifica (se configurado). Devolve o recibo.
    #[tool(description = "Delega um pedido a um ajudante do tipo certo (tipo, IA, modelo e agente). Sem taskType, o Decisor classifica. Retoma a sessão de um ajudante do mesmo tipo fechado há menos de 30 min. Devolve o recibo")]
    async fn delegate(&self, Parameters(a): Parameters<DelegateArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "delegate", &parts);
        finish(delegate_run(self.state.clone(), parts, a).await)
    }

    /// Chefe da missão agêntica: manda o pedido ao chefe aberto ou abre um (scout, builder, reviewer, controller).
    #[tool(description = "Chefe da missão agêntica (scout, builder, reviewer ou controller): reaproveita o chefe aberto ou abre um. Só o piloto chama")]
    async fn chief_invoke(&self, Parameters(a): Parameters<ChiefArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "chief_invoke", &parts);
        let st = self.state.clone();
        finish(blocking(move || chief_run(&st, &parts, a)).await)
    }

    /// Decisões da missão: tipo, IA, modelo, quem decidiu, custo e recibo.
    #[tool(description = "Decisões da missão: tipo, IA, modelo, quem decidiu, custo e recibo")]
    async fn decision_list(&self, Parameters(a): Parameters<DecisionListArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "decision_list", &parts);
        let s = &self.state;
        finish((|| -> AppResult<Value> {
            let me = caller_row(s, &parts)?;
            let mission = own_mission(&me)?;
            let limit = a.limit.unwrap_or(20).clamp(1, 200);
            Ok(json!({"decisions": s.db.mission_decisions(&mission, limit)?}))
        })())
    }
}
