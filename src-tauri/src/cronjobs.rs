//! Agendamentos: job com expressão cron. Só roda com o app aberto. A thread `regente-cron` verifica a cada 30 s.
use crate::db::CronJob;
use crate::error::{fail, AppError, AppResult};
use crate::orchestrator::bracketed;
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::status::PaneStatus;
use crate::util::{new_id, now_ms};
use chrono::{Local, TimeZone};
use cron::Schedule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// What a `spawn` job opens (stored as `spawn_json`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SpawnCfg {
    /// CLI de IA da lista de provedores (ex.: claude, codex, gemini).
    pub cli: String,
    /// Modelo (opcional).
    pub model: Option<String>,
    /// Esforço de raciocínio (opcional).
    pub effort: Option<String>,
    /// Permission of the pane the job opens: set by `cron_create` (the caller's, never above it). None = pend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// The caller pane that created the job: the job's pane is its child. Set by `cron_create` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

fn bad_cron() -> AppError {
    AppError::new("bad_cron", "expressão cron inválida")
}

/// Standard weekday field (0 and 7 = domingo, 1 = segunda … 6 = sábado; lists, ranges and steps; no names)
/// to the crate's (1 = domingo … 7 = sábado), as a sorted, deduplicated list. None if it does not parse.
pub fn crate_weekdays(f: &str) -> Option<String> {
    let mut set = std::collections::BTreeSet::new();
    for item in f.split(',') {
        let (r, step) = item.split_once('/').map_or((item, 1), |(r, s)| (r, s.parse().unwrap_or(0)));
        let (a, b) = match r {
            "*" => (0, 6),
            _ => match r.split_once('-') {
                Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
                None => {
                    let a: u32 = r.parse().ok()?;
                    (a, if item.contains('/') { 7 } else { a })
                }
            },
        };
        if step == 0 || a > b || b > 7 {
            return None;
        }
        set.extend((a..=b).step_by(step as usize).map(|d| d % 7 + 1));
    }
    Some(set.into_iter().map(|d| d.to_string()).collect::<Vec<_>>().join(","))
}

/// The expression as the crate reads it: five standard fields get the seconds field, and the weekday field of
/// five-, six- or seven-field expressions is converted to the crate's numbering. Anything else is `bad_cron`.
pub fn crate_expr(expr: &str) -> AppResult<String> {
    let mut parts: Vec<String> = expr.split_whitespace().map(str::to_string).collect();
    let dow_at = match parts.len() {
        5 => {
            parts.insert(0, "0".into());
            5
        }
        6 | 7 => 5,
        _ => return Err(bad_cron()),
    };
    parts[dow_at] = crate_weekdays(&parts[dow_at]).ok_or_else(bad_cron)?;
    Ok(parts.join(" "))
}

fn to_schedule(expr: &str) -> AppResult<Schedule> {
    Schedule::from_str(&crate_expr(expr)?).map_err(|_| bad_cron())
}

/// The next run strictly after `after_ms` (local time zone).
pub fn next_run(expr: &str, after_ms: i64) -> AppResult<i64> {
    let sched = to_schedule(expr)?;
    let after = Local.timestamp_millis_opt(after_ms).single().ok_or_else(|| AppError::new("bad_cron", "hora inválida"))?;
    sched.after(&after).next().map(|t| t.timestamp_millis()).ok_or_else(|| AppError::new("bad_cron", "a expressão nunca dispara"))
}

/// Simple UI frequencies to a standard expression (weekdays 1 = segunda … 7 = domingo).
pub fn expr_from_preset(kind: &str, hour: u32, minute: u32, days: &[u32], month_day: u32) -> AppResult<String> {
    if hour > 23 || minute > 59 {
        return fail("bad_cron", "hora ou minuto fora do intervalo");
    }
    match kind {
        "daily" => Ok(format!("{minute} {hour} * * *")),
        "weekly" => {
            if days.is_empty() || days.iter().any(|d| !(1..=7).contains(d)) {
                return fail("bad_cron", "dias da semana: 1 (segunda) a 7 (domingo)");
            }
            let list = days.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            Ok(format!("{minute} {hour} * * {list}"))
        }
        "monthly" => {
            if !(1..=31).contains(&month_day) {
                return fail("bad_cron", "dia do mês: 1 a 31");
            }
            Ok(format!("{minute} {hour} {month_day} * *"))
        }
        _ => fail("bad_cron", "frequência desconhecida"),
    }
}

