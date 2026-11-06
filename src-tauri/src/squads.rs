//! Squads: uma composição de time (maestro + agentes do catálogo) com intake, fases e gates. Formato JSON.
//! Os originais vêm do binário (`seed/squads`, copiados para `data/originals/squads`); o usuário salva em
//! `data/squads`, que vence o original de mesmo id. Nada aqui escreve nos originais.

use crate::catalog::{self, HarnessArgs};
use crate::db::{Gate, Mission, PaneRow, Workspace};
use crate::error::{fail, AppResult};
use crate::missions::{self, MissionOpts};
use crate::orchestrator::bracketed;
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::util::{lk, new_id, now_ms};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct IntakeQ {
    pub q: String,
    /// "first_url" | "inspired_by"
    pub autofill: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RosterEntry {
    pub agent: String,
    pub role: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max: Option<u32>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Phase {
    pub name: String,
    pub steps: Vec<String>,
    pub gate: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaAllow {
    pub providers: Vec<String>,
    pub models: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SquadDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub intake: Vec<IntakeQ>,
    pub roster: Vec<RosterEntry>,
    pub phases: Vec<Phase>,
    pub maestro_notes: String,
    /// Harness of the maestro; None = claude / opus / high.
    pub maestro: Option<RosterEntry>,
    pub media: Option<MediaAllow>,
    pub peak_panes: Option<u32>,
    /// "original" | "regente", filled on load.
    pub source: String,
}

fn dir_of(data: &Path, source: &str) -> std::path::PathBuf {
    if source == "original" {
        data.join("originals").join("squads")
    } else {
        data.join("squads")
    }
}

fn read_dir(dir: &Path, source: &str) -> Vec<SquadDef> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<SquadDef>(&t).map_err(|e| e.to_string())) {
            Ok(mut s) if catalog::valid_name(&s.id) => {
                s.source = source.into();
                out.push(s);
            }
            Ok(s) => tracing::warn!("squad com id inválido ignorada: {:?}", s.id),
            Err(e) => tracing::warn!("squad ilegível ({}): {e}", path.display()),
        }
    }
    out
}

