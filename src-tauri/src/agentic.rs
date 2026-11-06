//! Modo agêntico: tipos de tarefa ("Quem faz cada tarefa"), o Decisor que classifica um pedido com um modelo
//! pequeno e barato, e a escolha automática de agente. Config em `data/agentic.json` (semeada do binário).

use crate::catalog::{self, valid_name, HarnessArgs};
use crate::cost::prices::Prices;
use crate::cost::Usage;
use crate::db::{Mission, PaneRow, Workspace};
use crate::error::{fail, AppError, AppResult};
use crate::mcp::tools_panes::plain_arg;
use crate::missions::{self, MissionOpts};
use crate::orchestrator::clean_text;
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::settings::save_json;
use crate::util::{lk, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const DEFAULT: &str = include_str!("../seed/agentic.json");

/// Modelo do Decisor quando a config não diz um (o menor da casa).
pub const DECISOR_MODEL: &str = "claude-haiku-5-5";
/// Largest Decisor reply read, in bytes.
const REPLY_MAX: usize = 256 * 1024;
/// Descriptions in the Decisor prompt, and how many agents it lists.
const DESC_MAX: usize = 300;
const AGENTS_MAX: usize = 40;
/// Largest request text sent to the Decisor or stored on its decision.
pub const REQUEST_MAX: usize = 4000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskType {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub examples: Vec<String>,
    pub not_examples: Vec<String>,
    pub cli: String,
    /// Override do tipo. None = segue o slot do perfil (`perfil`) ou o padrão da CLI.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// "auto" | nome do agente.
    pub agent: String,
    pub account: Option<String>,
    pub skills: Vec<String>,
    /// Slot do perfil de trabalho (`planejar`, `implementar`, `revisar`, `testar`, `explorar`); None = sem slot.
    pub perfil: Option<String>,
}

/// Slots válidos de `TaskType::perfil`.
const SLOTS: [&str; 5] = ["planejar", "implementar", "revisar", "testar", "explorar"];

/// Migração do seed antigo: (tipo, slot, modelo, esforço) do seed anterior ao perfil. Um tipo sem `perfil` que ainda
/// tem o modelo e o esforço do seed antigo passa a seguir o perfil; um tipo editado mantém o override.
const SEED_ANTIGO: [(&str, &str, &str, &str); 9] = [
    ("piloto", "planejar", "claude-opus-5-5", "high"),
    ("pesquisa", "explorar", "claude-sonnet-5-5", "medium"),
    ("plano", "planejar", "claude-opus-5-5", "high"),
    ("frontend", "implementar", "claude-sonnet-5-5", "high"),
    ("backend", "implementar", "claude-opus-5-5", "high"),
    ("testes", "testar", "claude-sonnet-5-5", "medium"),
    ("revisao", "revisar", "claude-opus-5-5", "high"),
    ("docs", "implementar", "claude-haiku-5-5", "low"),
    ("mecanico", "explorar", "claude-haiku-5-5", "low"),
];

/// Aplica a migração de `SEED_ANTIGO` a uma config carregada (só tipos de CLI `claude` sem `perfil`).
fn migrate_perfil(c: &mut AgenticConfig) {
    for t in c.types.iter_mut().filter(|t| t.perfil.is_none() && t.cli == "claude") {
        let Some((_, slot, model, effort)) = SEED_ANTIGO.iter().find(|r| r.0 == t.id) else { continue };
        if t.model.as_deref() == Some(model) && t.effort.as_deref() == Some(effort) {
            t.model = None;
            t.effort = None;
        }
        t.perfil = Some((*slot).into());
    }
}

/// O slot do perfil que o tipo segue, inteiro (CLI, modelo e esforço). O override do tipo (`model`/`effort`) vem antes,
/// como argumento; a CLI do tipo só vale quando ele tem modelo próprio (`task_cli`).
pub fn perfil_harness(perfil: &crate::perfil::Perfil, task: &TaskType) -> HarnessArgs {
    task.perfil.as_deref().and_then(|s| crate::perfil::slot_of(perfil, s)).map(crate::perfil::slot_args).unwrap_or_default()
}

/// A CLI que o tipo pede explicitamente: só quando ele tem modelo próprio (override) ou não segue um slot do perfil.
/// O tipo que segue o perfil usa a CLI do slot (o perfil pode mandar a pesquisa para o Gemini e a construção para o Codex).
pub fn task_cli(task: &TaskType) -> Option<String> {
    let segue_perfil = task.perfil.is_some() && task.model.is_none();
    Some(task.cli.clone()).filter(|c| !c.trim().is_empty() && !segue_perfil)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DecisorConfig {
    /// none | openai | anthropic
    pub kind: String,
    pub base_url: String,
    pub model: String,
    /// Nome do segredo no cofre.
    pub secret: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AgenticConfig {
    /// types[0] é o piloto.
    pub types: Vec<TaskType>,
    pub choose_agent: bool,
    pub decisor: DecisorConfig,
    /// chain | torre | paralelo
    pub default_style: String,
}

/// The config on disk; seeded from the binary when missing. A file that does not parse or fails `validate`
/// falls back to the seed (with a warning).
pub fn load(data: &Path) -> AgenticConfig {
    let default = || serde_json::from_str::<AgenticConfig>(DEFAULT).expect("valid default");
    match crate::settings::load_or_seed::<AgenticConfig>(&data.join("agentic.json"), DEFAULT) {
        Ok(mut c) => match validate(&c) {
            Ok(()) => {
                migrate_perfil(&mut c);
                c
            }
            Err(e) => {
                tracing::warn!("agentic.json rejeitado ({e}): usando o padrão");
                default()
            }
        },
        Err(e) => {
            tracing::warn!("agentic.json ilegível ({e}): usando o padrão");
            default()
        }
    }
}

/// The Decisor's endpoint: https, or http only for loopback (the key must not travel in clear text).
pub fn check_base_url(raw: &str) -> AppResult<()> {
    let url = reqwest::Url::parse(raw).map_err(|_| AppError::new("bad_decisor_url", "URL do Decisor inválida"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => fail("bad_decisor_url", "a URL do Decisor precisa ser https (http só em localhost)"),
    }
}

/// One HTTP client for every Decisor call: a timeout, and no redirects (the key never follows one).
pub(crate) fn client() -> AppResult<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::new("decisor_failed", e.to_string()))?;
    Ok(CLIENT.get_or_init(|| c))
}

/// Text sent to the Decisor: control chars out, at most `REQUEST_MAX` chars.
pub fn clean_request(text: &str) -> String {
    clean_text(text).chars().take(REQUEST_MAX).collect()
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn save(data: &Path, c: &AgenticConfig) -> AppResult<()> {
    validate(c)?;
    std::fs::create_dir_all(data)?;
    save_json(&data.join("agentic.json"), c)
}

fn validate(c: &AgenticConfig) -> AppResult<()> {
    if c.types.is_empty() || c.types.len() > 30 {
        return fail("bad_types", "são de 1 a 30 tipos de tarefa (o primeiro é o piloto)");
    }
    let mut seen = HashSet::new();
    for t in &c.types {
        if !valid_name(&t.id) || !seen.insert(t.id.as_str()) {
            return fail("bad_name", format!("id de tipo inválido ou repetido: {}", t.id));
        }
        if t.agent != "auto" && !valid_name(&t.agent) {
            return fail("bad_name", format!("agente inválido no tipo {}: {}", t.id, t.agent));
        }
        if t.skills.iter().any(|s| !valid_name(s)) {
            return fail("bad_name", format!("skill inválida no tipo {}", t.id));
        }
        if t.perfil.as_deref().is_some_and(|p| !SLOTS.contains(&p)) {
            return fail("bad_perfil", format!("slot de perfil inválido no tipo {}", t.id));
        }
        // these values reach the CLI command line
        for v in [Some(t.cli.as_str()), t.model.as_deref(), t.effort.as_deref()].into_iter().flatten() {
            crate::mcp::tools_panes::plain_arg(v)?;
        }
    }
    if !matches!(c.decisor.kind.as_str(), "none" | "openai" | "anthropic") {
        return fail("bad_decisor", "Decisor: use none, openai ou anthropic");
    }
    if !c.decisor.base_url.trim().is_empty() {
        check_base_url(c.decisor.base_url.trim())?;
    }
    if !matches!(c.default_style.as_str(), "chain" | "torre" | "paralelo") {
        return fail("bad_style", "estilo padrão: chain, torre ou paralelo");
    }
    Ok(())
}

/// Messages for the Decisor: the system text describes each type and asks for a JSON reply; the user message is the request.
pub fn classify_messages(types: &[TaskType], agents: &[(String, String)], request: &str) -> (String, String) {
    let mut sys = String::from("Você classifica um pedido de desenvolvimento em um tipo de tarefa. Tipos:\n");
    for t in types {
        sys += &format!("- {} ({}): {}\n", t.id, t.name, cut(&t.description, DESC_MAX));
        if !t.examples.is_empty() {
            sys += &format!("  exemplos: {}\n", t.examples.join("; "));
        }
        if !t.not_examples.is_empty() {
            sys += &format!("  não é deste tipo: {}\n", t.not_examples.join("; "));
        }
    }
    if !agents.is_empty() {
        sys += "Agentes disponíveis (escolha pelo nome, ou null):\n";
        for (name, desc) in agents.iter().take(AGENTS_MAX) {
            sys += &format!("- {name}: {}\n", cut(desc, DESC_MAX));
        }
    }
    sys += "Responda somente com JSON: {\"task_type\": \"<id>\", \"agent\": \"<nome>\" ou null}.";
    (sys, clean_request(request))
}

/// Tolerant: takes the first JSON object in the text; accepts task_type|tipo and agent|agente; the type must exist.
pub fn parse_decision(text: &str, types: &[TaskType]) -> Option<(String, Option<String>)> {
    let obj = text.match_indices('{').find_map(|(i, _)| match serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>().next() {
        Some(Ok(v @ Value::Object(_))) => Some(v),
        _ => None,
    })?;
    let task = obj.get("task_type").or_else(|| obj.get("tipo"))?.as_str()?;
    if !types.iter().any(|t| t.id == task) {
        return None;
    }
    let agent = obj.get("agent").or_else(|| obj.get("agente")).and_then(Value::as_str).map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
    Some((task.to_string(), agent))
}

#[derive(Clone)]
pub struct DecisorOut {
    /// None when the reply names no valid type: the call still cost money, so the caller records it.
    pub task_type: Option<String>,
    pub agent: Option<String>,
    pub cost_usd: f64,
    /// The provider reported the cost (true), or it was calculated from the price table (false).
    pub measured: bool,
    pub model: String,
    pub latency_ms: u128,
    pub tok_in: u64,
    pub tok_out: u64,
}

/// Usage of a Decisor call in the day's totals (cli "decisor").
pub fn account(db: &crate::db::Db, out: &DecisorOut) -> AppResult<()> {
    db.add_usage(&crate::util::today(), "decisor", &out.model, "-", out.tok_in as i64, out.tok_out as i64, out.cost_usd, 0)
}

/// The JSON body of a reply, read at most `REPLY_MAX` bytes.
/// `who` names the provider in the error ("o Decisor", "a IA auxiliar"), so the message says which one failed.
pub(crate) async fn send(req: reqwest::RequestBuilder, who: &str) -> AppResult<Value> {
    let mut resp = req.send().await.map_err(|e| AppError::new("decisor_failed", format!("{who}: falha de rede: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return fail("decisor_failed", format!("{who} respondeu HTTP {}", status.as_u16()));
    }
    if resp.content_length().is_some_and(|n| n > REPLY_MAX as u64) {
        return fail("decisor_failed", format!("resposta de {who} acima de 256 KB"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::new("decisor_failed", e.to_string()))? {
        if body.len() + chunk.len() > REPLY_MAX {
            return fail("decisor_failed", format!("resposta de {who} acima de 256 KB"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| AppError::new("decisor_failed", e.to_string()))
}

/// Classifies a request with the configured Decisor. `openai`: `{base}/chat/completions`, cost from `usage.cost` when present.
/// `anthropic`: `{base}/v1/messages`, cost from the price table.
pub async fn decide(cfg: &DecisorConfig, secret: Option<String>, prices: &Prices, types: &[TaskType], agents: &[(String, String)], request: &str) -> AppResult<DecisorOut> {
    let client = client()?;
    if !cfg.base_url.trim().is_empty() {
        check_base_url(cfg.base_url.trim())?;
    }
    let (system, user) = classify_messages(types, agents, request);
    let model = if cfg.model.trim().is_empty() { DECISOR_MODEL.to_string() } else { cfg.model.clone() };
    let t0 = Instant::now();
    let (text, usage, reported) = match cfg.kind.as_str() {
        "openai" => {
            let base = cfg.base_url.trim_end_matches('/');
            let body = json!({
                "model": model,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
                "response_format": {"type": "json_object"},
                "usage": {"include": true},
            });
            let mut req = client.post(format!("{base}/chat/completions")).json(&body);
            if let Some(k) = &secret {
                req = req.bearer_auth(k);
            }
            let v = send(req, "o Decisor").await?;
            let text = v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
            let u = &v["usage"];
            let usage = Usage { input: u["prompt_tokens"].as_u64().unwrap_or(0), output: u["completion_tokens"].as_u64().unwrap_or(0), ..Default::default() };
            (text, usage, u["cost"].as_f64())
        }
        "anthropic" => {
            let base = if cfg.base_url.trim().is_empty() { "https://api.anthropic.com" } else { cfg.base_url.trim_end_matches('/') };
            let body = json!({
                "model": model,
                "max_tokens": 300,
                "system": system,
                "messages": [{"role": "user", "content": user}],
                "output_config": {"effort": "low"},
            });
            let mut req = client.post(format!("{base}/v1/messages")).header("anthropic-version", "2023-06-01").json(&body);
            if let Some(k) = &secret {
                req = req.header("x-api-key", k);
            }
            let v = send(req, "o Decisor").await?;
            let text = v["content"].as_array().and_then(|c| c.iter().find_map(|b| b["text"].as_str())).unwrap_or_default().to_string();
            let u = &v["usage"];
            let usage = Usage {
                input: u["input_tokens"].as_u64().unwrap_or(0),
                output: u["output_tokens"].as_u64().unwrap_or(0),
                cache_read: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
                cache_write: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                cache_write_1h: u["cache_creation"]["ephemeral_1h_input_tokens"].as_u64().unwrap_or(0),
            };
            (text, usage, None)
        }
        _ => return fail("decisor_off", "o Decisor está desligado"),
    };
    let latency_ms = t0.elapsed().as_millis();
    // an invalid reply is still returned, with its cost: the caller records the spend
    let (task_type, agent) = match parse_decision(&text, types) {
        Some((t, a)) => (Some(t), a),
        None => (None, None),
    };
    let (cost_usd, measured) = match reported {
        Some(c) => (c, true),
        None => (prices.cost(&model, &usage), false),
    };
    Ok(DecisorOut { task_type, agent, cost_usd, measured, model, latency_ms, tok_in: usage.input, tok_out: usage.output })
}

/// Lowercase, no accents, words of at least 4 letters.
fn words(text: &str) -> HashSet<String> {
    let folded: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' => 'a',
            'é' | 'ê' => 'e',
            'í' => 'i',
            'ó' | 'ô' | 'õ' => 'o',
            'ú' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect();
    folded.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).map(str::to_string).collect()
}

/// The installed agent that shares the most words with the request and the type's description (at least 2), else None.
pub fn auto_agent(agents: &[(String, String)], task: &TaskType, request: &str) -> Option<String> {
    let want = words(&format!("{request} {}", task.description));
    let mut best: Option<(usize, &str)> = None;
    for (name, desc) in agents {
        let have = words(&format!("{name} {desc}"));
        let score = want.intersection(&have).count();
        if score >= 2 && best.is_none_or(|(b, _)| score > b) {
            best = Some((score, name));
        }
    }
    best.map(|(_, n)| n.to_string())
}

const HANDOFF_SCHEMAS: &str = include_str!("../seed/handoff-schemas.json");

/// Schema de entrega padrão de um tipo de tarefa (`seed/handoff-schemas.json`). None = o tipo não tem schema.
pub fn handoff_schema_padrao(tipo: &str) -> Option<Value> {
    let mut m: serde_json::Map<String, Value> = serde_json::from_str(HANDOFF_SCHEMAS).ok()?;
    m.remove(tipo)
}

/// A IA e o modelo que um tipo de tarefa usa de fato: o override do tipo, senão o slot do perfil que ele segue.
fn ia_do_tipo(t: &TaskType, perfil: &crate::perfil::Perfil) -> String {
    match (&t.model, t.perfil.as_deref().and_then(|s| crate::perfil::slot_of(perfil, s))) {
        (Some(m), _) => format!("{} {m}", t.cli),
        (None, Some(s)) => format!("{} {} ({})", s.cli, s.model, s.effort),
        (None, None) => format!("{} padrão", t.cli),
    }
}

/// Text of the pilot's system prompt: the task table, the delegation rules and the chief rules of the style.
/// A tabela mostra a IA e o modelo reais de cada tipo e de cada chefe (o perfil decide; o piloto não escolhe modelo).
pub fn pilot_prompt(cfg: &AgenticConfig, style: &str, perfil: &crate::perfil::Perfil) -> String {
    let mut p = String::from("## Quem faz cada tarefa\n| tarefa | o que é | IA e modelo |\n|---|---|---|\n");
    for t in cfg.types.iter().skip(1) {
        p += &format!("| {} | {} | {} |\n", t.id, t.description, ia_do_tipo(t, perfil));
    }
    let pp = &perfil.papeis;
    let chefe = |s: &crate::perfil::Slot| format!("{} {} ({})", s.cli, s.model, s.effort);
    p += &format!(
        "\nChefes: scout {}, builder {}, reviewer {}{}. Quem escolhe a IA e o modelo é o usuário (perfil); não peça outro modelo.\n",
        chefe(&pp.scout),
        chefe(&pp.builder),
        chefe(&pp.reviewer),
        if style == "torre" { format!(", controller {}", chefe(&pp.controller)) } else { String::new() },
    );
    p += "\n## Como delegar\n- Delegue com `delegate`, um pedido por ajudante. Peça entregas pequenas.\n";
    match style {
        "torre" => p += "- Só o chefe controller despacha: fale com ele via `chief_invoke controller`.\n",
        "paralelo" => {
            p += &format!(
                "- Quebre o pedido em tarefas com dono e critério de aceite e registre cada uma no quadro (`task_create`), dizendo na \
                 descrição de quais outras ela depende.\n\
                 - Acione AO MESMO TEMPO todas as tarefas que não dependem de nenhuma outra, até {} de uma vez: um `delegate` para cada, \
                 sem esperar uma entrega para chamar a próxima.\n\
                 - Quando uma entrega chegar, marque a tarefa `feito` (`task_update`) e acione na hora as que ficaram livres.\n\
                 - Nunca dispare mais que isso: muitos agentes de uma vez gastam o limite sem ganho.\n\
                 - Com tudo entregue, `chief_invoke reviewer` revisa o conjunto.\n",
                perfil.paralelo_max.max(1)
            )
        }
        _ => p += "- Para trabalhos grandes, use `chief_invoke` na ordem scout → builder → reviewer.\n",
    }
    if let Some(c) = &pp.cruzado {
        p += &format!(
            "- Revisão cruzada ligada ({}): depois do veredito do reviewer, mande o relatório dele para `chief_invoke cruzado`. \
             Só trate como bug o que o revisor cruzado confirmar; o que ele chamar de falso positivo, descarte com o motivo.\n",
            chefe(c)
        );
    }
    p += &format!("\n{}\n", crate::artefatos::REGRA_PILOTO);
    p
}

/// The pilot's harness and extra account, as the wizard sends them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PilotArgs {
    pub cli: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub account: Option<String>,
}

/// Removes the mission's worktree after a failed start (a warning when git refuses).
fn drop_worktree(ws: &Workspace, m: &Mission) {
    if let Some(p) = m.worktree_path.as_deref() {
        if let Err(err) = missions::remove_worktree(Path::new(&ws.path), Path::new(p)) {
            tracing::warn!("worktree da missão {} não removida após falha: {err}", m.id);
        }
    }
}

/// An agentic mission: its only pane at the start is the pilot (role `piloto`). Missing harness fields come from the pilot type.
#[allow(clippy::too_many_arguments)]
pub fn start_agentic_mission(s: &AppState, ws: &Workspace, name: &str, pilot: &PilotArgs, brief: &str, style: &str, opts: &MissionOpts) -> AppResult<(Mission, PaneRow)> {
    let (recipe_id, worktree, profile) = (opts.recipe_id.as_deref(), opts.worktree, opts.profile.as_deref());
    if !matches!(style, "chain" | "torre" | "plano" | "paralelo") {
        return fail("bad_style", "estilo: chain, torre, plano ou paralelo");
    }
    if brief.trim().is_empty() {
        return fail("bad_brief", "escreva o brief da missão");
    }
    let cfg = load(&s.data);
    let pt = cfg.types.first().ok_or_else(|| AppError::new("bad_types", "agentic.json não tem o tipo piloto"))?;
    // o planejador ("plano") usa o slot `planejar` do perfil, não o tipo piloto
    let planejador = style == "plano";
    // o que o usuário escolheu no assistente vence; o override do tipo "piloto" (Configurações → Agêntico) vem depois;
    // sem nada disso, o papel `piloto` do perfil decide CLI, modelo e esforço juntos
    let arg = HarnessArgs {
        cli: pilot.cli.clone().or_else(|| if planejador { None } else { pt.model.as_ref().map(|_| pt.cli.clone()) }),
        model: pilot.model.clone().or_else(|| if planejador { None } else { pt.model.clone() }),
        effort: pilot.effort.clone().or_else(|| if planejador { None } else { pt.effort.clone() }),
    };
    let (perfil, _, _) = crate::perfil::resolve(&lk(&s.settings).perfis, &ws.id, profile);
    // o planejador ("plano") usa o slot `planejar`; o piloto, o papel `piloto`
    let padrao = if planejador {
        crate::perfil::slot_of(&perfil, "planejar").map(crate::perfil::slot_args).unwrap_or_default()
    } else {
        crate::perfil::slot_args(&perfil.papeis.piloto)
    };
    let h = catalog::resolve_harness(None, &arg, None, &padrao, "claude");
    for v in [Some(h.cli.as_str()), h.model.as_deref(), h.effort.as_deref()].into_iter().flatten() {
        plain_arg(v)?;
    }
    let account = pilot.account.clone().filter(|a| !a.trim().is_empty());
    if let Some(acc) = account.as_deref() {
        if acc != "principal" && !crate::providers::valid_account(acc) {
            return fail("bad_account", format!("conta inválida: {acc}"));
        }
    }
    let (mut m, cwd) = missions::new_livre(ws, name, worktree)?;
    m.mode = "agentico".into();
    m.style = Some(style.into());
    m.profile = profile.map(Into::into);
    m.brief = Some(brief.into());
    m.recipe_id = recipe_id.map(Into::into);
    if let Err(e) = s.db.insert_mission(&m) {
        drop_worktree(ws, &m);
        return Err(e);
    }
    crate::worktree_cache::acelerar(s, ws, &m);
    let spec = PaneSpec {
        workspace_id: ws.id.clone(),
        mission_id: Some(m.id.clone()),
        kind: PaneKind::Agent,
        name: Some(if planejador { "Planejador" } else { "Piloto" }.into()),
        cli: Some(h.cli),
        model: h.model,
        effort: h.effort,
        account,
        permission: Permission::Auto,
        cwd,
        prompt: Some(brief.into()),
        role: Some(if planejador { "planner" } else { "piloto" }.into()),
        append_system: Some(if planejador { crate::planexec::PLANNER_RULES.into() } else { pilot_prompt(&cfg, style, &perfil) }),
        mcp_profile: Some("full".into()),
        ..Default::default()
    };
    match s.spawn_pane(spec) {
        Ok(row) => {
            if planejador {
                crate::fases::abrir(s, &m.id, Some(&row.id), None, crate::fases::PLANEJAMENTO);
            }
            Ok((m, row))
        }
        Err(e) => {
            // as in a squad mission: a pilot that cannot start archives the mission and drops its worktree
            drop_worktree(ws, &m);
            m.status = "arquivada".into();
            m.archived_at = Some(now_ms());
            s.db.update_mission(&m)?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;

    fn types() -> Vec<TaskType> {
        serde_json::from_str::<AgenticConfig>(DEFAULT).unwrap().types
    }

    fn task(id: &str, desc: &str) -> TaskType {
        TaskType { id: id.into(), description: desc.into(), ..Default::default() }
    }

    #[test]
    fn parse_decision_is_tolerant_and_checks_the_type() {
        let t = types();
        assert_eq!(parse_decision("blá {\"task_type\":\"testes\",\"agent\":null} fim", &t), Some(("testes".into(), None)));
        assert_eq!(parse_decision("{\"tipo\":\"frontend\",\"agente\":\"sq-frontend\"}", &t), Some(("frontend".into(), Some("sq-frontend".into()))));
        assert_eq!(parse_decision("{\"task_type\":\"inventado\"}", &t), None);
        assert_eq!(parse_decision("sem json nenhum", &t), None);
    }

    #[test]
    fn classify_lists_every_type_and_example() {
        let t = types();
        let (sys, user) = classify_messages(&t, &[], "corrigir o botão");
        for ty in &t {
            assert!(sys.contains(&ty.id), "{}", ty.id);
            assert!(sys.contains(&ty.examples[0]), "{}", ty.examples[0]);
        }
        assert_eq!(user, "corrigir o botão");
    }

    #[test]
    fn auto_agent_needs_two_shared_words() {
        let agents = vec![
            ("sq-qa".to_string(), "verifica fluxos com testes automatizados".to_string()),
            ("sq-media".to_string(), "gera imagens e ícones".to_string()),
        ];
        let testes = task("testes", "Escreve e roda testes automatizados e verifica fluxos ponta a ponta.");
        assert_eq!(auto_agent(&agents, &testes, "criar testes e2e do checkout").as_deref(), Some("sq-qa"));
        // the type's own description shares words with sq-qa, so a generic request needs a type that does not
        let docs = types().into_iter().find(|t| t.id == "docs").unwrap();
        assert_eq!(auto_agent(&agents, &docs, "oi, tudo bem?"), None);
    }

    /// A local stand-in for the provider: the Decisor's HTTP call goes to 127.0.0.1 only.
    async fn mock(route: &'static str, reply: Value) -> String {
        let app = axum::Router::new().route(route, post(move || {
            let reply = reply.clone();
            async move { axum::Json(reply) }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://127.0.0.1:{port}")
    }

    fn prices() -> Prices {
        serde_json::from_str(crate::settings::PRICES_DEFAULT).unwrap()
    }

    #[tokio::test]
    async fn decide_openai_mock() {
        let reply = json!({"choices": [{"message": {"content": "{\"task_type\":\"docs\"}"}}], "usage": {"cost": 0.0001}});
        let base = mock("/chat/completions", reply).await;
        let cfg = DecisorConfig { kind: "openai".into(), base_url: base, model: "modelo-x".into(), secret: None };
        let out = decide(&cfg, None, &prices(), &types(), &[], "escrever o README").await.unwrap();
        assert_eq!(out.task_type.as_deref(), Some("docs"));
        assert!(out.measured);
        assert!((out.cost_usd - 0.0001).abs() < 1e-12);
        assert_eq!(out.model, "modelo-x");
    }

    #[tokio::test]
    async fn decide_anthropic_mock() {
        let reply = json!({
            "content": [{"type": "text", "text": "{\"task_type\":\"plano\"}"}],
            "usage": {"input_tokens": 100, "output_tokens": 10},
            "model": "claude-haiku-5-5"
        });
        let base = mock("/v1/messages", reply).await;
        let cfg = DecisorConfig { kind: "anthropic".into(), base_url: base, model: String::new(), secret: Some("k".into()) };
        let p = prices();
        let out = decide(&cfg, Some("k".into()), &p, &types(), &[], "planejar a migração").await.unwrap();
        assert_eq!(out.task_type.as_deref(), Some("plano"));
        assert!(!out.measured);
        assert_eq!(out.model, DECISOR_MODEL, "the small model is the default");
        let expected = p.cost(DECISOR_MODEL, &Usage { input: 100, output: 10, ..Default::default() });
        assert!((out.cost_usd - expected).abs() < 1e-12);
    }

    #[test]
    fn save_refuses_bad_config_and_load_seeds() {
        let d = tempfile::tempdir().unwrap();
        let mut c = load(d.path());
        assert!(d.path().join("agentic.json").exists(), "seeded when missing");
        assert_eq!(c.types.len(), 10);
        c.types[0].agent = "../fora".into();
        assert!(save(d.path(), &c).is_err());
        c.types[0].agent = "auto".into();
        c.decisor.kind = "gemini".into();
        assert!(save(d.path(), &c).is_err());
    }

    #[test]
    fn decisor_url_must_be_https_or_loopback() {
        assert!(check_base_url("https://api.exemplo.com/v1").is_ok());
        assert!(check_base_url("http://127.0.0.1:8080/v1").is_ok());
        assert!(check_base_url("http://localhost:1/v1").is_ok());
        assert!(check_base_url("http://api.exemplo.com").is_err(), "clear text to a remote host");
        assert!(check_base_url("ftp://x").is_err());
        assert!(check_base_url("não é url").is_err());
    }

    #[test]
    fn prompt_is_bounded() {
        let long = "x".repeat(1000);
        let t = TaskType { id: "t1".into(), name: "T".into(), description: long.clone(), ..Default::default() };
        let agents: Vec<(String, String)> = (0..50).map(|i| (format!("ag-{i}"), long.clone())).collect();
        let (sys, user) = classify_messages(&[t], &agents, &format!("oi\u{7}{}", "r".repeat(5000)));
        assert!(!sys.contains(&"x".repeat(301)), "descriptions cut at 300 chars");
        assert_eq!(sys.matches("- ag-").count(), AGENTS_MAX);
        assert!(!user.contains('\u{7}'));
        assert_eq!(user.chars().count(), REQUEST_MAX);
    }

    #[tokio::test]
    async fn invalid_reply_is_returned_with_its_cost() {
        let reply = json!({"choices": [{"message": {"content": "não sei"}}], "usage": {"cost": 0.002}});
        let base = mock("/chat/completions", reply).await;
        let cfg = DecisorConfig { kind: "openai".into(), base_url: base, model: "m".into(), secret: None };
        let out = decide(&cfg, None, &prices(), &types(), &[], "x").await.unwrap();
        assert_eq!(out.task_type, None);
        assert!((out.cost_usd - 0.002).abs() < 1e-12);
    }

    #[tokio::test]
    async fn oversized_reply_is_refused() {
        let big = "x".repeat(REPLY_MAX + 10);
        let reply = json!({"content": [{"type": "text", "text": big}]});
        let base = mock("/v1/messages", reply).await;
        let cfg = DecisorConfig { kind: "anthropic".into(), base_url: base, model: String::new(), secret: None };
        let err = decide(&cfg, None, &prices(), &types(), &[], "x").await.err().expect("refused");
        assert_eq!(err.code(), "decisor_failed");
    }
}

#[cfg(test)]
mod perfil_tests {
    use super::*;

    fn tipo(id: &str, cli: &str, model: Option<&str>, effort: Option<&str>) -> TaskType {
        TaskType { id: id.into(), cli: cli.into(), model: model.map(Into::into), effort: effort.map(Into::into), ..Default::default() }
    }

    #[test]
    fn untouched_seed_follows_the_profile_and_edited_keeps_its_override() {
        let mut c = AgenticConfig { types: vec![tipo("piloto", "claude", Some("claude-opus-5-5"), Some("high")), tipo("docs", "claude", Some("claude-haiku-5-5"), Some("medium"))], ..Default::default() };
        migrate_perfil(&mut c);
        // seed antigo intacto: vira "segue o perfil"
        assert_eq!((c.types[0].model.clone(), c.types[0].effort.clone()), (None, None));
        assert_eq!(c.types[0].perfil.as_deref(), Some("planejar"));
        // editado (esforço diferente do seed): mantém o override, mas ganha o slot
        assert_eq!(c.types[1].effort.as_deref(), Some("medium"));
        assert_eq!(c.types[1].perfil.as_deref(), Some("implementar"));
    }

    #[test]
    fn non_claude_and_unmapped_types_are_untouched() {
        let mut c = AgenticConfig {
            types: vec![tipo("piloto", "codex", Some("claude-opus-5-5"), Some("high")), tipo("midia", "claude", Some("claude-sonnet-5-5"), Some("medium")), tipo("novo", "claude", None, None)],
            ..Default::default()
        };
        migrate_perfil(&mut c);
        assert!(c.types.iter().all(|t| t.perfil.is_none()));
        assert_eq!(c.types[0].model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(c.types[1].model.as_deref(), Some("claude-sonnet-5-5"));
    }

    #[test]
    fn tipo_que_segue_o_perfil_usa_o_slot_inteiro_com_a_cli_dele() {
        let p = crate::perfil::preset("equilibrado").unwrap();
        let t = TaskType { perfil: Some("explorar".into()), ..tipo("pesquisa", "claude", None, None) };
        let h = perfil_harness(&p, &t);
        assert_eq!((h.cli.as_deref(), h.model.as_deref(), h.effort.as_deref()), (Some("claude"), Some("claude-sonnet-5-5"), Some("low")));
        // segue o perfil: a CLI do tipo não força nada
        assert_eq!(task_cli(&t), None);
        // tipo com modelo próprio: a CLI dele vale
        assert_eq!(task_cli(&TaskType { model: Some("gpt-5.5".into()), ..tipo("pesquisa", "codex", None, None) }).as_deref(), Some("codex"));
    }

    #[test]
    fn slot_de_outra_cli_manda_o_tipo_para_ela() {
        let mut p = crate::perfil::preset("equilibrado").unwrap();
        p.modelos.explorar = crate::perfil::Slot { cli: "gemini".into(), model: "gemini-3.5-flash".into(), effort: "low".into() };
        let t = TaskType { perfil: Some("explorar".into()), ..tipo("pesquisa", "claude", None, None) };
        let slot = perfil_harness(&p, &t);
        let h = catalog::resolve_harness(Some(&slot), &HarnessArgs { cli: task_cli(&t), ..Default::default() }, None, &slot, "claude");
        assert_eq!((h.cli.as_str(), h.model.as_deref()), ("gemini", Some("gemini-3.5-flash")));
    }

    #[test]
    fn piloto_ve_os_modelos_reais_e_as_regras_do_paralelo_e_da_revisao_cruzada() {
        let cfg: AgenticConfig = serde_json::from_str(DEFAULT).unwrap();
        let mut p = crate::perfil::preset("equilibrado").unwrap();
        let chain = pilot_prompt(&cfg, "chain", &p);
        assert!(chain.contains("| pesquisa |") && chain.contains("claude claude-sonnet-5-5 (low)"), "{chain}");
        assert!(chain.contains("scout → builder → reviewer"));
        assert!(!chain.contains("cruzado"));
        p.paralelo_max = 4;
        p.papeis.cruzado = Some(crate::perfil::Slot { cli: "codex".into(), model: "gpt-5.5".into(), effort: "medium".into() });
        let par = pilot_prompt(&cfg, "paralelo", &p);
        assert!(par.contains("AO MESMO TEMPO") && par.contains("até 4 de uma vez"), "{par}");
        assert!(par.contains("chief_invoke cruzado") && par.contains("codex gpt-5.5"), "{par}");
        assert!(!par.contains("scout → builder → reviewer"));
    }

    #[test]
    fn seed_types_all_have_a_valid_slot_and_midia_keeps_its_model() {
        let c: AgenticConfig = serde_json::from_str(DEFAULT).unwrap();
        for t in c.types.iter().filter(|t| t.id != "midia") {
            let slot = t.perfil.as_deref().expect("slot");
            assert!(SLOTS.contains(&slot), "{slot}");
            assert_eq!((t.model.clone(), t.effort.clone()), (None, None), "{}", t.id);
        }
        let midia = c.types.iter().find(|t| t.id == "midia").unwrap();
        assert_eq!((midia.perfil.clone(), midia.model.as_deref()), (None, Some("claude-sonnet-5-5")));
    }

    #[test]
    fn seed_handoff_schemas_accept_good_and_refuse_bad_samples() {
        let valido = |tipo: &str, v: Value| jsonschema::validator_for(&handoff_schema_padrao(tipo).unwrap()).unwrap().is_valid(&v);
        assert!(valido("revisao", json!({"verdict": "ok", "issues": [{"file": "a.rs", "line": 3, "note": "nome ruim"}]})));
        assert!(!valido("revisao", json!({"verdict": "talvez"})));
        assert!(valido("pesquisa", json!({"findings": ["achado"], "sources": ["https://x"]})));
        assert!(!valido("pesquisa", json!({"findings": vec!["a"; 8]})));
        assert!(valido("frontend", json!({"changed": ["src/a.tsx"], "tests": "3 ok", "notes": "nada"})));
        assert!(!valido("backend", json!({"tests": "sem lista de alterados"})));
        assert!(valido("testes", json!({"passed": 12, "failed": [], "fixed": ["x"]})));
        assert!(!valido("testes", json!({"passed": "doze"})));
        // tipo sem schema padrão: nada é validado
        assert!(handoff_schema_padrao("piloto").is_none());
    }
}