/// Next `n` runs after `after_ms`, for the UI preview.
pub fn preview(expr: &str, after_ms: i64) -> AppResult<Vec<i64>> {
    let sched = to_schedule(expr)?;
    let after = Local.timestamp_millis_opt(after_ms).single().ok_or_else(|| AppError::new("bad_cron", "hora inválida"))?;
    Ok(sched.after(&after).take(3).map(|t| t.timestamp_millis()).collect())
}

/// Drops the lineage keys (`permission`, `parentId`) from a spawn config: a UI job never carries them.
fn without_lineage(spawn_json: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(spawn_json) else { return spawn_json.to_string() };
    if let Some(o) = v.as_object_mut() {
        o.remove("permission");
        o.remove("parentId");
    }
    v.to_string()
}

/// Validates and stores a job from the UI: its spawn config has no permission or parent (those are the caller's, over MCP).
pub fn save(s: &AppState, mut job: CronJob) -> AppResult<CronJob> {
    job.spawn_json = job.spawn_json.as_deref().map(without_lineage);
    store(s, job)
}

/// Stores a job an MCP caller created: its spawn config carries the caller's permission and parent, set by `cron_create`.
pub fn save_from_mcp(s: &AppState, job: CronJob) -> AppResult<CronJob> {
    store(s, job)
}

fn store(s: &AppState, mut job: CronJob) -> AppResult<CronJob> {
    if job.id.is_empty() {
        job.id = new_id();
    }
    if job.name.trim().is_empty() {
        return fail("bad_name", "o agendamento precisa de um nome");
    }
    job.next_run = Some(next_run(&job.expr, now_ms())?);
    s.db.upsert_cron(&job)?;
    Ok(job)
}

/// The job's last pane is still usable: its row is open and its process runs (or it is hibernated, and a write wakes it).
fn reusable(s: &AppState, pane: &str) -> bool {
    s.db.get_pane(pane).is_ok_and(|r| r.closed_at.is_none()) && (s.panes.is_alive(pane) || s.status_of(pane) == Some(PaneStatus::Hibernated))
}

/// Runs the job's action. `Some(reason)` when the run is skipped; the reason becomes the job's last error.
fn fire(s: &AppState, job: &CronJob) -> AppResult<Option<&'static str>> {
    if let Some(pane) = job.target.strip_prefix("pane:") {
        match s.db.get_pane(pane) {
            Ok(row) if row.closed_at.is_none() => {}
            _ => return fail("gone", "painel não existe mais"),
        }
        s.write_input(pane, &bracketed(&job.prompt))?;
        return Ok(None);
    }
    // a spawn job keeps writing into the pane it opened, while that pane is usable
    if let Some(prev) = job.last_pane.as_deref().filter(|p| reusable(s, p)) {
        if s.panes.typed_since_enter(prev) {
            return Ok(Some("pulado: usuário digitando no painel"));
        }
        s.write_input(prev, &bracketed(&job.prompt))?;
        return Ok(None);
    }
    let cfg: SpawnCfg = job.spawn_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).ok_or_else(|| AppError::new("bad_cron", "alvo spawn sem configuração de IA"))?;
    let ws = s.db.get_workspace(&job.workspace_id)?;
    // UI jobs carry no permission and no parent: pend, top level
    let permission = cfg.permission.as_deref().and_then(|p| serde_json::from_value::<Permission>(json!(p)).ok()).unwrap_or(Permission::Pend);
    let row = s.spawn_pane(PaneSpec {
        workspace_id: ws.id.clone(),
        kind: PaneKind::Agent,
        name: Some(job.name.clone()),
        cli: Some(cfg.cli),
        model: cfg.model,
        effort: cfg.effort,
        permission,
        parent_id: cfg.parent_id.clone(),
        cwd: ws.path.clone(),
        prompt: Some(job.prompt.clone()),
        ..Default::default()
    })?;
    s.db.set_cron_last_pane(&job.id, &row.id)?;
    Ok(None)
}

