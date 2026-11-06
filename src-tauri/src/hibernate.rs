//! Hibernação automática (experimental): hiberna, com o `hibernate()` comum, o painel Claude ocioso há mais tempo.
//! Só roda com a flag ligada. Cada motivo que impede a hibernação vem explicado em pt-BR ("Por que não hiberna").
use crate::db::PaneRow;
use crate::error::{fail, AppResult};
use crate::orchestrator::{last_lines, strip_ansi};
use crate::runtime::AppState;
use crate::status::PaneStatus;
use crate::util::lk;
use std::time::{Duration, Instant};

/// A woken pane is not hibernated again for this long.
pub const WAKE_RESIDENCE: Duration = Duration::from_secs(5 * 60);
/// Lines of the pane kept as its frozen frame.
const FRAME_LINES: usize = 60;

pub struct Candidate<'a> {
    pub id: &'a str,
    pub cli: Option<&'a str>,
    pub resident: bool,
    pub focused: bool,
    pub has_parent: bool,
    pub live_mission: bool,
    pub status: PaneStatus,
    pub idle_for: Duration,
    pub has_session: bool,
    /// The user is typing in the pane (`typed_since_enter`).
    pub typed: bool,
    /// Woken less than `WAKE_RESIDENCE` ago.
    pub woke_recently: bool,
    /// Workspace remoto: the tmux session would keep the CLI running.
    pub remote: bool,
}

pub struct Ctx {
    pub enabled: bool,
    pub idle_minutes: u64,
    pub resident_floor: usize,
    pub live_claude: usize,
    /// Another pane is hibernating right now (its process is still going down).
    pub in_flight: bool,
}

/// Reasons in pt-BR; empty = may hibernate. Order: disabled, not Claude, resident, focused, has a caller,
/// live mission, not idle, idle for less than N min, no session, at the floor of N residents, another hibernation in progress.
pub fn hibernate_blockers(c: &Candidate, ctx: &Ctx, manual: bool) -> Vec<String> {
    let mut r = vec![];
    if !ctx.enabled && !manual {
        r.push("hibernação automática desligada".into());
    }
    if c.cli != Some("claude") {
        r.push("só o Claude hiberna automaticamente".into());
    }
    if c.resident {
        r.push("painel residente (fixado pelo usuário)".into());
    }
    if c.focused {
        r.push("painel em foco".into());
    }
    if c.has_parent {
        r.push("tem um painel chamador".into());
    }
    if c.live_mission {
        r.push("faz parte de uma missão em andamento".into());
    }
    if c.typed {
        r.push("você está digitando neste painel".into());
    }
    if c.status != PaneStatus::Idle {
        r.push("não está ocioso".into());
    }
    if !manual && c.idle_for < Duration::from_secs(ctx.idle_minutes * 60) {
        r.push(format!("ocioso há menos de {} min", ctx.idle_minutes));
    }
    if !c.has_session {
        r.push("sem sessão para retomar".into());
    }
    if ctx.live_claude <= ctx.resident_floor {
        r.push(format!("no piso de {} painéis Claude abertos", ctx.resident_floor));
    }
    if ctx.in_flight {
        r.push("outra hibernação em andamento".into());
    }
    if c.woke_recently {
        r.push("acordou há menos de 5 min".into());
    }
    if c.remote {
        r.push("painel remoto não hiberna".into());
    }
    r
}

/// Among the panes with no blockers, picks the one with the oldest last input.
pub fn pick<'a>(cands: &[(Candidate<'a>, Instant)], ctx: &Ctx) -> Option<&'a str> {
    cands.iter().filter(|(c, _)| hibernate_blockers(c, ctx, false).is_empty()).min_by_key(|(_, last)| *last).map(|(c, _)| c.id)
}

/// A live pane as the decision needs it (owned data behind a `Candidate`).
struct Info {
    row: PaneRow,
    status: PaneStatus,
    last_input: Instant,
    idle_for: Duration,
    live_mission: bool,
    focused: bool,
    woke_recently: bool,
    typed: bool,
}

fn candidate(i: &Info) -> Candidate<'_> {
    Candidate {
        id: &i.row.id,
        cli: i.row.cli.as_deref(),
        resident: i.row.resident,
        focused: i.focused,
        has_parent: i.row.parent_id.is_some(),
        live_mission: i.live_mission,
        status: i.status,
        idle_for: i.idle_for,
        has_session: i.row.session_id.is_some(),
        typed: i.typed,
        woke_recently: i.woke_recently,
        remote: crate::remote::is_remote(&i.row.cwd),
    }
}