/// Every squad: the user's (`data/squads`) first, then the originals whose id the user did not take.
pub fn list(data: &Path) -> Vec<SquadDef> {
    let mut out = read_dir(&dir_of(data, "regente"), "regente");
    for o in read_dir(&dir_of(data, "original"), "original") {
        if !out.iter().any(|s| s.id == o.id) {
            out.push(o);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn get(data: &Path, id: &str) -> Option<SquadDef> {
    if !catalog::valid_name(id) {
        return None;
    }
    list(data).into_iter().find(|s| s.id == id)
}

/// Caps for a saved squad (the UI and squad_save share them).
pub const MAX_PHASES: usize = 12;
pub const MAX_ROSTER: usize = 16;
pub const TEXT_MAX: usize = 4000;
pub const SQUAD_BYTES_MAX: usize = 64 * 1024;

/// Writes `data/squads/<id>.json` (a user squad; it overrides an original with the same id).
pub fn save(data: &Path, s: &SquadDef) -> AppResult<()> {
    if !catalog::valid_name(&s.id) {
        return fail("bad_name", "id da squad: minúsculas, dígitos e hífen (até 64)");
    }
    for r in &s.roster {
        if !catalog::valid_name(&r.agent) {
            return fail("bad_name", format!("agente do time com nome inválido: {}", r.agent));
        }
        // roster values reach the CLI command line
        for v in [r.cli.as_deref(), r.model.as_deref(), r.effort.as_deref()].into_iter().flatten() {
            crate::mcp::tools_panes::plain_arg(v)?;
        }
    }
    if let Some(m) = &s.maestro {
        check_maestro_cli(m.cli.as_deref())?;
        for v in [m.cli.as_deref(), m.model.as_deref(), m.effort.as_deref()].into_iter().flatten() {
            crate::mcp::tools_panes::plain_arg(v)?;
        }
    }
    // caps shared by the UI and the MCP: the squad must stay a readable brief, not a dump
    if s.phases.len() > MAX_PHASES || s.roster.len() > MAX_ROSTER {
        return fail("too_many", format!("uma squad tem até {MAX_PHASES} fases e {MAX_ROSTER} agentes"));
    }
    let texts = [s.name.as_str(), s.description.as_str(), s.maestro_notes.as_str()]
        .into_iter()
        .chain(s.intake.iter().map(|q| q.q.as_str()))
        .chain(s.roster.iter().filter_map(|r| r.role.as_deref()))
        .chain(s.phases.iter().flat_map(|p| std::iter::once(p.name.as_str()).chain(p.gate.as_deref()).chain(p.steps.iter().map(String::as_str))));
    if texts.into_iter().any(|t| t.chars().count() > TEXT_MAX) {
        return fail("text_too_long", format!("um texto da squad passa de {TEXT_MAX} caracteres"));
    }
    let mut s = s.clone();
    s.source = String::new();
    if serde_json::to_vec(&s)?.len() > SQUAD_BYTES_MAX {
        return fail("too_big", "a squad passa de 64 KB");
    }
    let dir = dir_of(data, "regente");
    std::fs::create_dir_all(&dir)?;
    // atômico: escreve um temporário e troca; uma falha no meio não deixa o JSON pela metade
    crate::settings::save_json(&dir.join(format!("{}.json", s.id)), &s)
}

/// The roster's harness for `agent`, or None when the agent is not in the squad.
pub fn roster_harness(s: &SquadDef, agent: &str) -> Option<HarnessArgs> {
    s.roster.iter().find(|r| r.agent == agent).map(|r| HarnessArgs { cli: r.cli.clone(), model: r.model.clone(), effort: r.effort.clone() })
}

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"]+"#).expect("regex"));
static INSPIRED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)inspirad[oa]\s+(?:em|no|na|pelo|pela)\s+([^\s,;!?]+)").expect("regex"));

/// Answers the intake questions the brief already gives: the first URL, or the reference after "inspirado em".
pub fn autofill(intake: &[IntakeQ], brief: &str) -> BTreeMap<usize, String> {
    let mut out = BTreeMap::new();
    for (i, q) in intake.iter().enumerate() {
        let found = match q.autofill.as_deref() {
            Some("first_url") => URL.find(brief).map(|m| m.as_str().trim_end_matches('.').to_string()),
            Some("inspired_by") => INSPIRED.captures(brief).map(|c| c[1].trim_end_matches('.').to_string()),
            _ => None,
        };
        if let Some(v) = found.filter(|v| !v.is_empty()) {
            out.insert(i, v);
        }
    }
    out
}

/// System prompt of the maestro: it only delegates, names the roster, runs the phases and stops at each gate.
pub fn maestro_prompt(s: &SquadDef, brief: &str, answers: &BTreeMap<usize, String>) -> String {
    let mut p = format!(
        "Você é o maestro desta missão Squad: \"{}\". {}\n\nVocê só delega. Não edita arquivos, não roda comandos e não escreve código: cada trabalho vai para um agente do time.\n\n",
        s.name, s.description
    );
    p += "## Time (agente · função · harness)\n| Agente | Função | Harness |\n|---|---|---|\n";
    for r in &s.roster {
        let harness = [r.cli.as_deref(), r.model.as_deref(), r.effort.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
        p += &format!("| {} | {} | {} |\n", r.agent, r.role.as_deref().unwrap_or("-"), if harness.is_empty() { "padrão do agente" } else { &harness });
    }
    p += "\n## Fases\n";
    for (i, ph) in s.phases.iter().enumerate() {
        p += &format!("{}. {}\n", i + 1, ph.name);
        for step in &ph.steps {
            p += &format!("   - {step}\n");
        }
        if let Some(g) = &ph.gate {
            p += &format!("   GATE: {g} → chame gate_request e espere a decisão antes da próxima fase.\n");
        }
    }
    p += "\n## Respostas do intake\n";
    for (i, q) in s.intake.iter().enumerate() {
        let a = answers.get(&i).map(String::as_str).unwrap_or("(não informado)");
        p += &format!("- {}: {a}\n", q.q);
    }
    if !s.maestro_notes.trim().is_empty() {
        p += &format!("\n## Notas do maestro\n{}\n", s.maestro_notes.trim());
    }
    p += &format!("\n## Brief do usuário\n{}\n", brief.trim());
    p += "\n## Regras\n\
        - Invoque agentes pelo nome com agent_invoke. Nunca invente um nome fora do time.\n\
        - Use task_create e task_update para o placar da missão.\n\
        - Cada entrega chega como mensagem [Regente]; reaja a ela sem pedir status.\n\
        - Ajustes pedidos nos gates reenviam só os agentes necessários.\n\
        - Gates: chame gate_request com título e resumo e espere a decisão; não avance sem ela.\n\
        - Ao final, chame mission_set_status com \"pronta\" e dê ao usuário um resumo do que foi entregue.\n";
    p
}

/// Tools the maestro may not use: it only delegates.
// MultiEdit não existe mais no Claude Code (o `--settings` avisa "matches no known tool"); Edit cobre as edições
const MAESTRO_DENIED: [&str; 5] = ["Edit", "Write", "NotebookEdit", "Bash", "PowerShell"];

/// Only Claude Code can run the maestro (its tool restrictions depend on it).
pub fn check_maestro_cli(cli: Option<&str>) -> AppResult<()> {
    match cli {
        Some(c) if c != "claude" => fail("bad_cli", "o maestro precisa do Claude Code"),
        _ => Ok(()),
    }
}

/// The maestro's pane spec: the only pane of a squad mission. Model and effort from the squad, else opus / high.
fn maestro_spec(ws: &str, mission: &str, squad: &SquadDef, brief: &str, answers: &BTreeMap<usize, String>, cwd: &str, parent: Option<&str>, permission: Permission) -> PaneSpec {
    let h = squad.maestro.clone().unwrap_or_default();
    PaneSpec {
        workspace_id: ws.into(),
        mission_id: Some(mission.into()),
        kind: PaneKind::Agent,
        name: Some(format!("Maestro · {}", squad.name)),
        cli: Some("claude".into()),
        model: Some(h.model.unwrap_or_else(|| "claude-opus-5-5".into())),
        effort: Some(h.effort.unwrap_or_else(|| "high".into())),
        permission,
        cwd: cwd.into(),
        prompt: Some(format!("Comece a missão agora. Brief do usuário:\n{brief}")),
        parent_id: parent.map(Into::into),
        role: Some("maestro".into()),
        mcp_profile: Some("minimal".into()),
        append_system: Some(maestro_prompt(squad, brief, answers)),
        disallowed_tools: Some(MAESTRO_DENIED.iter().map(|t| t.to_string()).collect()),
        ..Default::default()
    }
}

/// A squad mission: a "squad" mission with only the maestro's pane open (the team comes on demand).
/// If the maestro cannot start, the mission is archived and its worktree removed.
#[allow(clippy::too_many_arguments)]
pub fn start_squad_mission(s: &AppState, ws: &Workspace, squad: &SquadDef, name: &str, brief: &str, answers: BTreeMap<usize, String>, permission: Permission, opts: &MissionOpts) -> AppResult<(Mission, PaneRow)> {
    let (worktree, parent, recipe_id, profile) = (opts.worktree, opts.parent.as_deref(), opts.recipe_id.as_deref(), opts.profile.as_deref());
    check_maestro_cli(squad.maestro.as_ref().and_then(|m| m.cli.as_deref()))?;
    let (mut m, cwd) = missions::new_livre(ws, name, worktree)?;
    m.mode = "squad".into();
    m.squad_id = Some(squad.id.clone());
    m.brief = Some(brief.into());
    m.recipe_id = recipe_id.map(Into::into);
    m.profile = profile.map(Into::into);
    s.db.insert_mission(&m)?;
    crate::worktree_cache::acelerar(s, ws, &m);
    match s.spawn_pane(maestro_spec(&ws.id, &m.id, squad, brief, &answers, &cwd, parent, permission)) {
        Ok(row) => Ok((m, row)),
        Err(e) => {
            if let Some(p) = m.worktree_path.as_deref() {
                if let Err(err) = missions::remove_worktree(Path::new(&ws.path), Path::new(p)) {
                    tracing::warn!("worktree da missão {} não removida após falha: {err}", m.id);
                }
            }
            m.status = "arquivada".into();
            m.archived_at = Some(now_ms());
            s.db.update_mission(&m)?;
            Err(e)
        }
    }
}

/// What happened to a decision. `PaneClosed`: the decision is recorded and nothing is typed; the UI shows a warning.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GateReply {
    /// The waiting `gate_request` got it.
    Waiter,
    /// No waiter: typed into the pane (a hibernated pane queues it and wakes).
    Typed,
    /// The pane is closed: recorded only.
    PaneClosed,
    /// Gate do plano (`planexec`): recorded; the planexec acts on it. Nothing is typed here.
    Plan,
}

/// Decides a gate. The options are checked first, then the decision is written with a guarded update, so of
/// two concurrent answers only one wins. Only the winner touches the waiter or the pane.
pub fn answer_gate(s: &AppState, id: &str, decision: &str, note: Option<&str>) -> AppResult<GateReply> {
    let gate = s.db.get_gate(id)?;
    let options: Vec<String> = gate.options_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
    if !options.iter().any(|o| o == decision) {
        return fail("bad_decision", "decisão fora das opções do gate");
    }
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    if !s.db.decide_gate(id, decision, note)? {
        return fail("gate_closed", "gate já decidido");
    }
    if let Some(tx) = lk(&s.gate_waiters).remove(id) {
        if tx.send((decision.to_string(), note.map(str::to_string))).is_ok() {
            s.emit("gate-closed", json!({"id": id}));
            return Ok(GateReply::Waiter);
        }
    }
    let closed = s.db.get_pane(&gate.pane_id).map_or(true, |p| p.closed_at.is_some());
    // gate do plano: quem age é o planexec (`on_gate`); nada é digitado no planejador daqui
    let reply = if gate.title == crate::planexec::GATE_PLANO {
        GateReply::Plan
    } else if closed {
        GateReply::PaneClosed
    } else {
        let text = format!("[Regente] Gate «{}»: {decision}.{}", gate.title, note.map(|n| format!(" {n}")).unwrap_or_default());
        if let Err(e) = s.write_input(&gate.pane_id, &bracketed(&text)) {
            tracing::warn!("decisão do gate {id} não entregue ao painel {}: {e}", gate.pane_id);
        }
        GateReply::Typed
    };
    s.emit("gate-closed", json!({"id": id}));
    Ok(reply)
}

/// Decides every open gate of a pane as "cancelado" (the pane closed or died). Waiters get the same answer.
pub fn cancel_pane_gates(s: &AppState, pane: &str) {
    let Ok(gates) = s.db.open_gates_of_pane(pane) else { return };
    for g in gates {
        if s.db.decide_gate(&g.id, "cancelado", None).unwrap_or(false) {
            if let Some(tx) = lk(&s.gate_waiters).remove(&g.id) {
                let _ = tx.send(("cancelado".into(), None));
            }
            s.emit("gate-closed", json!({"id": g.id}));
        }
    }
}

/// Limits of a gate that an agent opens.
pub fn check_gate(title: &str, summary: &str, options: &[String]) -> AppResult<()> {
    if title.trim().is_empty() || title.chars().count() > 120 {
        return fail("bad_gate", "título: de 1 a 120 caracteres");
    }
    if summary.chars().count() > 4000 {
        return fail("bad_gate", "resumo: até 4000 caracteres");
    }
    let distinct: std::collections::BTreeSet<&str> = options.iter().map(String::as_str).collect();
    if options.is_empty() || options.len() > 6 || distinct.len() != options.len() {
        return fail("bad_gate", "opções: de 1 a 6, distintas");
    }
    if options.iter().any(|o| o.trim().is_empty() || o.chars().count() > 40) {
        return fail("bad_gate", "cada opção: de 1 a 40 caracteres");
    }
    Ok(())
}

/// A gate's waiter. Dropping it (its `gate_request` future was dropped, or it ended) removes the waiter.
pub struct GateTicket {
    pub id: String,
    s: std::sync::Arc<AppState>,
    rx: Option<tokio::sync::oneshot::Receiver<(String, Option<String>)>>,
}

impl GateTicket {
    /// Waits for the decision, without a deadline. None when the waiter was dropped unanswered.
    pub async fn decision(mut self) -> Option<(String, Option<String>)> {
        let rx = self.rx.take()?;
        rx.await.ok()
    }
}

impl Drop for GateTicket {
    fn drop(&mut self) {
        lk(&self.s.gate_waiters).remove(&self.id);
    }
}

/// Opens a gate for `pane`: validates it, registers the waiter, then persists it and emits `gate-open`.
/// The waiter exists before the gate does, so a decision that arrives at once cannot miss it.
pub fn open_gate(s: &std::sync::Arc<AppState>, pane: &PaneRow, title: String, summary: String, options: Vec<String>) -> AppResult<GateTicket> {
    check_gate(&title, &summary, &options)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = new_id();
    lk(&s.gate_waiters).insert(id.clone(), tx);
    let ticket = GateTicket { id: id.clone(), s: s.clone(), rx: Some(rx) };
    let gate = Gate {
        id: id.clone(),
        mission_id: pane.mission_id.clone(),
        pane_id: pane.id.clone(),
        title,
        summary,
        options_json: Some(json!(options).to_string()),
        decision: None,
        note: None,
        created_at: now_ms(),
        decided_at: None,
    };
    // on error the ticket drops here and removes its waiter
    s.db.insert_gate(&gate)?;
    s.emit("gate-open", json!(gate));
    s.notify_pane(&gate.pane_id, crate::notify::Kind::Gate, "Gate aguardando decisão", &gate.title);
    Ok(ticket)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn save_enforces_caps_and_writes_atomically() {
        let data = tmp();
        let base = SquadDef { id: "teste".into(), name: "Teste".into(), ..Default::default() };
        // a valid squad is saved, with no temporary file left behind
        save(data.path(), &base).unwrap();
        assert!(data.path().join("squads").join("teste.json").is_file());
        let leftovers = std::fs::read_dir(data.path().join("squads")).unwrap().filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "tmp")).count();
        assert_eq!(leftovers, 0);
        // too many phases
        let many = SquadDef { phases: (0..=MAX_PHASES).map(|i| Phase { name: format!("f{i}"), ..Default::default() }).collect(), ..base.clone() };
        assert_eq!(save(data.path(), &many).unwrap_err().code(), "too_many");
        // a text over the cap, in a step
        let long = SquadDef { phases: vec![Phase { name: "f".into(), steps: vec!["x".repeat(TEXT_MAX + 1)], gate: None }], ..base.clone() };
        assert_eq!(save(data.path(), &long).unwrap_err().code(), "text_too_long");
    }

    fn roster(rows: &[(&str, &str, &str)]) -> Vec<RosterEntry> {
        rows.iter().map(|(agent, role, model)| RosterEntry { agent: (*agent).into(), role: Some((*role).into()), cli: Some("claude".into()), model: Some((*model).into()), ..Default::default() }).collect()
    }

    /// Copies the binary's seed into `data/originals`, as boot does.
    fn seed(data: &Path) {
        crate::catalog::seed(data).unwrap();
    }

    fn squad(id: &str) -> SquadDef {
        SquadDef { id: id.into(), name: "Teste".into(), description: "d".into(), roster: roster(&[("sq-qa", "reviewer", "claude-sonnet-5-5")]), phases: vec![Phase { name: "F".into(), steps: vec!["faça".into()], gate: Some("Aprovar tudo".into()) }], ..Default::default() }
    }

    #[test]
    fn list_has_5_originals() {
        let d = tmp();
        seed(d.path());
        let mut ids: Vec<String> = list(d.path()).into_iter().filter(|s| s.source == "original").map(|s| s.id).collect();
        ids.sort();
        assert_eq!(ids, ["app-web", "automacao", "jogo-web", "saas-publicado", "site-completo"]);
    }

    #[test]
    fn save_overrides_original() {
        let d = tmp();
        seed(d.path());
        let mut s = get(d.path(), "jogo-web").unwrap();
        assert_eq!(s.source, "original");
        s.name = "Meu jogo-web".into();
        save(d.path(), &s).unwrap();
        let got = get(d.path(), "jogo-web").unwrap();
        assert_eq!(got.source, "regente");
        assert_eq!(got.name, "Meu jogo-web");
        assert_eq!(list(d.path()).iter().filter(|x| x.id == "jogo-web").count(), 1);
    }

    #[test]
    fn save_refuses_bad_ids_and_nothing_is_written() {
        let d = tmp();
        assert!(save(d.path(), &squad("../fora")).is_err());
        assert!(save(d.path(), &squad("con")).is_err());
        assert!(!d.path().join("squads").exists() || std::fs::read_dir(d.path().join("squads")).unwrap().count() == 0);
    }

    #[test]
    fn autofill_fills_url_and_reference_only() {
        let intake = vec![IntakeQ { q: "url".into(), autofill: Some("first_url".into()) }, IntakeQ { q: "ref".into(), autofill: Some("inspired_by".into()) }];
        let got = autofill(&intake, "site pra minha loja https://loja.com inspirado no apple.com");
        assert_eq!(got, BTreeMap::from([(0, "https://loja.com".to_string()), (1, "apple.com".to_string())]));
        assert!(autofill(&intake, "nada aqui").is_empty());
    }

    #[test]
    fn maestro_prompt_contains_roster_gates_and_tools() {
        let d = tmp();
        seed(d.path());
        let s = get(d.path(), "site-completo").unwrap();
        let p = maestro_prompt(&s, "um site de cinema", &BTreeMap::new());
        for r in &s.roster {
            assert!(p.contains(&r.agent), "{}", r.agent);
        }
        for ph in &s.phases {
            if let Some(g) = &ph.gate {
                assert!(p.contains(g.as_str()), "{g}");
            }
        }
        assert!(p.contains("gate_request") && p.contains("agent_invoke"));
    }

    #[test]
    fn roster_harness_only_for_roster_agents() {
        let s = SquadDef { roster: roster(&[("sq-frontend", "executor", "claude-sonnet-5-5")]), ..Default::default() };
        assert_eq!(roster_harness(&s, "sq-frontend").unwrap().model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(roster_harness(&s, "sq-qa"), None);
    }

    #[test]
    fn save_refuses_bad_roster_values() {
        let d = tmp();
        let mut s = squad("ok-squad");
        s.roster[0].agent = "Bad Name".into();
        assert!(save(d.path(), &s).is_err());
        s.roster[0].agent = "sq-qa".into();
        s.roster[0].model = Some("--flag".into());
        assert!(save(d.path(), &s).is_err());
    }

    fn app(dir: &Path) -> std::sync::Arc<AppState> {
        AppState::new(dir.into(), std::path::PathBuf::from("C:/x/regente.exe"), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).unwrap()
    }

    #[test]
    fn squad_mission_spawns_only_maestro() {
        let d = tmp();
        let s = app(d.path());
        // cmd.exe stands in for claude; no prompt argument, so the prompt is pasted (as in the MCP tests)
        {
            let mut cfg = lk(&s.providers);
            let claude = cfg.clis.iter_mut().find(|c| c.id == "claude").unwrap();
            claude.command = "cmd.exe".into();
            claude.prompt_args.clear();
        }
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        let jogo_web = get(d.path(), "jogo-web").unwrap();
        let (m, row) = start_squad_mission(&s, &ws, &jogo_web, "Sapo", "um jogo de sapo", BTreeMap::new(), Permission::Auto, &MissionOpts::default()).unwrap();
        assert_eq!((m.mode.as_str(), m.squad_id.as_deref()), ("squad", Some("jogo-web")));
        let panes: Vec<PaneRow> = s.list_panes(&ws.id).unwrap().into_iter().filter(|p| p.mission_id.as_deref() == Some(m.id.as_str())).collect();
        assert_eq!(panes.len(), 1);
        assert_eq!((panes[0].role.as_deref(), panes[0].name.as_str()), (Some("maestro"), "Maestro · Jogo Web"));
        let pasted = String::from_utf8_lossy(&lk(&s.rt).initial_input.get(&row.id).cloned().unwrap_or_default()).to_string();
        assert!(pasted.contains("Brief do usuário"), "{pasted}");
        for id in s.panes.ids() {
            s.close_pane(&id).unwrap();
        }
    }

    #[test]
    fn maestro_spec_denies_edit_and_shell() {
        let d = tmp();
        seed(d.path());
        let jogo_web = get(d.path(), "jogo-web").unwrap();
        let spec = maestro_spec("w", "m", &jogo_web, "b", &BTreeMap::new(), "C:/", None, Permission::Auto);
        assert_eq!(spec.permission, Permission::Auto);
        assert_eq!(spec.mcp_profile.as_deref(), Some("minimal"));
        let denied = spec.disallowed_tools.unwrap();
        for t in ["Edit", "Write", "NotebookEdit", "Bash", "PowerShell"] {
            assert!(denied.iter().any(|x| x == t), "{t}");
        }
        assert!(!denied.iter().any(|x| x == "MultiEdit"), "{denied:?}");
        assert_eq!(spec.model.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn orphan_gate_answer_writes_to_pane() {
        let d = tmp();
        let s = app(d.path());
        s.db.insert_pane(&PaneRow { id: "h".into(), workspace_id: "w".into(), kind: "agent".into(), name: "h".into(), permission: "pend".into(), cwd: "C:/".into(), status: "hibernado".into(), ..Default::default() }).unwrap();
        lk(&s.rt).statuses.insert("h".into(), crate::status::PaneStatus::Hibernated);
        s.db.insert_gate(&Gate { id: "g".into(), mission_id: None, pane_id: "h".into(), title: "Aprovar styleboard".into(), summary: "s".into(), options_json: Some(json!(["aprovar", "ajustar"]).to_string()), decision: None, note: None, created_at: 1, decided_at: None }).unwrap();
        assert!(answer_gate(&s, "g", "talvez", None).is_err(), "decision outside the options");
        answer_gate(&s, "g", "ajustar", Some(" mais leve ")).unwrap();
        let pending = String::from_utf8_lossy(&lk(&s.rt).pending_input["h"]).to_string();
        assert!(pending.contains("Gate «Aprovar styleboard»: ajustar. mais leve"), "{pending}");
        assert_eq!(s.db.get_gate("g").unwrap().decision.as_deref(), Some("ajustar"));
        assert!(answer_gate(&s, "g", "aprovar", None).is_err(), "a decided gate stays decided");
    }

    fn hibernated_pane(s: &AppState, id: &str, closed: bool) {
        s.db.insert_pane(&PaneRow { id: id.into(), workspace_id: "w".into(), kind: "agent".into(), name: id.into(), permission: "pend".into(), cwd: "C:/".into(), status: "hibernado".into(), closed_at: closed.then_some(1), ..Default::default() }).unwrap();
        lk(&s.rt).statuses.insert(id.into(), crate::status::PaneStatus::Hibernated);
    }

    fn gate(id: &str, pane: &str) -> Gate {
        Gate { id: id.into(), mission_id: None, pane_id: pane.into(), title: "Aprovar".into(), summary: "s".into(), options_json: Some(json!(["aprovar", "ajustar"]).to_string()), decision: None, note: None, created_at: 1, decided_at: None }
    }

    #[test]
    fn concurrent_answers_only_one_wins() {
        let d = tmp();
        let s = app(d.path());
        hibernated_pane(&s, "h2", false);
        s.db.insert_gate(&gate("race", "h2")).unwrap();
        let results: Vec<AppResult<GateReply>> = std::thread::scope(|sc| {
            let a = sc.spawn(|| answer_gate(&s, "race", "aprovar", None));
            let b = sc.spawn(|| answer_gate(&s, "race", "ajustar", None));
            vec![a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "exactly one answer wins");
        assert!(results.iter().any(|r| r.as_ref().is_err_and(|e| e.code() == "gate_closed")));
    }

    #[test]
    fn closed_pane_records_the_decision_without_typing() {
        let d = tmp();
        let s = app(d.path());
        hibernated_pane(&s, "c1", true);
        s.db.insert_gate(&gate("g-closed", "c1")).unwrap();
        assert_eq!(answer_gate(&s, "g-closed", "aprovar", None).unwrap(), GateReply::PaneClosed);
        assert!(!lk(&s.rt).pending_input.contains_key("c1"), "nothing typed into a closed pane");
        assert_eq!(s.db.get_gate("g-closed").unwrap().decision.as_deref(), Some("aprovar"));
    }

    #[test]
    fn waiter_is_removed_when_the_request_is_dropped() {
        let d = tmp();
        let s = app(d.path());
        let p = PaneRow { id: "p3".into(), workspace_id: "w".into(), cwd: "C:/".into(), status: "ocioso".into(), ..Default::default() };
        s.db.insert_pane(&p).unwrap();
        let ticket = open_gate(&s, &p, "Aprovar".into(), "s".into(), vec!["aprovar".into()]).unwrap();
        let id = ticket.id.clone();
        assert!(lk(&s.gate_waiters).contains_key(&id));
        // the same future shape gate_request awaits: dropping it unanswered must drop the waiter
        let fut = ticket.decision();
        drop(fut);
        assert!(!lk(&s.gate_waiters).contains_key(&id), "a dropped request removes its waiter");
    }

    #[test]
    fn closing_a_pane_cancels_its_open_gates() {
        let d = tmp();
        let s = app(d.path());
        let p = PaneRow { id: "p4".into(), workspace_id: "w".into(), cwd: "C:/".into(), status: "ocioso".into(), ..Default::default() };
        s.db.insert_pane(&p).unwrap();
        let ticket = open_gate(&s, &p, "Aprovar".into(), "s".into(), vec!["aprovar".into()]).unwrap();
        let id = ticket.id.clone();
        cancel_pane_gates(&s, "p4");
        assert_eq!(s.db.get_gate(&id).unwrap().decision.as_deref(), Some("cancelado"));
        assert!(s.db.open_gates_of_pane("p4").unwrap().is_empty());
    }

    #[test]
    fn gate_limits() {
        let ok = vec!["aprovar".to_string(), "ajustar".to_string()];
        assert!(check_gate("t", "s", &ok).is_ok());
        assert!(check_gate("", "s", &ok).is_err());
        assert!(check_gate(&"x".repeat(121), "s", &ok).is_err());
        assert!(check_gate("t", &"x".repeat(4001), &ok).is_err());
        assert!(check_gate("t", "s", &[]).is_err());
        assert!(check_gate("t", "s", &["a".into(), "a".into()]).is_err());
        assert!(check_gate("t", "s", &(0..7).map(|i| i.to_string()).collect::<Vec<_>>()).is_err());
        assert!(check_gate("t", "s", &["x".repeat(41)]).is_err());
    }

    #[test]
    fn maestro_must_run_on_claude() {
        let d = tmp();
        seed(d.path());
        let mut sq = get(d.path(), "jogo-web").unwrap();
        sq.maestro = Some(RosterEntry { cli: Some("codex".into()), ..Default::default() });
        assert!(save(d.path(), &sq).is_err());
        let s = app(d.path());
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        assert_eq!(start_squad_mission(&s, &ws, &sq, "x", "b", BTreeMap::new(), Permission::Auto, &MissionOpts::default()).unwrap_err().code(), "bad_cli");
        assert!(check_maestro_cli(Some("claude")).is_ok());
        assert!(check_maestro_cli(None).is_ok());
    }

    #[test]
    fn failed_maestro_spawn_archives_the_mission() {
        let d = tmp();
        seed(d.path());
        let s = app(d.path());
        {
            let mut cfg = lk(&s.providers);
            let claude = cfg.clis.iter_mut().find(|c| c.id == "claude").unwrap();
            claude.command = "cli-que-nao-existe-regente".into();
        }
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        let jogo_web = get(d.path(), "jogo-web").unwrap();
        assert!(start_squad_mission(&s, &ws, &jogo_web, "Falha", "b", BTreeMap::new(), Permission::Auto, &MissionOpts { recipe_id: Some("receita-jogo-web".into()), ..Default::default() }).is_err());
        let missions = s.db.list_missions(&ws.id).unwrap();
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].status, "arquivada");
    }
}