/// Fires a job and records the outcome. Only the run columns are written (a job deleted meanwhile stays deleted).
fn run_and_record(s: &AppState, job: &CronJob, now: i64, next: Option<i64>) {
    let (ok, err) = match fire(s, job) {
        Ok(None) => (true, None),
        Ok(Some(reason)) => (true, Some(reason.to_string())),
        Err(e) => (false, Some(e.to_string())),
    };
    if let Err(e) = s.db.update_cron_run(&job.id, now, err.as_deref(), next) {
        tracing::warn!("cron {}: falha ao gravar: {e}", job.id);
    }
    s.emit("cron-ran", json!({"id": job.id, "ok": ok, "workspaceId": job.workspace_id}));
}

/// Fires every job that is due. A job whose run was missed while the app was closed fires once (not a burst):
/// its next run is computed from now, not from the missed time.
pub fn tick(s: &AppState) {
    let now = now_ms();
    let due = match s.db.cron_jobs(None) {
        Ok(jobs) => jobs.into_iter().filter(|j| j.enabled && j.next_run.is_some_and(|t| t <= now)).collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!("cron: falha ao listar: {e}");
            return;
        }
    };
    for job in due {
        run_and_record(s, &job, now, next_run(&job.expr, now).ok());
    }
}

/// "Executar agora": fires the job without changing its schedule.
pub fn run_now(s: &AppState, id: &str) -> AppResult<()> {
    let job = s.db.cron_jobs(None)?.into_iter().find(|j| j.id == id);
    let Some(job) = job else { return fail("not_found", "agendamento não existe") };
    run_and_record(s, &job, now_ms(), job.next_run);
    Ok(())
}