/// Every pane with a live process, and the context shared by all of them.
fn snapshot(s: &AppState, now: Instant) -> (Vec<Info>, Ctx) {
    let (focused, woke_at, statuses) = {
        let rt = s.rt();
        (rt.focused.clone(), rt.woke_at.clone(), rt.statuses.clone())
    };
    let mut infos = vec![];
    for id in s.panes.ids() {
        let (Ok(row), Some((_, last_output))) = (s.db.get_pane(&id), s.panes.activity(&id)) else { continue };
        let Some(last_input) = s.panes.last_input(&id) else { continue };
        let live_mission = row.mission_id.as_deref().and_then(|m| s.db.get_mission(m).ok()).is_some_and(|m| m.archived_at.is_none());
        let quiet_since = last_output.map_or(last_input, |o| o.max(last_input));
        infos.push(Info {
            status: statuses.get(&id).copied().unwrap_or(PaneStatus::Starting),
            last_input,
            idle_for: now.duration_since(quiet_since),
            live_mission,
            focused: focused.as_deref() == Some(id.as_str()),
            woke_recently: woke_at.get(&id).is_some_and(|t| now.duration_since(*t) < WAKE_RESIDENCE),
            typed: s.panes.typed_since_enter(&id),
            row,
        });
    }
    let cfg = lk(&s.settings).auto_hibernate.clone();
    let ctx = Ctx {
        enabled: cfg.enabled,
        idle_minutes: cfg.idle_minutes,
        resident_floor: cfg.resident_floor,
        live_claude: infos.iter().filter(|i| i.row.cli.as_deref() == Some("claude")).count(),
        in_flight: infos.iter().any(|i| i.status == PaneStatus::Hibernated),
    };
    (infos, ctx)
}

/// Called every 30 s by the ticker: hibernates at most one pane.
pub fn tick(s: &AppState) {
    if !lk(&s.settings).auto_hibernate.enabled {
        return;
    }
    let now = Instant::now();
    let (infos, ctx) = snapshot(s, now);
    let cands: Vec<(Candidate, Instant)> = infos.iter().map(|i| (candidate(i), i.last_input)).collect();
    let Some(id) = pick(&cands, &ctx) else { return };
    // the verdict may have changed since the snapshot (focus, typing, a new input): decide again
    if !reasons_for(s, id, false).is_ok_and(|r| r.is_empty()) {
        return;
    }
    let text = strip_ansi(&String::from_utf8_lossy(&s.panes.snapshot(id)));
    if let Err(e) = s.hibernate(id, &last_lines(&text, FRAME_LINES)) {
        tracing::warn!("hibernação automática de {id} falhou: {e}");
    }
}

