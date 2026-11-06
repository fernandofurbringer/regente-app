use super::{cap_perm, caller_row, check_texts, finish, is_self_or_descendant, note, outranks, pane_in_ws, RegenteMcp};
use super::tools_panes::{plain_arg, reachable};
use crate::cronjobs::{self, SpawnCfg};
use crate::db::CronJob;
use crate::error::{fail, AppError, AppResult};
use crate::util::now_ms;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

/// Jobs per workspace created over MCP.
const MCP_CRON_MAX: usize = 20;
/// Minimum gap between two runs of a job created over MCP.
const MCP_MIN_GAP_MS: i64 = 5 * 60 * 1000;

#[derive(Deserialize, JsonSchema)]
pub struct CreateArgs {
    /// Nome do agendamento.
    pub name: String,
    /// Expressão cron de 5 campos (min hora dia mês dia-da-semana; 1 = segunda, 7 = domingo).
    pub expr: String,
    /// Prompt enviado a cada disparo.
    pub prompt: String,
    /// "spawn" (abre um painel novo, padrão) ou o id de um painel do workspace (recebe o prompt).
    pub target: Option<String>,
    /// Com target "spawn": a IA que abre o painel.
    pub spawn: Option<SpawnCfg>,
}

#[derive(Deserialize, JsonSchema)]
pub struct IdArgs {
    /// Id do agendamento.
    pub id: String,
}

/// A job given as an argument: it must be in the caller's workspace.
fn job_in_ws(s: &crate::runtime::AppState, ws: &str, id: &str) -> AppResult<CronJob> {
    match s.db.cron_jobs(Some(ws))?.into_iter().find(|j| j.id == id) {
        Some(j) => Ok(j),
        None => fail("not_found", "agendamento não existe neste workspace"),
    }
}

