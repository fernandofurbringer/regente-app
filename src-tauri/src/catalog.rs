//! Catálogo de agentes e skills: os originais vêm do binário (`seed/`, copiados para `data/originals` a cada boot),
//! o usuário tem `data/agents` e `data/skills`, e o Claude/Codex do usuário entram só para leitura.
//! Também resolve o harness de um agente (CLI, modelo, esforço) e monta o plugin de skills de cada painel.

use crate::error::AppResult;
use crate::orchestrator::clean_text;
use include_dir::{include_dir, Dir};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

static SEED: Dir = include_dir!("$CARGO_MANIFEST_DIR/seed");

/// Skills text that goes into a non-plugin CLI's system prompt.
const SKILLS_TEXT_MAX: usize = 8000;

#[derive(Debug, Clone, PartialEq)]
pub enum FmValue {
    Str(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Frontmatter {
    pub fields: BTreeMap<String, FmValue>,
    pub body: String,
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    let quoted = v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')));
    if quoted { &v[1..v.len() - 1] } else { v }
}

/// Items of an inline list `a, "b, c", d`: commas inside quotes do not split; quotes are removed.
fn split_list(inner: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote) = (vec![], String::new(), None::<char>);
    for c in inner.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == ',' => out.push(std::mem::take(&mut cur)),
            None => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Simple `---` frontmatter (no YAML): `key: value`, `key: [a, b]`, `key:` followed by `- item` lines, and
/// `key: |` / `key: >` block scalars. Indented lines that are not list items or block lines are ignored, so they
/// cannot create keys. Comments (`#`) are ignored. A missing or unclosed `---` means no frontmatter: the whole text is the body.
pub fn parse_frontmatter(text: &str) -> Frontmatter {
    let none = || Frontmatter { fields: BTreeMap::new(), body: text.to_string() };
    let src = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = src.lines().collect();
    if lines.first().map(|l| l.trim_end()) != Some("---") {
        return none();
    }
    let Some(end) = lines[1..].iter().position(|l| l.trim_end() == "---").map(|p| p + 1) else { return none() };
    let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t');
    let mut fields = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut i = 1;
    while i < end {
        let raw = lines[i];
        i += 1;
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(item) = t.strip_prefix("- ") {
            if let Some(key) = &current {
                if let Some(FmValue::List(items)) = fields.get_mut(key) {
                    items.push(unquote(item).to_string());
                }
            }
            continue;
        }
        if indented(raw) {
            continue;
        }
        let Some((key, value)) = t.split_once(':') else { continue };
        let key = key.trim().to_string();
        let value = value.trim();
        current = None;
        if value == "|" || value == ">" {
            let mut parts = vec![];
            while i < end && (indented(lines[i]) || lines[i].trim().is_empty()) {
                parts.push(lines[i].trim());
                i += 1;
            }
            let joined = if value == "|" { parts.join("\n") } else { parts.join(" ") };
            fields.insert(key, FmValue::Str(joined.trim().to_string()));
        } else if value.is_empty() {
            fields.insert(key.clone(), FmValue::List(vec![]));
            current = Some(key);
        } else if value.starts_with('[') && value.ends_with(']') {
            fields.insert(key, FmValue::List(split_list(&value[1..value.len() - 1])));
        } else {
            fields.insert(key, FmValue::Str(unquote(value).to_string()));
        }
    }
    Frontmatter { fields, body: lines[end + 1..].join("\n").trim().to_string() }
}

/// Windows device names (con, nul, aux, prn, com1-9, lpt1-9): no folder or file may take them as its stem.
pub fn reserved_stem(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    matches!(s.as_str(), "con" | "prn" | "aux" | "nul") || (s.len() == 4 && (s.starts_with("com") || s.starts_with("lpt")) && matches!(s.as_bytes()[3], b'1'..=b'9'))
}

/// Lowercase ASCII names only (`[a-z0-9-]+`, at most 64, not a Windows device name): they become folders and CLI arguments.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !reserved_stem(name)
}

/// A frontmatter scalar as written by `save_agent`: values that would read back as a list or a quoted
/// string, or that contain `: `, are double-quoted (inner double quotes become single quotes).
fn yaml_value(v: &str) -> String {
    if v.starts_with(['[', '"', '\'']) || v.contains(": ") {
        format!("\"{}\"", v.replace('"', "'"))
    } else {
        v.to_string()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub role: Option<String>,
    pub skills: Vec<String>,
    pub body: String,
    /// original | regente | claude
    pub source: String,
    pub path: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDef {
    pub name: String,
    pub description: String,
    /// original | regente | claude | codex
    pub source: String,
    pub dir: String,
}

/// Rewrites `data/originals/**` from the binary's copy. Runs on every boot (user edits to originals are replaced).
pub fn seed(data: &Path) -> AppResult<()> {
    let root = data.join("originals");
    let _ = std::fs::remove_dir_all(&root);
    write_seed(&SEED, &root)
}

fn write_seed(dir: &Dir, root: &Path) -> AppResult<()> {
    for f in dir.files() {
        let dest = root.join(f.path());
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, f.contents())?;
    }
    for d in dir.dirs() {
        write_seed(d, root)?;
    }
    Ok(())
}

/// Agent folders in precedence order: the first one with a given name wins.
fn agent_dirs(data: &Path, home: &Path) -> [(PathBuf, &'static str); 3] {
    [(data.join("agents"), "regente"), (data.join("originals").join("agents"), "original"), (home.join(".claude").join("agents"), "claude")]
}

fn agent_from(text: &str, source: &str, path: &Path) -> AgentDef {
    let fm = parse_frontmatter(text);
    let s = |k: &str| match fm.fields.get(k) {
        Some(FmValue::Str(v)) if !v.is_empty() => Some(v.clone()),
        _ => None,
    };
    let skills = match fm.fields.get("skills") {
        Some(FmValue::List(v)) => v.clone(),
        _ => vec![],
    };
    let fallback = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    AgentDef {
        name: s("name").unwrap_or(fallback),
        description: s("description").unwrap_or_default(),
        cli: s("cli"),
        model: s("model"),
        effort: s("effort"),
        role: s("role"),
        skills,
        body: fm.body,
        source: source.into(),
        path: path.to_string_lossy().into(),
    }
}

/// Every agent, by precedence (regente > original > claude), sorted by name.
pub fn list_agents(data: &Path, home: &Path) -> Vec<AgentDef> {
    let mut out: Vec<AgentDef> = vec![];
    for (dir, source) in agent_dirs(data, home) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let a = agent_from(&text, source, &path);
            if valid_name(&a.name) && !out.iter().any(|x| x.name == a.name) {
                out.push(a);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn get_agent(data: &Path, home: &Path, name: &str) -> Option<AgentDef> {
    if !valid_name(name) {
        return None;
    }
    list_agents(data, home).into_iter().find(|a| a.name == name)
}

/// Writes a user agent to `data/agents/<name>.md` (it overrides the original of the same name).
pub fn save_agent(data: &Path, a: &AgentDef) -> AppResult<PathBuf> {
    use crate::error::fail;
    if !valid_name(&a.name) {
        return fail("bad_name", "nome do agente: use só letras minúsculas, dígitos e hífen");
    }
    let single = [Some(&a.description), a.cli.as_ref(), a.model.as_ref(), a.effort.as_ref(), a.role.as_ref()];
    if single.into_iter().flatten().any(|v| v.contains(['\n', '\r'])) {
        return fail("bad_field", "campos do agente devem ter uma linha só");
    }
    if let Some(bad) = a.skills.iter().find(|s| !valid_name(s)) {
        return fail("bad_skill", format!("nome de skill inválido: {bad}"));
    }
    let mut head = format!("---\nname: {}\ndescription: {}\n", a.name, yaml_value(&a.description));
    for (k, v) in [("cli", &a.cli), ("model", &a.model), ("effort", &a.effort), ("role", &a.role)] {
        if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
            head.push_str(&format!("{k}: {}\n", yaml_value(v)));
        }
    }
    head.push_str(&format!("skills: [{}]\n---\n", a.skills.join(", ")));
    let dir = data.join("agents");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.md", a.name));
    std::fs::write(&path, format!("{head}{}\n", a.body.trim()))?;
    Ok(path)
}

/// Skill body (frontmatter removed) and its catalog entry.
fn read_skill(dir: &Path, source: &str) -> Option<(SkillDef, String)> {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let fm = parse_frontmatter(&text);
    let field = |k: &str| match fm.fields.get(k) {
        Some(FmValue::Str(v)) => Some(v.clone()),
        _ => None,
    };
    let name = field("name").or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))?;
    let def = SkillDef { name, description: field("description").unwrap_or_default(), source: source.into(), dir: dir.to_string_lossy().into() };
    Some((def, fm.body))
}

fn skill_dirs(data: &Path, home: &Path) -> [(PathBuf, &'static str); 4] {
    [
        (data.join("skills"), "regente"),
        (data.join("originals").join("skills"), "original"),
        (home.join(".claude").join("skills"), "claude"),
        (home.join(".codex").join("skills"), "codex"),
    ]
}

pub fn list_skills(data: &Path, home: &Path) -> Vec<SkillDef> {
    let mut out: Vec<SkillDef> = vec![];
    for (root, source) in skill_dirs(data, home) {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let Some((def, _)) = read_skill(&e.path(), source) else { continue };
            if valid_name(&def.name) && !out.iter().any(|x| x.name == def.name) {
                out.push(def);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn find_skill(data: &Path, home: &Path, name: &str) -> Option<SkillDef> {
    if !valid_name(name) {
        return None;
    }
    list_skills(data, home).into_iter().find(|s| s.name == name)
}

#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct HarnessArgs {
    pub cli: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Harness {
    pub cli: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Where each field came from, for the receipt.
    pub source: String,
}

/// `cima` por cima de `baixo`, campo a campo. Se `cima` escolhe o modelo ou troca a CLI, nada de `baixo` entra (o modelo
/// e o esforço de `baixo` podem ser de outra CLI); só esforço em `cima` se junta ao resto de `baixo`.
pub fn sobre(cima: &HarnessArgs, baixo: &HarnessArgs) -> HarnessArgs {
    if cima.model.is_some() || (cima.cli.is_some() && cima.cli != baixo.cli) {
        return cima.clone();
    }
    HarnessArgs { cli: cima.cli.clone().or(baixo.cli.clone()), model: cima.model.clone().or(baixo.model.clone()), effort: cima.effort.clone().or(baixo.effort.clone()) }
}

/// Precedence for each field: roster > invocation argument > agent default > profile slot > CLI default.
/// The profile slot (`perfil`, from `agentic::perfil_harness`) carries its own CLI (None = `claude`): it may choose the
/// CLI when nothing above does, and its model and effort count only when the resolved CLI is that one.
/// Quem quer que o perfil vença o argumento e o agente (papel da missão, escolha do usuário) passa o slot como `roster`.
pub fn resolve_harness(roster: Option<&HarnessArgs>, arg: &HarnessArgs, agent: Option<&AgentDef>, perfil: &HarnessArgs, default_cli: &str) -> Harness {
    let agent_args = HarnessArgs { cli: agent.and_then(|a| a.cli.clone()), model: agent.and_then(|a| a.model.clone()), effort: agent.and_then(|a| a.effort.clone()) };
    let pick = |f: fn(&HarnessArgs) -> &Option<String>, usa_perfil: bool| -> (Option<String>, &'static str) {
        if let Some(v) = roster.and_then(|r| f(r).clone()) {
            return (Some(v), "roster");
        }
        if let Some(v) = f(arg).clone() {
            return (Some(v), "argumento");
        }
        if let Some(v) = f(&agent_args).clone() {
            return (Some(v), "agente");
        }
        if usa_perfil {
            if let Some(v) = f(perfil).clone() {
                return (Some(v), "perfil");
            }
        }
        (None, "padrão")
    };
    let (cli, cli_src) = pick(|h| &h.cli, true);
    let usa_perfil = cli.as_deref().unwrap_or(default_cli) == perfil.cli.as_deref().unwrap_or("claude");
    let (model, model_src) = pick(|h| &h.model, usa_perfil);
    let (effort, effort_src) = pick(|h| &h.effort, usa_perfil);
    Harness {
        cli: cli.unwrap_or_else(|| default_cli.into()),
        model,
        effort,
        source: format!("cli: {cli_src}; modelo: {model_src}; esforço: {effort_src}"),
    }
}

/// The role skill of a pane role; None for roles without one.
pub fn role_skill(role: &str) -> Option<&'static str> {
    Some(match role {
        "piloto" | "maestro" => "orquestrar",
        "ajudante" | "agente" | "executor" => "executar",
        "chefe:reviewer" | "reviewer" | "chefe:cruzado" => "revisar",
        "chefe:scout" | "scout" => "pesquisar",
        "planner" => "planejar",
        "tester" | "chefe:tester" => "testar",
        _ => return None,
    })
}

/// Skills a pane receives. `allowed`: None = role skill + agent skills; Some([]) = none; Some(list) = exactly that list.
pub fn pane_skills(role: Option<&str>, agent: Option<&AgentDef>, allowed: Option<&[String]>) -> Vec<String> {
    if let Some(list) = allowed {
        return list.to_vec();
    }
    let mut out: Vec<String> = role.and_then(role_skill).map(str::to_string).into_iter().collect();
    for s in agent.map(|a| a.skills.as_slice()).unwrap_or_default() {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// Copies each skill into `pane_dir/plugin/skills/<nome>/` and writes `.claude-plugin/plugin.json`.
/// Returns None when no skill was copied; skills not found come back in the second element.
pub fn build_plugin_dir(data: &Path, home: &Path, pane_dir: &Path, pane_id: &str, skills: &[String]) -> AppResult<(Option<PathBuf>, Vec<String>)> {
    let root = pane_dir.join("plugin");
    // a respawn with another skill list must not keep the old copies
    let _ = std::fs::remove_dir_all(&root);
    let mut missing = vec![];
    let mut copied = 0;
    for name in skills {
        match find_skill(data, home, name) {
            Some(sk) => {
                copy_dir(Path::new(&sk.dir), &root.join("skills").join(name))?;
                copied += 1;
            }
            None => {
                tracing::warn!("skill não encontrada para o painel {pane_id}: {name}");
                missing.push(name.clone());
            }
        }
    }
    if copied == 0 {
        return Ok((None, missing));
    }
    let manifest = root.join(".claude-plugin");
    std::fs::create_dir_all(&manifest)?;
    let short: String = pane_id.chars().take(8).collect();
    std::fs::write(manifest.join("plugin.json"), json!({"name": format!("regente-{short}"), "version": "0.0.0"}).to_string())?;
    Ok((Some(root), missing))
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_dir(&e.path(), &dst.join(e.file_name()))?;
        } else if ft.is_file() {
            std::fs::copy(e.path(), dst.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// For CLIs without plugins: the skill bodies, joined, sanitized and capped, to go into the system prompt.
/// Returns the text and the names that did not load.
pub fn skills_text(data: &Path, home: &Path, skills: &[String]) -> (String, Vec<String>) {
    let (mut parts, mut missing) = (vec![], vec![]);
    for name in skills {
        match find_skill(data, home, name).and_then(|sk| read_skill(Path::new(&sk.dir), &sk.source)) {
            Some((_, body)) => parts.push(format!("## Skill: {name}\n{body}")),
            None => {
                tracing::warn!("skill não encontrada: {name}");
                missing.push(name.clone());
            }
        }
    }
    (clean_text(&parts.join("\n\n")).chars().take(SKILLS_TEXT_MAX).collect(), missing)
}

/// Whether `name` is one of the seeded originals (an agent or a skill), whatever the source of an install.
pub fn is_original(data: &Path, name: &str) -> bool {
    valid_name(name) && (data.join("originals").join("agents").join(format!("{name}.md")).is_file() || data.join("originals").join("skills").join(name).is_dir())
}

/// Skills de metodologia por papel: o corpo fica em até 60 linhas (são lidas em cada painel).
pub const METODOLOGIA: [&str; 4] = ["planejar", "executar", "revisar", "testar"];
const METODO_CORPO_MAX: usize = 60;
const CORPO_AVISO: usize = 150;
const CORPO_MAX: usize = 500;
const DESC_AVISO: usize = 300;
const DESC_MAX: usize = 1536;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// aviso | erro
    pub nivel: &'static str,
    pub texto: String,
}

fn finding(nivel: &'static str, texto: String) -> Finding {
    Finding { nivel, texto }
}

/// Confere o tamanho do corpo e da descrição (a descrição é o gatilho que o CLI lê em cada sessão).
pub fn lint_skill(sk: &SkillDef) -> Vec<Finding> {
    let mut out = vec![];
    let linhas = match read_skill(Path::new(&sk.dir), &sk.source) {
        Some((_, body)) => body.lines().count(),
        None => {
            out.push(finding("erro", "não consegui ler o SKILL.md".into()));
            0
        }
    };
    let chars = sk.description.chars().count();
    if chars == 0 {
        out.push(finding("aviso", "sem descrição: falta o gatilho de uso".into()));
    } else if !(sk.description.starts_with("Use") || sk.description.contains("Use quando")) {
        out.push(finding("aviso", "descrição sem gatilho: comece com \"Use\" ou \"Use quando\"".into()));
    }
    if chars > DESC_MAX {
        out.push(finding("erro", format!("descrição com {chars} caracteres (máximo {DESC_MAX})")));
    } else if chars > DESC_AVISO {
        out.push(finding("aviso", format!("descrição com {chars} caracteres (ideal até {DESC_AVISO})")));
    }
    if linhas > CORPO_MAX {
        out.push(finding("erro", format!("corpo com {linhas} linhas (máximo {CORPO_MAX})")));
    } else if linhas > CORPO_AVISO {
        out.push(finding("aviso", format!("corpo com {linhas} linhas (ideal até {CORPO_AVISO})")));
    }
    if METODOLOGIA.contains(&sk.name.as_str()) && linhas > METODO_CORPO_MAX {
        out.push(finding("erro", format!("skill de metodologia com {linhas} linhas (máximo {METODO_CORPO_MAX})")));
    }
    out
}

/// Soma dos caracteres das descrições: o custo fixo que as skills acrescentam ao contexto.
pub fn total_description_chars(skills: &[SkillDef]) -> usize {
    skills.iter().map(|s| s.description.chars().count()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn frontmatter_variants() {
        let fm = parse_frontmatter("---\nname: a\n# comentário\ndescription: x: y\nskills: [b, \"c\"]\nlista:\n- um\n- dois\n---\ncorpo\n");
        assert_eq!(fm.fields["name"], FmValue::Str("a".into()));
        assert_eq!(fm.fields["description"], FmValue::Str("x: y".into()));
        assert_eq!(fm.fields["skills"], FmValue::List(vec!["b".into(), "c".into()]));
        assert_eq!(fm.fields["lista"], FmValue::List(vec!["um".into(), "dois".into()]));
        assert_eq!(fm.body, "corpo");

        let plain = parse_frontmatter("só texto\nlinha 2");
        assert!(plain.fields.is_empty());
        assert_eq!(plain.body, "só texto\nlinha 2");
    }

    #[test]
    fn frontmatter_hardening() {
        // BOM, indented continuation lines that must not create keys, block scalars, quoted commas
        let fm = parse_frontmatter("\u{feff}---\nname: a\ndescription: |\n  linha um\n  linha dois\nskills: [\"x, y\", z]\n  intruso: 1\n---\ncorpo");
        assert_eq!(fm.fields["name"], FmValue::Str("a".into()));
        assert_eq!(fm.fields["description"], FmValue::Str("linha um\nlinha dois".into()));
        assert_eq!(fm.fields["skills"], FmValue::List(vec!["x, y".into(), "z".into()]));
        assert!(!fm.fields.contains_key("intruso"));
        assert_eq!(fm.body, "corpo");
        let folded = parse_frontmatter("---\nd: >\n  a\n  b\n---\n");
        assert_eq!(folded.fields["d"], FmValue::Str("a b".into()));
        // unclosed: no frontmatter at all
        let open = parse_frontmatter("---\nname: a\nsem fechamento");
        assert!(open.fields.is_empty());
        assert!(open.body.starts_with("---"));
    }

    #[test]
    fn reserved_windows_names_are_refused() {
        for n in ["con", "nul", "aux", "prn", "com1", "lpt9"] {
            assert!(!valid_name(n), "{n}");
        }
        assert!(valid_name("com10") && valid_name("lpt"));
    }

    #[test]
    fn save_quotes_risky_values_and_reads_back() {
        let data = tmp();
        let a = AgentDef { name: "meu-agente".into(), description: "x: y".into(), role: Some("[a]".into()), body: "corpo".into(), ..Default::default() };
        let path = save_agent(data.path(), &a).unwrap();
        let back = agent_from(&std::fs::read_to_string(path).unwrap(), "regente", Path::new("meu-agente.md"));
        assert_eq!(back.description, "x: y");
        assert_eq!(back.role.as_deref(), Some("[a]"));
        assert_eq!(back.body, "corpo");
    }

    #[test]
    fn seed_and_list() {
        let (data, home) = (tmp(), tmp());
        seed(data.path()).unwrap();
        let agents = list_agents(data.path(), home.path());
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"sq-frontend"), "{names:?}");
        for chief in ["chefe-scout", "chefe-builder", "chefe-reviewer", "chefe-controller", "chefe-cruzado"] {
            assert!(names.contains(&chief), "{names:?}");
        }
        for specialist in ["sq-jogos", "sq-seguranca", "sq-imagem"] {
            assert!(names.contains(&specialist), "{names:?}");
        }
        assert_eq!(agents.iter().find(|a| a.name == "sq-frontend").unwrap().source, "original");

        let mut mine = get_agent(data.path(), home.path(), "sq-frontend").unwrap();
        mine.body = "override".into();
        save_agent(data.path(), &mine).unwrap();
        let again = get_agent(data.path(), home.path(), "sq-frontend").unwrap();
        assert_eq!(again.source, "regente");
        assert_eq!(again.body, "override");

        let skills: Vec<String> = list_skills(data.path(), home.path()).into_iter().map(|s| s.name).collect();
        for role in ["orquestrar", "executar", "revisar", "pesquisar", "planejar", "testar"] {
            assert!(skills.iter().any(|s| s == role), "{skills:?}");
        }
    }

    #[test]
    fn seed_skills_pass_the_lint() {
        let (data, home) = (tmp(), tmp());
        seed(data.path()).unwrap();
        let skills = list_skills(data.path(), home.path());
        for sk in &skills {
            let erros: Vec<Finding> = lint_skill(sk).into_iter().filter(|f| f.nivel == "erro").collect();
            assert!(erros.is_empty(), "{}: {erros:?}", sk.name);
        }
        for name in METODOLOGIA {
            let sk = skills.iter().find(|s| s.name == name).expect(name);
            assert!(sk.description.starts_with("Use"), "{name} precisa começar pelo gatilho");
        }
        assert!(total_description_chars(&skills) > 0);
    }

    #[test]
    fn lint_flags_long_methodology_body_and_long_description() {
        let d = tmp();
        let dir = d.path().join("executar");
        std::fs::create_dir_all(&dir).unwrap();
        let corpo: String = (0..70).map(|i| format!("linha {i}\n")).collect();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: executar\n---\n{corpo}")).unwrap();
        let longa = "x".repeat(400);
        let sk = SkillDef { name: "executar".into(), description: longa, source: "regente".into(), dir: dir.to_string_lossy().into() };
        let texto: Vec<String> = lint_skill(&sk).into_iter().map(|f| format!("{}: {}", f.nivel, f.texto)).collect();
        assert!(texto.iter().any(|t| t.starts_with("erro: skill de metodologia")), "{texto:?}");
        assert!(texto.iter().any(|t| t.starts_with("aviso: descrição")), "{texto:?}");
    }

    #[test]
    fn lint_checks_the_trigger_and_reports_unreadable_skills() {
        let d = tmp();
        let dir = d.path().join("x");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: x\n---\ncorpo\n").unwrap();
        let sem_gatilho = SkillDef { name: "x".into(), description: "Faz coisas".into(), source: "regente".into(), dir: dir.to_string_lossy().into() };
        let texto: Vec<String> = lint_skill(&sem_gatilho).into_iter().map(|f| format!("{}: {}", f.nivel, f.texto)).collect();
        assert!(texto.iter().any(|t| t.starts_with("aviso: descrição sem gatilho")), "{texto:?}");
        let com_gatilho = SkillDef { description: "Use quando faz coisas".into(), ..sem_gatilho.clone() };
        assert!(lint_skill(&com_gatilho).is_empty());
        // a pasta sumiu entre a listagem e a checagem: é um achado, não um zero silencioso
        let sumida = SkillDef { dir: d.path().join("nao-existe").to_string_lossy().into(), ..com_gatilho };
        let achados = lint_skill(&sumida);
        assert!(achados.iter().any(|f| f.nivel == "erro" && f.texto.contains("não consegui ler")), "{achados:?}");
    }

    #[test]
    fn names_are_validated_before_touching_disk() {
        let (data, home) = (tmp(), tmp());
        seed(data.path()).unwrap();
        assert!(get_agent(data.path(), home.path(), "../x").is_none());
        assert!(find_skill(data.path(), home.path(), "..").is_none());
        let bad = AgentDef { name: "../evil".into(), ..Default::default() };
        assert!(save_agent(data.path(), &bad).is_err());
    }

    fn agent(cli: Option<&str>, model: Option<&str>, effort: Option<&str>) -> AgentDef {
        AgentDef { name: "a".into(), cli: cli.map(Into::into), model: model.map(Into::into), effort: effort.map(Into::into), ..Default::default() }
    }

    #[test]
    fn papel_do_usuario_vence_o_agente_e_o_pedido_do_orquestrador() {
        let slot = HarnessArgs { cli: Some("codex".into()), model: Some("gpt-5.3-codex-spark".into()), effort: Some("low".into()) };
        let opus = agent(Some("claude"), Some("claude-opus-5-5"), Some("high"));
        let pedido = HarnessArgs { model: Some("claude-opus-5-5".into()), ..Default::default() };
        let h = resolve_harness(Some(&slot), &pedido, Some(&opus), &HarnessArgs::default(), "claude");
        assert_eq!((h.cli.as_str(), h.model.as_deref(), h.effort.as_deref()), ("codex", Some("gpt-5.3-codex-spark"), Some("low")));
        // time montado pelo usuário com modelo: ele inteiro vence o slot
        let time = HarnessArgs { model: Some("claude-haiku-5-5".into()), ..Default::default() };
        assert_eq!(sobre(&time, &slot), time);
        // só esforço no time: junta com o resto do slot
        let esforco = HarnessArgs { effort: Some("high".into()), ..Default::default() };
        assert_eq!(sobre(&esforco, &slot), HarnessArgs { effort: Some("high".into()), ..slot.clone() });
        // o perfil escolhe a CLI quando nada acima escolhe
        let h = resolve_harness(None, &HarnessArgs::default(), None, &slot, "claude");
        assert_eq!((h.cli.as_str(), h.model.as_deref()), ("codex", Some("gpt-5.3-codex-spark")));
    }

    #[test]
    fn harness_precedence() {
        let roster = HarnessArgs { model: Some("X".into()), ..Default::default() };
        let arg = HarnessArgs { model: Some("Y".into()), ..Default::default() };
        let ag = agent(Some("codex"), Some("Z"), Some("high"));
        let h = resolve_harness(Some(&roster), &arg, Some(&ag), &HarnessArgs::default(), "claude");
        assert_eq!(h.model.as_deref(), Some("X"));
        assert!(h.source.contains("roster"), "{}", h.source);
        assert_eq!(h.cli, "codex");

        let h = resolve_harness(None, &arg, Some(&ag), &HarnessArgs::default(), "claude");
        assert_eq!(h.model.as_deref(), Some("Y"));
        let h = resolve_harness(None, &HarnessArgs::default(), Some(&ag), &HarnessArgs::default(), "claude");
        assert_eq!(h.model.as_deref(), Some("Z"));
        assert_eq!(h.effort.as_deref(), Some("high"));

        let h = resolve_harness(None, &HarnessArgs::default(), None, &HarnessArgs::default(), "claude");
        assert_eq!(h.model, None);
        assert_eq!(h.cli, "claude");
    }

    #[test]
    fn profile_slot_sits_under_the_agent_and_only_for_claude() {
        let slot = HarnessArgs { cli: None, model: Some("claude-haiku-5-5".into()), effort: Some("low".into()) };
        let ag = agent(None, Some("Z"), None);
        // agente vence o slot; sem agente, o slot entra
        let h = resolve_harness(None, &HarnessArgs::default(), Some(&ag), &slot, "claude");
        assert_eq!(h.model.as_deref(), Some("Z"));
        assert_eq!(h.effort.as_deref(), Some("low"));
        assert!(h.source.contains("esforço: perfil"), "{}", h.source);
        let h = resolve_harness(None, &HarnessArgs::default(), None, &slot, "claude");
        assert_eq!(h.model.as_deref(), Some("claude-haiku-5-5"));
        // arg explícito vence o slot
        let arg = HarnessArgs { model: Some("X".into()), ..Default::default() };
        assert_eq!(resolve_harness(None, &arg, None, &slot, "claude").model.as_deref(), Some("X"));
        // CLI não-claude: o slot não se aplica
        let codex = HarnessArgs { cli: Some("codex".into()), ..Default::default() };
        let h = resolve_harness(None, &codex, None, &slot, "claude");
        assert_eq!((h.cli.as_str(), h.model), ("codex", None));
    }

    #[test]
    fn pane_skills_rules() {
        let ag = AgentDef { skills: vec!["x".into()], ..Default::default() };
        assert_eq!(pane_skills(Some("piloto"), Some(&ag), None), ["orquestrar", "x"]);
        assert!(pane_skills(Some("piloto"), Some(&ag), Some(&[])).is_empty());
        assert_eq!(pane_skills(Some("piloto"), Some(&ag), Some(&["y".to_string()])), ["y"]);
    }

    #[test]
    fn plugin_dir_has_manifest_and_copies_skills() {
        let (data, home, pane) = (tmp(), tmp(), tmp());
        seed(data.path()).unwrap();
        let (dir, missing) = build_plugin_dir(data.path(), home.path(), pane.path(), "abcdef0123456789", &["executar".into(), "nao-existe".into()]).unwrap();
        let dir = dir.expect("plugin dir");
        assert!(dir.join("skills/executar/SKILL.md").is_file());
        let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest["name"], "regente-abcdef01");
        assert_eq!(missing, ["nao-existe"]);
        let (text, none) = skills_text(data.path(), home.path(), &["executar".into(), "x-nao".into()]);
        assert!(text.contains("## Skill: executar") && none == ["x-nao"]);
    }

    #[test]
    fn role_skill_map() {
        assert_eq!(role_skill("piloto"), Some("orquestrar"));
        assert_eq!(role_skill("chefe:reviewer"), Some("revisar"));
        assert_eq!(role_skill("planner"), Some("planejar"));
        assert_eq!(role_skill("tester"), Some("testar"));
        assert_eq!(role_skill("chefe:tester"), Some("testar"));
        assert_eq!(role_skill("nada"), None);
    }
}
