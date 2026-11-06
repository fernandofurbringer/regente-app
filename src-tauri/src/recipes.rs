//! Receitas: formações prontas (modo, squad, piloto, slots, respostas de intake). JSON, como as squads.
//! Os originais vêm do binário (`seed/recipes`, copiados para `data/originals/recipes`); o usuário salva em
//! `data/recipes`, que vence o original de mesmo id. Nada aqui escreve nos originais.

use crate::catalog::{self, HarnessArgs};
use crate::error::{fail, AppResult};
use crate::orchestrator::clean_text;
use crate::squads::{self, SquadDef};
use serde_json::json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RecipeDef {
    pub id: String,
    pub name: String,
    pub description: String,
    /// livre | squad | agentico
    pub mode: String,
    /// Id da squad (modo squad).
    pub squad: Option<String>,
    /// Harness do piloto (modo agêntico).
    pub pilot: Option<HarnessArgs>,
    /// Painéis parciais (`PaneSpec` em JSON) do modo livre.
    pub slots: Vec<serde_json::Value>,
    /// Respostas do intake, pela pergunta.
    pub intake: BTreeMap<String, String>,
    pub worktree: Option<bool>,
    pub style: Option<String>,
    /// "original" | "regente", filled on load.
    pub source: String,
}

const MODES: [&str; 3] = ["livre", "squad", "agentico"];

fn dir_of(data: &Path, source: &str) -> PathBuf {
    if source == "original" {
        data.join("originals").join("recipes")
    } else {
        data.join("recipes")
    }
}

fn read_dir(dir: &Path, source: &str) -> Vec<RecipeDef> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<RecipeDef>(&t).map_err(|e| e.to_string())) {
            Ok(mut r) if catalog::valid_name(&r.id) => {
                r.slots = clean_slots(&r.id, std::mem::take(&mut r.slots));
                r.source = source.into();
                out.push(r);
            }
            Ok(r) => tracing::warn!("receita com id inválido ignorada: {:?}", r.id),
            Err(e) => tracing::warn!("receita ilegível ({}): {e}", path.display()),
        }
    }
    out
}