/// The cron thread: every 30 s, the cron jobs and automatic hibernation (which kills a process, so it is not
/// on the 500 ms ticker). A panic in one tick is logged and does not end the thread.
pub fn start(s: Arc<AppState>) {
    fn guarded(name: &str, f: impl FnOnce()) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
            tracing::error!("panic no tick {name}");
        }
    }
    std::thread::Builder::new()
        .name("regente-cron".into())
        .spawn(move || {
            let mut ticks: u32 = 0;
            while !s.rt().shutting_down {
                std::thread::sleep(Duration::from_millis(500));
                ticks += 1;
                if ticks == 60 {
                    ticks = 0;
                    guarded("cron", || tick(&s));
                    guarded("hibernate", || crate::hibernate::tick(&s));
                }
            }
        })
        .expect("cron thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_ms(h: u32, m: u32) -> i64 {
        Local.with_ymd_and_hms(2026, 10, 15, h, m, 0).unwrap().timestamp_millis()
    }

    #[test]
    fn five_fields_get_seconds_and_standard_weekdays() {
        assert_eq!(crate_expr("30 9 * * 1-5").unwrap(), "0 30 9 * * 2,3,4,5,6");
        assert_eq!(crate_expr("0 0 9 * * * *").unwrap(), "0 0 9 * * 1,2,3,4,5,6,7 *");
        assert_eq!(crate_expr("0 0 9 * * 0 *").unwrap(), "0 0 9 * * 1 *");
        assert_eq!(crate_expr("x").unwrap_err().code(), "bad_cron");
        assert_eq!(crate_expr("0 9 * * MON").unwrap_err().code(), "bad_cron");
    }

    #[test]
    fn weekday_sets() {
        assert_eq!(crate_weekdays("1-7").unwrap(), "1,2,3,4,5,6,7");
        assert_eq!(crate_weekdays("5-7").unwrap(), "1,6,7");
        assert_eq!(crate_weekdays("0").unwrap(), "1");
        assert_eq!(crate_weekdays("7").unwrap(), "1");
        assert_eq!(crate_weekdays("1,3,7").unwrap(), "1,2,4");
        assert_eq!(crate_weekdays("*/2").unwrap(), "1,3,5,7");
        assert_eq!(crate_weekdays("1-5/2").unwrap(), "2,4,6");
        assert_eq!(crate_weekdays("MON"), None);
        assert_eq!(crate_weekdays("5-1"), None);
        assert_eq!(crate_weekdays("*/0"), None);
    }

    #[test]
    fn next_run_is_today_at_nine() {
        let after = local_ms(8, 0);
        assert_eq!(next_run("0 9 * * *", after).unwrap(), local_ms(9, 0));
    }

    #[test]
    fn presets_use_standard_weekdays() {
        assert_eq!(expr_from_preset("weekly", 9, 30, &[1, 3, 5], 0).unwrap(), "30 9 * * 1,3,5");
        assert_eq!(expr_from_preset("daily", 7, 5, &[], 0).unwrap(), "5 7 * * *");
        assert_eq!(expr_from_preset("monthly", 0, 0, &[], 31).unwrap(), "0 0 31 * *");
    }

    #[test]
    fn standard_weekdays_fire_on_the_right_days() {
        // 2026-10-15 is a Thursday: "1-5" (segunda a sexta) fires on Friday 16 at 09:30, not on Sunday
        let after = local_ms(10, 0);
        assert_eq!(next_run("30 9 * * 1-5", after).unwrap(), Local.with_ymd_and_hms(2026, 10, 16, 9, 30, 0).unwrap().timestamp_millis());
        // domingo é 0 ou 7
        let sat = Local.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap().timestamp_millis();
        assert_eq!(next_run("0 9 * * 0", sat).unwrap(), Local.with_ymd_and_hms(2026, 10, 18, 9, 0, 0).unwrap().timestamp_millis());
    }

    fn state(dir: &std::path::Path) -> Arc<AppState> {
        AppState::new(dir.into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: serde_json::Value| {})).unwrap()
    }

    fn job(id: &str, target: &str, next: Option<i64>) -> CronJob {
        CronJob { id: id.into(), workspace_id: "w".into(), name: id.into(), expr: "0 9 * * *".into(), prompt: "resumo".into(), target: target.into(), spawn_json: None, enabled: true, last_run: None, next_run: next, last_error: None, last_pane: None }
    }

    #[test]
    fn tick_writes_to_hibernated_pane_and_moves_next_run() {
        use crate::db::PaneRow;
        use crate::status::PaneStatus;
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        s.db.insert_pane(&PaneRow { id: "p".into(), workspace_id: "w".into(), kind: "agent".into(), name: "p".into(), permission: "pend".into(), cwd: "C:/".into(), status: "hibernado".into(), ..Default::default() }).unwrap();
        s.rt().statuses.insert("p".into(), PaneStatus::Hibernated);
        let past = now_ms() - 60_000;
        s.db.upsert_cron(&job("j", "pane:p", Some(past))).unwrap();
        tick(&s);
        let pending = s.rt().pending_input.get("p").cloned().unwrap_or_default();
        assert_eq!(pending, bracketed("resumo"));
        let after = s.db.cron_jobs(None).unwrap().remove(0);
        assert!(after.next_run.unwrap() > now_ms(), "next run moved forward");
        assert_eq!(after.last_error, None);
        assert!(after.last_run.is_some());
    }

    #[test]
    fn run_record_does_not_resurrect_a_deleted_job() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let j = job("gone", "pane:nope", None);
        s.db.upsert_cron(&j).unwrap();
        // deleted while the run was in flight
        s.db.delete_cron("gone").unwrap();
        run_and_record(&s, &j, now_ms(), Some(now_ms() + 1000));
        assert!(s.db.cron_jobs(None).unwrap().is_empty());
    }

    #[test]
    fn run_record_changes_only_run_columns() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let j = job("k", "pane:nope", None);
        s.db.upsert_cron(&j).unwrap();
        run_and_record(&s, &j, 5, Some(7));
        let got = s.db.cron_jobs(None).unwrap().remove(0);
        assert_eq!((got.name.as_str(), got.expr.as_str(), got.prompt.as_str()), ("k", "0 9 * * *", "resumo"));
        assert_eq!((got.last_run, got.next_run), (Some(5), Some(7)));
        assert!(got.last_error.is_some(), "the missing pane is recorded");
    }


    /// A spawn job with a live CLI (cmd.exe under a fake id) in a real workspace.
    fn spawn_state(d: &std::path::Path) -> (Arc<AppState>, String) {
        use crate::providers::CliDef;
        use crate::util::lk;
        let s = state(d);
        let args = vec!["/k".to_string(), "rem {session}".to_string()];
        lk(&s.providers).clis.push(CliDef { id: "fake".into(), label: "Fake".into(), command: "cmd.exe".into(), session_new: args.clone(), session_resume: args, ..Default::default() });
        let ws = s.db.upsert_workspace(&d.to_string_lossy()).unwrap();
        let mut j = job("sp", "spawn", None);
        j.workspace_id = ws.id.clone();
        j.spawn_json = Some(r#"{"cli":"fake"}"#.into());
        s.db.upsert_cron(&j).unwrap();
        (s, ws.id)
    }

    #[test]
    fn spawn_job_writes_into_its_last_pane_on_the_next_run() {
        let d = tempfile::tempdir().unwrap();
        let (s, ws) = spawn_state(d.path());
        run_now(&s, "sp").unwrap();
        let first = s.db.cron_jobs(None).unwrap().remove(0);
        assert_eq!(first.last_error, None);
        let pane = first.last_pane.clone().expect("the run stored the pane it opened");
        run_now(&s, "sp").unwrap();
        let got = s.db.cron_jobs(None).unwrap().remove(0);
        assert_eq!(got.last_error, None, "the second run writes into the pane, it does not skip");
        assert_eq!(got.last_pane.as_deref(), Some(pane.as_str()));
        assert_eq!(s.db.open_panes(&ws).unwrap().len(), 1, "no second pane was opened");
        s.shutdown();
    }

    #[test]
    fn spawn_job_reuses_the_persisted_pane_without_in_memory_state() {
        let d = tempfile::tempdir().unwrap();
        let (s, ws) = spawn_state(d.path());
        run_now(&s, "sp").unwrap();
        let pane = s.db.cron_jobs(None).unwrap().remove(0).last_pane.unwrap();
        // a restart forgets the runtime maps; the pane's process and the job's last_pane are what is left
        s.rt().statuses.clear();
        run_now(&s, "sp").unwrap();
        assert_eq!(s.db.cron_jobs(None).unwrap().remove(0).last_pane.as_deref(), Some(pane.as_str()));
        assert_eq!(s.db.open_panes(&ws).unwrap().len(), 1);
        s.shutdown();
    }

    #[test]
    fn spawn_job_waits_while_the_user_types_in_its_pane() {
        let d = tempfile::tempdir().unwrap();
        let (s, _ws) = spawn_state(d.path());
        run_now(&s, "sp").unwrap();
        let pane = s.db.cron_jobs(None).unwrap().remove(0).last_pane.unwrap();
        s.panes.write(&pane, b"x").unwrap(); // typed text, no Enter yet
        run_now(&s, "sp").unwrap();
        let got = s.db.cron_jobs(None).unwrap().remove(0);
        assert_eq!(got.last_error.as_deref(), Some("pulado: usuário digitando no painel"));
        s.shutdown();
    }
    #[test]
    fn ui_jobs_cannot_carry_lineage() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let mut j = job("ui", "spawn", None);
        j.spawn_json = Some(r#"{"cli":"fake","permission":"danger","parentId":"x"}"#.into());
        save(&s, j).unwrap();
        let got = s.db.cron_jobs(None).unwrap().remove(0);
        let cfg: SpawnCfg = serde_json::from_str(got.spawn_json.as_deref().unwrap()).unwrap();
        assert_eq!((cfg.cli.as_str(), cfg.permission, cfg.parent_id), ("fake", None, None));
    }

    #[test]
    fn preview_lists_three_runs() {
        let after = local_ms(8, 0);
        let runs = preview("0 9 * * *", after).unwrap();
        assert_eq!(runs, vec![local_ms(9, 0), local_ms(9, 0) + 86_400_000, local_ms(9, 0) + 2 * 86_400_000]);
    }
}