/// "Por que não hiberna": the reasons that block this pane right now (empty = it may hibernate). `manual`: the user asked to hibernate it now.
pub fn reasons_for(s: &AppState, id: &str, manual: bool) -> AppResult<Vec<String>> {
    let (infos, ctx) = snapshot(s, Instant::now());
    match infos.iter().find(|i| i.row.id == id) {
        Some(i) => Ok(hibernate_blockers(&candidate(i), &ctx, manual)),
        None => fail("not_running", "painel não está rodando"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx { enabled: true, idle_minutes: 10, resident_floor: 2, live_claude: 3, in_flight: false }
    }

    fn ok<'a>(id: &'a str) -> Candidate<'a> {
        Candidate {
            id,
            cli: Some("claude"),
            resident: false,
            focused: false,
            has_parent: false,
            live_mission: false,
            status: PaneStatus::Idle,
            idle_for: Duration::from_secs(15 * 60),
            has_session: true,
            typed: false,
            woke_recently: false,
            remote: false,
        }
    }

    #[test]
    fn idle_claude_pane_may_hibernate() {
        assert!(hibernate_blockers(&ok("p"), &ctx(), false).is_empty());
    }

    #[test]
    fn each_condition_alone_gives_its_reason() {
        let cases: Vec<(Box<dyn Fn(&mut Candidate<'_>, &mut Ctx)>, &str)> = vec![
            (Box::new(|_, c| c.enabled = false), "hibernação automática desligada"),
            (Box::new(|k, _| k.cli = Some("codex")), "só o Claude"),
            (Box::new(|k, _| k.resident = true), "residente"),
            (Box::new(|k, _| k.focused = true), "em foco"),
            (Box::new(|k, _| k.has_parent = true), "chamador"),
            (Box::new(|k, _| k.live_mission = true), "missão em andamento"),
            (Box::new(|k, _| k.typed = true), "digitando"),
            (Box::new(|k, _| k.status = PaneStatus::Working), "não está ocioso"),
            (Box::new(|k, _| k.idle_for = Duration::from_secs(60)), "ocioso há menos de 10 min"),
            (Box::new(|k, _| k.has_session = false), "sem sessão"),
            (Box::new(|_, c| c.in_flight = true), "outra hibernação"),
            (Box::new(|k, _| k.woke_recently = true), "acordou há menos de 5 min"),
        ];
        for (tweak, want) in cases {
            let (mut k, mut c) = (ok("p"), ctx());
            tweak(&mut k, &mut c);
            let r = hibernate_blockers(&k, &c, false);
            assert!(r.iter().any(|m| m.contains(want)), "{want}: {r:?}");
        }
    }

    #[test]
    fn at_the_resident_floor_nothing_hibernates() {
        let c = Ctx { live_claude: 2, ..ctx() };
        let r = hibernate_blockers(&ok("p"), &c, false);
        assert!(r.iter().any(|m| m.contains("no piso")), "{r:?}");
    }

    #[test]
    fn manual_ignores_switch_and_idle_time_only() {
        let (mut k, mut c) = (ok("p"), ctx());
        c.enabled = false;
        k.idle_for = Duration::from_secs(1);
        assert!(hibernate_blockers(&k, &c, true).is_empty());
        k.status = PaneStatus::Working;
        assert!(hibernate_blockers(&k, &c, true).iter().any(|m| m.contains("não está ocioso")));
        let mut f = ok("p");
        f.resident = true;
        assert!(hibernate_blockers(&f, &ctx(), true).iter().any(|m| m.contains("residente")));
    }

    #[test]
    fn woke_recently_blocks() {
        let mut k = ok("p");
        k.woke_recently = true;
        assert!(hibernate_blockers(&k, &ctx(), false).iter().any(|m| m.contains("acordou")));
    }

    /// Two idle "claude" panes (cmd.exe under a Claude id, resumable): one tick hibernates exactly one.
    #[test]
    fn one_tick_hibernates_at_most_one_pane() {
        use crate::providers::{CliDef, PaneSpec};
        use crate::runtime::AppState;
        use crate::settings::AutoHibernate;
        use crate::util::lk;
        use std::sync::Arc;
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: serde_json::Value| {})).unwrap();
        let args = vec!["/k".to_string(), "rem {session}".to_string()];
        lk(&s.providers).clis.push(CliDef { id: "claude".into(), label: "Claude".into(), command: "cmd.exe".into(), session_new: args.clone(), session_resume: args, ..Default::default() });
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        let ids: Vec<String> = (0..2)
            .map(|_| s.spawn_pane(PaneSpec { workspace_id: ws.id.clone(), cli: Some("claude".into()), cwd: d.path().to_string_lossy().into(), ..Default::default() }).unwrap().id)
            .collect();
        let t0 = Instant::now();
        loop {
            s.tick_status();
            let ready = ids.iter().all(|id| s.status_of(id) == Some(PaneStatus::Idle) && String::from_utf8_lossy(&s.panes.snapshot(id)).contains('>'));
            if ready {
                break;
            }
            // teto folgado: com a suíte toda em paralelo os dois cmd.exe demoram a ficar ociosos (o normal é < 1 s)
            assert!(t0.elapsed() < Duration::from_secs(60), "panes not idle");
            std::thread::sleep(Duration::from_millis(100));
        }
        lk(&s.settings).auto_hibernate = AutoHibernate { enabled: true, idle_minutes: 0, resident_floor: 0 };
        tick(&s);
        let hibernated = ids.iter().filter(|id| s.status_of(id) == Some(PaneStatus::Hibernated)).count();
        assert_eq!(hibernated, 1);
        s.shutdown();
    }

    #[test]
    fn pick_takes_the_oldest_input_without_blockers() {
        let t = Instant::now();
        let mut blocked = ok("x");
        blocked.resident = true;
        let cands = vec![(ok("novo"), t), (ok("velho"), t - Duration::from_secs(600)), (blocked, t - Duration::from_secs(900))];
        assert_eq!(pick(&cands, &ctx()), Some("velho"));
    }
}