#[tool_router(router = cron_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Cria um agendamento no workspace: uma expressão cron e um prompt, enviados a um painel novo ou a um painel existente.
    #[tool(description = "Cria um agendamento no workspace: expressão cron de 5 campos e prompt, enviado a um painel novo (spawn) ou a um painel existente")]
    async fn cron_create(&self, Parameters(a): Parameters<CreateArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "cron_create", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            check_texts(&[a.name.as_str(), a.prompt.as_str()])?;
            if s.db.cron_jobs(Some(&me.workspace_id))?.len() >= MCP_CRON_MAX {
                return fail("too_many", format!("limite de {MCP_CRON_MAX} agendamentos por workspace"));
            }
            // at least 5 minutes between the next two runs
            let runs = cronjobs::preview(&a.expr, now_ms())?;
            if runs.len() >= 2 && runs[1] - runs[0] < MCP_MIN_GAP_MS {
                return fail("too_frequent", "intervalo mínimo de 5 minutos entre execuções");
            }
            let spawn_json = match a.target.as_deref() {
                None | Some("spawn") => {
                    let mut cfg = a.spawn.clone().ok_or_else(|| AppError::new("bad_cron", "target spawn precisa de spawn: {cli, model?, effort?}"))?;
                    plain_arg(&cfg.cli)?;
                    for v in [&cfg.model, &cfg.effort].into_iter().flatten() {
                        plain_arg(v)?;
                    }
                    // the job's pane gets the caller's permission (never above it) and the caller as parent, whatever the input said
                    cfg.permission = Some(cap_perm(&me, None)?.as_str().into());
                    cfg.parent_id = Some(me.id.clone());
                    Some(serde_json::to_string(&cfg).unwrap_or_default())
                }
                Some(pane) => {
                    let row = pane_in_ws(s, &me.workspace_id, pane)?;
                    if !is_self_or_descendant(s, &me.id, pane) {
                        return fail("forbidden", "só o próprio painel ou os seus filhos podem receber o prompt");
                    }
                    reachable(s, &me, &row, true)?;
                    if outranks(&row, &me) {
                        return fail("forbidden", "o painel alvo tem permissão acima da sua");
                    }
                    None
                }
            };
            let target = match a.target.as_deref() {
                None | Some("spawn") => "spawn".to_string(),
                Some(pane) => format!("pane:{pane}"),
            };
            let job = cronjobs::save_from_mcp(
                s,
                CronJob {
                    id: String::new(),
                    workspace_id: me.workspace_id.clone(),
                    name: a.name.clone(),
                    expr: a.expr.clone(),
                    prompt: a.prompt.clone(),
                    target,
                    spawn_json,
                    enabled: true,
                    last_run: None,
                    next_run: None,
                    last_error: None,
                    last_pane: None,
                },
            )?;
            Ok(json!({"id": job.id, "nextRun": job.next_run}))
        })())
    }

    /// Agendamentos do workspace.
    #[tool(description = "Agendamentos do workspace, com a próxima execução e o último erro")]
    async fn cron_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "cron_list", &parts);
        finish((|| -> AppResult<Value> {
            let me = caller_row(&self.state, &parts)?;
            Ok(json!({"jobs": self.state.db.cron_jobs(Some(&me.workspace_id))?}))
        })())
    }

    /// Apaga um agendamento do workspace.
    #[tool(description = "Apaga um agendamento do workspace")]
    async fn cron_delete(&self, Parameters(a): Parameters<IdArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "cron_delete", &parts);
        finish((|| -> AppResult<Value> {
            let me = caller_row(&self.state, &parts)?;
            job_in_ws(&self.state, &me.workspace_id, &a.id)?;
            self.state.db.delete_cron(&a.id)?;
            Ok(json!({"ok": true}))
        })())
    }

    /// Dispara um agendamento do workspace agora, sem mudar a agenda.
    #[tool(description = "Dispara um agendamento do workspace agora, sem mudar a agenda")]
    async fn cron_run_now(&self, Parameters(a): Parameters<IdArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "cron_run_now", &parts);
        finish((|| -> AppResult<Value> {
            let me = caller_row(&self.state, &parts)?;
            job_in_ws(&self.state, &me.workspace_id, &a.id)?;
            cronjobs::run_now(&self.state, &a.id)?;
            Ok(json!({"ok": true, "at": now_ms()}))
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PaneRow;
    use crate::mcp::CallerPane;
    use crate::orchestrator::bracketed;
    use crate::runtime::AppState;
    use crate::status::PaneStatus;
    use std::sync::Arc;

    fn state(dir: &std::path::Path) -> Arc<AppState> {
        AppState::new(dir.into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: serde_json::Value| {})).unwrap()
    }

    fn pane(id: &str, ws: &str, parent: Option<&str>) -> PaneRow {
        PaneRow { id: id.into(), workspace_id: ws.into(), kind: "agent".into(), name: id.into(), permission: "pend".into(), cwd: "C:/".into(), status: "ocioso".into(), parent_id: parent.map(Into::into), ..Default::default() }
    }

    /// Request parts as the auth middleware leaves them: the caller pane in the extensions.
    fn as_pane(id: &str) -> http::request::Parts {
        let mut parts = http::Request::new(()).into_parts().0;
        parts.extensions.insert(CallerPane(id.into()));
        parts
    }

    fn create(m: &RegenteMcp, caller: &str, expr: &str, target: Option<&str>, spawn: bool) -> CallToolResult {
        let args = CreateArgs {
            name: "resumo".into(),
            expr: expr.into(),
            prompt: "resumo do dia".into(),
            target: target.map(Into::into),
            spawn: spawn.then(|| SpawnCfg { cli: "claude".into(), ..Default::default() }),
        };
        futures_block(m.cron_create(Parameters(args), Extension(as_pane(caller))))
    }

    /// Runs an async tool on a small runtime (the tests are plain #[test]).
    fn futures_block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    fn text(r: &CallToolResult) -> String {
        r.content[0].as_text().unwrap().text.clone()
    }

    #[test]
    fn pane_target_gets_the_prompt_on_tick() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        s.db.insert_pane(&pane("c", "w", None)).unwrap();
        s.db.insert_pane(&pane("t", "w", Some("c"))).unwrap();
        s.rt().statuses.insert("t".into(), PaneStatus::Hibernated);
        let m = RegenteMcp::new(s.clone());
        let r = create(&m, "c", "0 9 * * *", Some("t"), false);
        assert_ne!(r.is_error, Some(true), "{}", text(&r));
        let id = serde_json::from_str::<Value>(&text(&r)).unwrap()["id"].as_str().unwrap().to_string();
        let job = s.db.cron_jobs(Some("w")).unwrap().into_iter().find(|j| j.id == id).unwrap();
        assert_eq!(job.target, "pane:t");
        s.db.update_cron_run(&id, 0, None, Some(now_ms() - 60_000)).unwrap();
        crate::cronjobs::tick(&s);
        assert_eq!(s.rt().pending_input.get("t").cloned().unwrap_or_default(), bracketed("resumo do dia"));
    }

    #[test]
    fn pane_target_must_be_the_caller_or_its_descendant() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        s.db.insert_pane(&pane("c", "w", None)).unwrap();
        s.db.insert_pane(&pane("x", "w", None)).unwrap();
        let m = RegenteMcp::new(s.clone());
        let r = create(&m, "c", "0 9 * * *", Some("x"), false);
        assert!(text(&r).contains("só o próprio painel"), "{}", text(&r));
        assert!(s.db.cron_jobs(None).unwrap().is_empty());
    }

    #[test]
    fn expressions_closer_than_five_minutes_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        s.db.insert_pane(&pane("c", "w", None)).unwrap();
        let m = RegenteMcp::new(s.clone());
        let r = create(&m, "c", "* * * * *", None, true);
        assert!(text(&r).contains("5 minutos"), "{}", text(&r));
    }

    #[test]
    fn twenty_first_job_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        s.db.insert_pane(&pane("c", "w", None)).unwrap();
        for i in 0..20 {
            let j = CronJob { id: format!("j{i}"), workspace_id: "w".into(), name: "j".into(), expr: "0 9 * * *".into(), prompt: "p".into(), target: "spawn".into(), spawn_json: None, enabled: true, last_run: None, next_run: None, last_error: None, last_pane: None };
            s.db.upsert_cron(&j).unwrap();
        }
        let m = RegenteMcp::new(s.clone());
        let r = create(&m, "c", "0 9 * * *", None, true);
        assert!(text(&r).contains("limite de 20"), "{}", text(&r));
    }

    #[test]
    fn foreign_workspace_jobs_are_not_found() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        s.db.insert_pane(&pane("c", "w", None)).unwrap();
        let foreign = CronJob { id: "f".into(), workspace_id: "other".into(), name: "f".into(), expr: "0 9 * * *".into(), prompt: "p".into(), target: "spawn".into(), spawn_json: None, enabled: true, last_run: None, next_run: None, last_error: None, last_pane: None };
        s.db.upsert_cron(&foreign).unwrap();
        let m = RegenteMcp::new(s.clone());
        let del = futures_block(m.cron_delete(Parameters(IdArgs { id: "f".into() }), Extension(as_pane("c"))));
        assert!(text(&del).contains("neste workspace"), "{}", text(&del));
        let run = futures_block(m.cron_run_now(Parameters(IdArgs { id: "f".into() }), Extension(as_pane("c"))));
        assert!(text(&run).contains("neste workspace"), "{}", text(&run));
        assert_eq!(s.db.cron_jobs(None).unwrap().len(), 1, "the foreign job survives");
    }

    /// A fake CLI that runs as cmd.exe, for jobs that really open a pane.
    fn fake_cli(s: &AppState) {
        use crate::util::lk;
        let args = vec!["/k".to_string(), "rem {session}".to_string()];
        lk(&s.providers).clis.push(crate::providers::CliDef { id: "fake".into(), label: "Fake".into(), command: "cmd.exe".into(), session_new: args.clone(), session_resume: args, ..Default::default() });
    }

    /// Creates a spawn job as the pane `caller` and returns its id.
    fn spawn_job_as(m: &RegenteMcp, caller: &str) -> String {
        let args = CreateArgs { name: "resumo".into(), expr: "0 9 * * *".into(), prompt: "resumo".into(), target: None, spawn: Some(SpawnCfg { cli: "fake".into(), ..Default::default() }) };
        let r = futures_block(m.cron_create(Parameters(args), Extension(as_pane(caller))));
        assert_ne!(r.is_error, Some(true), "{}", text(&r));
        serde_json::from_str::<Value>(&text(&r)).unwrap()["id"].as_str().unwrap().to_string()
    }

    /// The pane a job opened under `parent`, if any.
    fn spawned_under(s: &AppState, ws: &str, parent: &str) -> Option<PaneRow> {
        s.db.open_panes(ws).unwrap().into_iter().find(|p| p.parent_id.as_deref() == Some(parent))
    }

    #[test]
    fn plan_callers_spawn_job_opens_a_plan_pane_under_it() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        fake_cli(&s);
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        s.db.insert_pane(&PaneRow { id: "c".into(), workspace_id: ws.id.clone(), kind: "agent".into(), name: "c".into(), permission: "plan".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), ..Default::default() }).unwrap();
        let m = RegenteMcp::new(s.clone());
        let job = spawn_job_as(&m, "c");
        crate::cronjobs::run_now(&s, &job).unwrap();
        let pane = spawned_under(&s, &ws.id, "c").expect("the job opened a pane under the caller");
        assert_eq!(pane.permission, "plan", "never above the caller");
        s.shutdown();
    }

    #[test]
    fn a_helpers_cron_pane_has_a_parent_and_is_not_top() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        fake_cli(&s);
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        s.db.insert_pane(&PaneRow { id: "c".into(), workspace_id: ws.id.clone(), kind: "agent".into(), name: "c".into(), permission: "pend".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), parent_id: Some("p".into()), role: Some("ajudante".into()), ..Default::default() }).unwrap();
        let m = RegenteMcp::new(s.clone());
        let job = spawn_job_as(&m, "c");
        crate::cronjobs::run_now(&s, &job).unwrap();
        let pane = spawned_under(&s, &ws.id, "c").expect("the job opened a pane under the helper");
        assert!(crate::mcp::require_top(&pane).is_err(), "a cron pane of a helper is not top");
        s.shutdown();
    }
}