/// Every recipe: the user's (`data/recipes`) first, then the originals whose id the user did not take.
pub fn list(data: &Path) -> Vec<RecipeDef> {
    let mut out = read_dir(&dir_of(data, "regente"), "regente");
    for o in read_dir(&dir_of(data, "original"), "original") {
        if !out.iter().any(|r| r.id == o.id) {
            out.push(o);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn get(data: &Path, id: &str) -> Option<RecipeDef> {
    if !catalog::valid_name(id) {
        return None;
    }
    list(data).into_iter().find(|r| r.id == id)
}

/// Writes `data/recipes/<id>.json`. The mode, the squad name and the pilot's values are checked here.
pub fn save(data: &Path, r: &RecipeDef) -> AppResult<()> {
    if !catalog::valid_name(&r.id) {
        return fail("bad_name", "id da receita: minúsculas, dígitos e hífen (até 64)");
    }
    if !MODES.contains(&r.mode.as_str()) {
        return fail("bad_mode", "modo: livre, squad ou agentico");
    }
    if let Some(sq) = r.squad.as_deref().filter(|s| !catalog::valid_name(s)) {
        return fail("bad_name", format!("squad com nome inválido: {sq}"));
    }
    if let Some(p) = &r.pilot {
        // pilot values reach the CLI command line
        for v in [p.cli.as_deref(), p.model.as_deref(), p.effort.as_deref()].into_iter().flatten() {
            crate::mcp::tools_panes::plain_arg(v)?;
        }
    }
    let slots = r.slots.iter().map(sanitize_slot).collect::<AppResult<Vec<_>>>()?;
    let dir = dir_of(data, "regente");
    std::fs::create_dir_all(&dir)?;
    let mut r = r.clone();
    r.slots = slots;
    r.source = String::new();
    std::fs::write(dir.join(format!("{}.json", r.id)), serde_json::to_string_pretty(&r)?)?;
    Ok(())
}

/// Apaga `data/recipes/<id>.json`. Só as do usuário: os originais vêm do binário e não se removem.
pub fn remove(data: &Path, id: &str) -> AppResult<()> {
    if !catalog::valid_name(id) {
        return fail("bad_name", "id da receita inválido");
    }
    let path = dir_of(data, "regente").join(format!("{id}.json"));
    if !path.is_file() {
        return fail("not_found", "receita não encontrada (os originais não podem ser removidos)");
    }
    std::fs::remove_file(path)?;
    Ok(())
}

/// The fields a slot keeps (a `PaneSpec` subset for agents). `allowedSkills` (a list) is handled apart. Everything else
/// (`run`, `appendSystem`, `parentId`, ...) is dropped.
const SLOT_KEYS: [&str; 8] = ["cli", "model", "effort", "permission", "name", "prompt", "account", "agent"];

/// A slot as a recipe may keep it: the agent fields only, `kind` always "agent". `danger` (or any permission but plan, pend, auto) is refused;
/// CLI values pass `plain_arg`; the prompt is cleaned and cut to 4000 characters.
pub fn sanitize_slot(v: &serde_json::Value) -> AppResult<serde_json::Value> {
    let Some(obj) = v.as_object() else { return fail("bad_slot", "cada slot é um objeto") };
    let mut out = serde_json::Map::new();
    out.insert("kind".into(), json!("agent"));
    for k in SLOT_KEYS {
        let Some(val) = obj.get(k).filter(|x| !x.is_null()) else { continue };
        let Some(s) = val.as_str() else { return fail("bad_slot", format!("slot: {k} precisa ser texto")) };
        let kept = match k {
            "permission" if !matches!(s, "plan" | "pend" | "auto") => return fail("bad_slot", "permissão do slot: plan, pend ou auto"),
            "cli" | "model" | "effort" | "account" => {
                crate::mcp::tools_panes::plain_arg(s)?;
                s.to_string()
            }
            "agent" => {
                if !catalog::valid_name(s) {
                    return fail("bad_slot", "slot: nome de agente inválido");
                }
                s.to_string()
            }
            "prompt" => clean_text(s).chars().take(4000).collect(),
            _ => s.to_string(),
        };
        out.insert(k.into(), json!(kept));
    }
    // skills do slot: lista de nomes válidos (substitui as do agente); até 32
    if let Some(v) = obj.get("allowedSkills").filter(|x| !x.is_null()) {
        let Some(list) = v.as_array() else { return fail("bad_slot", "slot: allowedSkills precisa ser uma lista") };
        let mut names = Vec::new();
        for x in list.iter().take(32) {
            let Some(n) = x.as_str().filter(|n| catalog::valid_name(n)) else { return fail("bad_slot", "slot: nome de skill inválido") };
            names.push(n.to_string());
        }
        out.insert("allowedSkills".into(), json!(names));
    }
    Ok(serde_json::Value::Object(out))
}

/// Loaded slots: the valid ones, sanitized; a hand-edited invalid slot is dropped with a warning.
fn clean_slots(id: &str, slots: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    slots
        .iter()
        .filter_map(|s| match sanitize_slot(s) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("slot inválido ignorado na receita {id}: {e}");
                None
            }
        })
        .collect()
}

/// Answers for a squad's intake: the recipe's answers (matched by the question text), over the brief's autofill.
pub fn intake_answers(r: &RecipeDef, squad: &SquadDef, brief: &str) -> BTreeMap<usize, String> {
    let mut out = squads::autofill(&squad.intake, brief);
    for (i, q) in squad.intake.iter().enumerate() {
        if let Some(a) = r.intake.get(&q.q) {
            out.insert(i, a.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn seven_originals() {
        let d = tmp();
        crate::catalog::seed(d.path()).unwrap();
        let mut ids: Vec<String> = list(d.path()).into_iter().filter(|r| r.source == "original").map(|r| r.id).collect();
        ids.sort();
        assert_eq!(
            ids,
            ["agentico-padrao", "livre-3-claude", "plano-execucao", "receita-app-web", "receita-automacao", "receita-jogo-web", "receita-saas-publicado", "receita-site-completo"]
        );
    }

    #[test]
    fn intake_answers_recipe_wins_and_autofill_fills_the_rest() {
        let d = tmp();
        crate::catalog::seed(d.path()).unwrap();
        let squad = squads::get(d.path(), "site-completo").unwrap();
        let recipe = RecipeDef {
            id: "minha".into(),
            mode: "squad".into(),
            squad: Some("site-completo".into()),
            intake: BTreeMap::from([
                (squad.intake[1].q.clone(), "Ingressos online".to_string()),
                (squad.intake[3].q.clone(), "https://outro.com".to_string()),
            ]),
            ..Default::default()
        };
        let got = intake_answers(&recipe, &squad, "site pra cinema, conteúdo em https://loja.com");
        assert_eq!(got.get(&1).map(String::as_str), Some("Ingressos online"), "the recipe answers question 2");
        assert_eq!(got.get(&3).map(String::as_str), Some("https://outro.com"), "the recipe wins over the autofill");
        assert!(!got.contains_key(&4), "nothing answers question 5");
        let plain = RecipeDef { id: "plain".into(), mode: "squad".into(), ..Default::default() };
        assert_eq!(intake_answers(&plain, &squad, "https://loja.com").get(&3).map(String::as_str), Some("https://loja.com"));
    }

    #[test]
    fn slots_keep_only_agent_fields() {
        let hostile = json!({"kind": "terminal", "run": "calc", "appendSystem": "x", "parentId": "p", "cli": "claude", "model": "m", "prompt": "oi", "role": "piloto"});
        assert_eq!(sanitize_slot(&hostile).unwrap(), json!({"kind": "agent", "cli": "claude", "model": "m", "prompt": "oi"}));
        assert_eq!(sanitize_slot(&json!({"permission": "danger"})).unwrap_err().code(), "bad_slot");
        assert_eq!(sanitize_slot(&json!({"permission": "pend"})).unwrap()["permission"], "pend");
        assert_eq!(sanitize_slot(&json!({"cli": "x\"y"})).unwrap_err().code(), "bad_arg");
        assert_eq!(sanitize_slot(&json!({"cli": 3})).unwrap_err().code(), "bad_slot");
        let long = sanitize_slot(&json!({"prompt": "x".repeat(5000)})).unwrap();
        assert_eq!(long["prompt"].as_str().unwrap().chars().count(), 4000);
    }

    #[test]
    fn slot_keeps_agent_and_skills_and_refuses_bad_names() {
        let ok = sanitize_slot(&json!({"cli": "claude", "agent": "sq-jogos", "allowedSkills": ["revisar", "testar"], "model": "opus", "effort": "high"})).unwrap();
        assert_eq!(ok, json!({"kind": "agent", "cli": "claude", "model": "opus", "effort": "high", "agent": "sq-jogos", "allowedSkills": ["revisar", "testar"]}));
        assert_eq!(sanitize_slot(&json!({"agent": "Mau Nome"})).unwrap_err().code(), "bad_slot");
        assert_eq!(sanitize_slot(&json!({"allowedSkills": ["../x"]})).unwrap_err().code(), "bad_slot");
        assert_eq!(sanitize_slot(&json!({"allowedSkills": "revisar"})).unwrap_err().code(), "bad_slot");
        // lista vazia é válida: "nenhuma skill"
        assert_eq!(sanitize_slot(&json!({"allowedSkills": []})).unwrap()["allowedSkills"], json!([]));
        assert!(sanitize_slot(&json!({"allowedSkills": null})).unwrap().get("allowedSkills").is_none());
    }

    #[test]
    fn a_saved_stack_keeps_its_slot_and_can_be_removed() {
        let d = tmp();
        let r = RecipeDef {
            id: "stack-1".into(),
            name: "Revisor forte".into(),
            mode: "livre".into(),
            slots: vec![json!({"cli": "claude", "model": "opus", "agent": "sq-seguranca", "allowedSkills": ["revisar"]})],
            ..Default::default()
        };
        save(d.path(), &r).unwrap();
        let got = get(d.path(), "stack-1").unwrap();
        assert_eq!(got.slots[0]["agent"], "sq-seguranca");
        assert_eq!(got.slots[0]["allowedSkills"], json!(["revisar"]));
        remove(d.path(), "stack-1").unwrap();
        assert!(get(d.path(), "stack-1").is_none());
        assert_eq!(remove(d.path(), "stack-1").unwrap_err().code(), "not_found");
        assert_eq!(remove(d.path(), "../x").unwrap_err().code(), "bad_name");
    }

    #[test]
    fn originals_cannot_be_removed() {
        let d = tmp();
        crate::catalog::seed(d.path()).unwrap();
        assert_eq!(remove(d.path(), "agentico-padrao").unwrap_err().code(), "not_found");
        assert!(get(d.path(), "agentico-padrao").is_some());
    }

    #[test]
    fn save_refuses_danger_slots_and_load_drops_hand_edited_ones() {
        let d = tmp();
        let refused = RecipeDef { id: "perigosa".into(), mode: "livre".into(), slots: vec![json!({"permission": "danger"})], ..Default::default() };
        assert_eq!(save(d.path(), &refused).unwrap_err().code(), "bad_slot");
        std::fs::create_dir_all(d.path().join("recipes")).unwrap();
        let hand = r#"{"id":"editada","name":"e","description":"","mode":"livre","slots":[{"permission":"danger"},{"kind":"terminal","run":"calc","cli":"claude"}]}"#;
        std::fs::write(d.path().join("recipes").join("editada.json"), hand).unwrap();
        assert_eq!(get(d.path(), "editada").unwrap().slots, vec![json!({"kind": "agent", "cli": "claude"})]);
    }

    #[test]
    fn save_checks_mode_names_and_pilot_values() {
        let d = tmp();
        let base = RecipeDef { id: "ok".into(), mode: "livre".into(), ..Default::default() };
        assert!(save(d.path(), &RecipeDef { mode: "outro".into(), ..base.clone() }).is_err());
        assert!(save(d.path(), &RecipeDef { squad: Some("Bad Name".into()), ..base.clone() }).is_err());
        assert!(save(d.path(), &RecipeDef { pilot: Some(HarnessArgs { model: Some("--x".into()), ..Default::default() }), ..base.clone() }).is_err());
        assert!(save(d.path(), &base).is_ok());
        assert_eq!(get(d.path(), "ok").unwrap().source, "regente");
    }
}
