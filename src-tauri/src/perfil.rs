//! Perfil de trabalho: o conjunto de modelos, esforço, contexto e limites de saída que um painel ou missão usa.
//! Os presets vêm de `resources/perfis.default.json`. O usuário edita um preset inteiro (`editados`) e ajusta
//! um workspace com um merge-patch (`patches`, locais e fora do git). Quem consome o perfil (H1a, H2, X1) só lê.

use crate::error::{fail, AppError, AppResult};
use crate::providers::CliDef;
use crate::runtime::AppState;
use crate::settings::save_json;
use crate::util::lk;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const PERFIS_DEFAULT: &str = include_str!("../resources/perfis.default.json");
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Ids fixos dos presets.
pub const IDS: [&str; 3] = ["economico", "equilibrado", "precisao"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Perfil {
    pub id: String,
    pub nome: String,
    pub modelos: Modelos,
    /// Quem faz o quê na missão agêntica: CLI, modelo e esforço de cada papel (piloto, chefes, revisor cruzado).
    /// Vence o arquivo do agente e o pedido do orquestrador: é a escolha do usuário.
    pub papeis: Papeis,
    /// Degraus de escalonamento, do mais barato ao mais caro: `modelo:esforço`.
    pub escada: Vec<String>,
    pub escalar_ao_falhar: bool,
    pub contexto: Contexto,
    pub cache_ttl_longos: String,
    pub mcp: Mcp,
    pub saidas: Saidas,
    pub testes: Testes,
    pub disciplina_executor: bool,
    pub artefatos: Artefatos,
    pub indice_codigo: bool,
    pub subagente_modelo: Option<String>,
    pub estilo_conciso: bool,
    pub paralelo_max: u32,
}

impl Default for Perfil {
    fn default() -> Self {
        Self {
            id: "equilibrado".into(),
            nome: "Equilibrado".into(),
            modelos: Modelos::default(),
            papeis: Papeis::default(),
            escada: ["claude-haiku-5-5:low", "claude-sonnet-5-5:low", "claude-sonnet-5-5:medium", "claude-opus-5-5:high"].map(String::from).to_vec(),
            escalar_ao_falhar: true,
            contexto: Contexto::default(),
            cache_ttl_longos: "1h".into(),
            mcp: Mcp::default(),
            saidas: Saidas::default(),
            testes: Testes::default(),
            disciplina_executor: true,
            artefatos: Artefatos::default(),
            indice_codigo: false,
            subagente_modelo: None,
            estilo_conciso: false,
            paralelo_max: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Modelos {
    pub planejar: Slot,
    pub implementar: Slot,
    pub revisar: Slot,
    pub testar: Slot,
    pub explorar: Slot,
}

impl Default for Modelos {
    fn default() -> Self {
        Self {
            planejar: slot("claude-opus-5-5", "high"),
            implementar: slot("claude-haiku-5-5", "low"),
            revisar: slot("claude-sonnet-5-5", "high"),
            testar: slot("claude-sonnet-5-5", "low"),
            explorar: slot("claude-sonnet-5-5", "low"),
        }
    }
}

/// Os papéis da missão agêntica. Os executores que os chefes acionam seguem os slots (`slot_do_agente`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Papeis {
    pub piloto: Slot,
    pub scout: Slot,
    pub builder: Slot,
    pub reviewer: Slot,
    pub controller: Slot,
    /// Segunda opinião na revisão: outra CLI confere cada achado do reviewer. None = desligada.
    pub cruzado: Option<Slot>,
}

impl Default for Papeis {
    fn default() -> Self {
        Self {
            piloto: slot("claude-opus-5-5", "high"),
            scout: slot("claude-sonnet-5-5", "low"),
            builder: slot("claude-sonnet-5-5", "medium"),
            reviewer: slot("claude-sonnet-5-5", "high"),
            controller: slot("claude-sonnet-5-5", "low"),
            cruzado: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Slot {
    #[serde(default = "claude")]
    pub cli: String,
    pub model: String,
    pub effort: String,
}

impl Default for Slot {
    fn default() -> Self {
        slot("claude-haiku-5-5", "low")
    }
}

fn claude() -> String {
    "claude".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Contexto {
    /// `CLAUDE_CODE_AUTO_COMPACT_WINDOW` para papéis longos.
    pub janela_longos: u64,
    pub payback_k: u32,
    pub restante_min: u32,
    /// auto | propor
    pub recomecar: String,
    pub nativa_ociosa: bool,
}

impl Default for Contexto {
    fn default() -> Self {
        Self { janela_longos: 400_000, payback_k: 3, restante_min: 5, recomecar: "auto".into(), nativa_ociosa: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Mcp {
    pub ajudantes: String,
    pub filtrar_por_papel: bool,
}

impl Default for Mcp {
    fn default() -> Self {
        Self { ajudantes: "minimal".into(), filtrar_por_papel: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Saidas {
    /// None = padrão do CLI (sem variável de ambiente).
    pub bash_max: Option<u32>,
    pub leitura_max: Option<u32>,
    pub resumir_acima: u32,
    pub digest_falhas: bool,
}

impl Default for Saidas {
    fn default() -> Self {
        Self { bash_max: Some(20_000), leitura_max: Some(15_000), resumir_acima: 12_000, digest_falhas: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Testes {
    pub relacionados_primeiro: bool,
    pub completo_no_fim: bool,
}

impl Default for Testes {
    fn default() -> Self {
        Self { relacionados_primeiro: true, completo_no_fim: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Artefatos {
    pub modo: String,
    pub formato: String,
}

impl Default for Artefatos {
    fn default() -> Self {
        Self { modo: "referencia".into(), formato: "auto".into() }
    }
}

/// Preferências de perfil em `settings.json`. `editados` e `patches` são guardados por id e por workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PerfisPrefs {
    /// Perfil padrão de painéis e missões sem perfil próprio.
    pub padrao: String,
    /// Preset editado pelo usuário, inteiro (um `Perfil` completo), por id.
    pub editados: BTreeMap<String, Value>,
    /// Ajuste por workspace, em JSON merge-patch sobre o perfil efetivo.
    pub patches: BTreeMap<String, Value>,
}

impl Default for PerfisPrefs {
    fn default() -> Self {
        Self { padrao: "equilibrado".into(), editados: BTreeMap::new(), patches: BTreeMap::new() }
    }
}

impl PerfisPrefs {
    /// Corrige o que não é de confiança: padrão desconhecido volta a `equilibrado`; edições e patches que não
    /// viram um `Perfil` válido em todos os presets somem.
    pub fn clamp(&mut self) {
        if !IDS.contains(&self.padrao.as_str()) {
            self.padrao = "equilibrado".into();
        }
        self.editados.retain(|id, v| validate_editado(id, v).is_ok());
        let editados = &self.editados;
        self.patches.retain(|_, p| patch_valido(p, editados));
    }
}

/// Os três presets, na ordem do arquivo.
pub fn presets() -> Vec<Perfil> {
    serde_json::from_str(PERFIS_DEFAULT).expect("perfis.default.json válido")
}

pub fn preset(id: &str) -> Option<Perfil> {
    presets().into_iter().find(|p| p.id == id)
}

fn slot(model: &str, effort: &str) -> Slot {
    Slot { cli: "claude".into(), model: model.into(), effort: effort.into() }
}

/// Preset `equilibrado` lido do JSON (a fonte dos presets). `Perfil::default` repete esses valores à mão, porque
/// desserializar um `Perfil` pode chamar `default`; um teste confere que os dois batem.
pub fn preset_default() -> Perfil {
    preset("equilibrado").expect("preset equilibrado")
}

fn preset_value(id: &str) -> Value {
    preset(id).and_then(|p| serde_json::to_value(p).ok()).unwrap_or(Value::Null)
}

fn known_cli(id: &str) -> bool {
    serde_json::from_str::<Value>(crate::settings::PROVIDERS_DEFAULT).ok().and_then(|v| v["clis"].as_array().map(|a| a.iter().any(|c| c["id"] == id))).unwrap_or(false)
}

/// Nome de modelo ou de CLI que pode ir a uma linha de comando (`[A-Za-z0-9._:/-]`, 1 a 64 chars).
fn model_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c))
}

fn bad(id: &str, msg: impl std::fmt::Display) -> AppResult<()> {
    fail("bad_perfil", format!("perfil {id}: {msg}"))
}

impl Perfil {
    /// Regras de valor: enumerações, charset de modelo e limites numéricos. Serve a edições, patches e resolução.
    pub fn validate(&self) -> AppResult<()> {
        let id = self.id.as_str();
        let pp = &self.papeis;
        let mut slots = vec![("planejar", &self.modelos.planejar), ("implementar", &self.modelos.implementar), ("revisar", &self.modelos.revisar), ("testar", &self.modelos.testar), ("explorar", &self.modelos.explorar)];
        slots.extend([("piloto", &pp.piloto), ("scout", &pp.scout), ("builder", &pp.builder), ("reviewer", &pp.reviewer), ("controller", &pp.controller)]);
        if let Some(c) = &pp.cruzado {
            slots.push(("revisor cruzado", c));
        }
        for (nome, s) in slots {
            if !EFFORTS.contains(&s.effort.as_str()) {
                return bad(id, format!("esforço inválido em {nome}: {}", s.effort));
            }
            if !model_ok(&s.model) {
                return bad(id, format!("modelo inválido em {nome}"));
            }
            if !known_cli(&s.cli) {
                return bad(id, format!("CLI desconhecida em {nome}: {}", s.cli));
            }
        }
        for degrau in &self.escada {
            match degrau.split_once(':') {
                Some((m, e)) if model_ok(m) && EFFORTS.contains(&e) => {}
                _ => return bad(id, format!("degrau da escada inválido: {degrau}")),
            }
        }
        if !["5m", "1h"].contains(&self.cache_ttl_longos.as_str()) {
            return bad(id, "cacheTtlLongos: 5m ou 1h");
        }
        if !["auto", "propor"].contains(&self.contexto.recomecar.as_str()) {
            return bad(id, "recomecar: auto ou propor");
        }
        if !(100_000..=1_000_000).contains(&self.contexto.janela_longos) {
            return bad(id, "janelaLongos fora de 100000..1000000");
        }
        if !(1..=50).contains(&self.contexto.payback_k) {
            return bad(id, "paybackK fora de 1..50");
        }
        if self.saidas.bash_max.is_some_and(|n| !(1000..=150_000).contains(&n)) {
            return bad(id, "bashMax fora de 1000..150000");
        }
        if !(1..=16).contains(&self.paralelo_max) {
            return bad(id, "paraleloMax fora de 1..16");
        }
        if !["referencia", "inline"].contains(&self.artefatos.modo.as_str()) || !["auto", "json", "md"].contains(&self.artefatos.formato.as_str()) {
            return bad(id, "artefatos: modo ou formato desconhecido");
        }
        if self.subagente_modelo.as_deref().is_some_and(|m| !model_ok(m)) {
            return bad(id, "subagenteModelo inválido");
        }
        Ok(())
    }
}

/// Aplica um patch de `editados` sobre o preset. Aqui `null` vale "vazio" (um `Option` sem valor), não "apagar":
/// `editados` guarda a diferença contra o preset, e um `None` precisa sobreviver a ela.
fn set_keep_null(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                set_keep_null(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// Perfil de `id` com o patch de `editados` aplicado. Sem validar.
fn perfil_de(id: &str, patch: &Value) -> AppResult<Perfil> {
    let mut base = preset_value(id);
    set_keep_null(&mut base, patch);
    let mut p: Perfil = serde_json::from_value(base).map_err(|e| AppError::new("bad_perfil", format!("perfil {id}: {e}")))?;
    p.id = id.to_string();
    Ok(p)
}

/// Um patch de workspace (merge-patch RFC 7396) vale se o perfil resultante, sobre cada preset já editado, é válido.
pub fn patch_valido(patch: &Value, editados: &BTreeMap<String, Value>) -> bool {
    patch.is_object() && IDS.iter().all(|id| valid_patch(id, patch, editados.get(*id)))
}

fn valid_patch(id: &str, patch: &Value, editado: Option<&Value>) -> bool {
    let mut base = preset_value(id);
    if let Some(e) = editado {
        set_keep_null(&mut base, e);
    }
    merge_patch(&mut base, patch);
    serde_json::from_value::<Perfil>(base).is_ok_and(|mut p| {
        p.id = id.to_string();
        p.validate().is_ok()
    })
}

/// Valida uma edição de preset (objeto completo ou patch sobre o preset). Um `id`, se vier, tem de ser o da chave.
pub fn validate_editado(id: &str, v: &Value) -> AppResult<Perfil> {
    if !IDS.contains(&id) {
        return fail("bad_perfil", format!("perfil desconhecido: {id}"));
    }
    if v.get("id").is_some_and(|x| x != id) {
        return fail("bad_perfil", format!("o id do perfil {id} não pode mudar"));
    }
    let p = perfil_de(id, v)?;
    p.validate()?;
    Ok(p)
}

/// Diferença de um preset editado contra o preset: é isso que vai para `editados` (sem `id`).
pub fn to_patch(id: &str, p: &Perfil) -> AppResult<Value> {
    let mut target = serde_json::to_value(p).map_err(|e| AppError::new("bad_perfil", e.to_string()))?;
    if let Some(o) = target.as_object_mut() {
        o.remove("id");
    }
    Ok(diff(&preset_value(id), &target))
}

fn diff(base: &Value, target: &Value) -> Value {
    match (base, target) {
        (Value::Object(b), Value::Object(t)) => {
            let mut out = serde_json::Map::new();
            for (k, tv) in t {
                match b.get(k) {
                    Some(bv) if bv == tv => {}
                    Some(bv) if bv.is_object() && tv.is_object() => {
                        let d = diff(bv, tv);
                        if d.as_object().is_some_and(|o| !o.is_empty()) {
                            out.insert(k.clone(), d);
                        }
                    }
                    _ => {
                        out.insert(k.clone(), tv.clone());
                    }
                }
            }
            Value::Object(out)
        }
        _ => target.clone(),
    }
}

/// Próximo degrau da `escada` depois de `modelo:esforço`. None no topo ou fora da escada.
pub fn escalate(perfil: &Perfil, model: &str, effort: &str) -> Option<(String, String)> {
    let atual = format!("{model}:{effort}");
    let i = perfil.escada.iter().position(|s| *s == atual)?;
    let prox = perfil.escada.get(i + 1)?;
    let (m, e) = prox.split_once(':')?;
    Some((m.to_string(), e.to_string()))
}

/// Slot de modelo do perfil pelo nome (`planejar`, `implementar`, `revisar`, `testar`, `explorar`).
pub fn slot_of<'a>(p: &'a Perfil, slot: &str) -> Option<&'a Slot> {
    match slot {
        "planejar" => Some(&p.modelos.planejar),
        "implementar" => Some(&p.modelos.implementar),
        "revisar" => Some(&p.modelos.revisar),
        "testar" => Some(&p.modelos.testar),
        "explorar" => Some(&p.modelos.explorar),
        _ => None,
    }
}

/// O slot de um papel da missão agêntica (o papel do painel: `piloto`, `chefe:scout`…, `cruzado`).
pub fn papel_of<'a>(p: &'a Perfil, role: &str) -> Option<&'a Slot> {
    match role {
        "piloto" | "maestro" => Some(&p.papeis.piloto),
        "chefe:scout" | "scout" => Some(&p.papeis.scout),
        "chefe:builder" | "builder" => Some(&p.papeis.builder),
        "chefe:reviewer" | "reviewer" => Some(&p.papeis.reviewer),
        "chefe:controller" | "controller" => Some(&p.papeis.controller),
        "cruzado" | "chefe:cruzado" => p.papeis.cruzado.as_ref(),
        _ => None,
    }
}

/// O slot de um agente que um chefe ou o orquestrador acionou, pelo papel do arquivo dele: quem executa usa
/// `implementar`, quem revisa `revisar`, quem pesquisa `explorar`, quem testa `testar`, quem planeja `planejar`.
pub fn slot_do_agente<'a>(p: &'a Perfil, agent_role: Option<&str>) -> &'a Slot {
    match agent_role.unwrap_or("executor") {
        "reviewer" | "chefe:reviewer" | "revisor" => &p.modelos.revisar,
        "scout" | "chefe:scout" | "pesquisa" => &p.modelos.explorar,
        "tester" | "chefe:tester" | "qa" => &p.modelos.testar,
        "planner" | "orchestrator" | "piloto" | "maestro" => &p.modelos.planejar,
        _ => &p.modelos.implementar,
    }
}

/// O slot como argumentos de harness completos (CLI, modelo e esforço juntos: o modelo é daquela CLI).
pub fn slot_args(s: &Slot) -> crate::catalog::HarnessArgs {
    crate::catalog::HarnessArgs { cli: Some(s.cli.clone()), model: Some(s.model.clone()), effort: Some(s.effort.clone()) }
}

/// JSON merge-patch (RFC 7396): `null` apaga a chave, objeto mescla, o resto substitui.
pub fn merge_patch(base: &mut Value, patch: &Value) {
    let Value::Object(p) = patch else {
        *base = patch.clone();
        return;
    };
    if !base.is_object() {
        *base = json!({});
    }
    let Some(b) = base.as_object_mut() else { return };
    for (k, v) in p {
        if v.is_null() {
            b.remove(k);
        } else {
            merge_patch(b.entry(k.clone()).or_insert(Value::Null), v);
        }
    }
}

/// Perfil resolvido para um id conhecido: preset, mais `editados`, mais o patch do workspace.
/// Um resultado inválido perde só o patch do workspace (preset + editados); se ainda assim for inválido, cai no preset
/// (com aviso no log). Devolve (perfil, ajustado).
fn resolve_id(prefs: &PerfisPrefs, ws_id: &str, id: &str) -> (Perfil, bool) {
    let mut base = preset_value(id);
    if let Some(p) = prefs.editados.get(id) {
        set_keep_null(&mut base, p);
    }
    let patch = prefs.patches.get(ws_id);
    let ajustado = patch.is_some_and(|p| p.as_object().is_some_and(|o| !o.is_empty()));
    // F0b liga o nível do projeto aqui
    let build = |v: Value| {
        serde_json::from_value::<Perfil>(v).map_err(|e| e.to_string()).and_then(|mut p| {
            p.id = id.to_string();
            p.validate().map(|_| p).map_err(|e| e.to_string())
        })
    };
    if let Some(p) = patch {
        let mut com_patch = base.clone();
        merge_patch(&mut com_patch, p);
        match build(com_patch) {
            Ok(perfil) => return (perfil, ajustado),
            Err(e) => tracing::warn!("perfil {id} do workspace {ws_id} inválido ({e}); sem o patch do workspace"),
        }
    }
    let perfil = build(base).unwrap_or_else(|e| {
        tracing::warn!("perfil {id} inválido ({e}); usando o preset");
        preset(id).expect("preset conhecido")
    });
    (perfil, ajustado)
}

/// Origem do perfil: `missao` (a missão escolheu) ou `padrao` (configuração geral). Nível `projeto` entra com F0b.
/// Devolve (perfil, origem, ajustado): ajustado = há patch do workspace.
pub fn resolve(prefs: &PerfisPrefs, ws_id: &str, mission_profile: Option<&str>) -> (Perfil, &'static str, bool) {
    let (id, origem) = match mission_profile {
        Some(id) => (id.to_string(), "missao"),
        None => (prefs.padrao.clone(), "padrao"),
    };
    let id = if IDS.contains(&id.as_str()) {
        id
    } else {
        tracing::warn!("perfil desconhecido: {id}; usando equilibrado");
        "equilibrado".into()
    };
    let (perfil, ajustado) = resolve_id(prefs, ws_id, &id);
    (perfil, origem, ajustado)
}

/// Variáveis de ambiente e argumentos que o perfil põe numa CLI, num painel. O recibo mostra isso.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Aplicado {
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
}

/// Papel longo (§4.1): piloto, maestro, planner e painéis do usuário (sem papel). Os demais são curtos.
pub fn papel_longo(role: Option<&str>) -> bool {
    matches!(role, None | Some("piloto" | "maestro" | "planner"))
}

/// Conta de API (rota, chave ou nuvem), que é onde o TTL de 1 h precisa ser pedido (assinatura já recebe 1 h).
/// [inferido] Heurística: rota do painel, ou chave/nuvem no ambiente que o Regente herdou. O ambiente herdado só conta
/// quando o TTL pedido é 1 h: com 5 m, um falso positivo rebaixaria o TTL de 1 h de uma assinatura.
pub fn conta_api(rota: bool, ttl: &str) -> bool {
    rota
        || (ttl != "5m"
            && ["ANTHROPIC_API_KEY", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"].iter().any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty())))
}

/// Nome de variável, valor ou argumento que pode ir à linha de comando (`.cmd` shims): sem aspas nem espaços.
fn seguro(v: &str) -> bool {
    !v.is_empty() && v.len() <= 80 && v.chars().all(|c| c.is_ascii_alphanumeric() || "_.:-=".contains(c))
}

/// Traduz o perfil para as variáveis e os argumentos de uma CLI, conforme `perfilEnv`/`perfilArgs` do provedor.
/// Chaves lógicas: env `janelaLongos` (só papel longo), `bashMax`, `leituraMax`, `subagenteModelo`, `cacheTtlLongos`
/// (só papel longo em conta de API), `sugestoes` (sempre false), `desligarTitulo` (Econômico); args `bashMax`
/// (quarto do valor) e `docMax` (8 KB no Econômico, 16 KB nos outros). Valor que não passa em `seguro` é descartado.
pub fn aplicado(cli: &CliDef, p: &Perfil, longo: bool, conta_api: bool) -> Aplicado {
    let mut out = Aplicado::default();
    let valor = |chave: &str| -> Option<String> {
        match chave {
            "janelaLongos" if longo => Some(p.contexto.janela_longos.to_string()),
            "bashMax" => p.saidas.bash_max.map(|n| n.to_string()),
            "leituraMax" => p.saidas.leitura_max.map(|n| n.to_string()),
            "subagenteModelo" => p.subagente_modelo.clone(),
            "cacheTtlLongos" if longo && conta_api => Some(p.cache_ttl_longos.clone()),
            "sugestoes" => Some("false".into()),
            "desligarTitulo" if p.id == "economico" => Some("1".into()),
            _ => None,
        }
    };
    for (chave, var) in &cli.perfil_env {
        let Some(v) = valor(chave) else { continue };
        if seguro(var) && seguro(&v) {
            out.env.push((var.clone(), v));
        } else {
            tracing::warn!("perfil: valor recusado para {chave}");
        }
    }
    for (chave, tpl) in &cli.perfil_args {
        let n = match chave.as_str() {
            "bashMax" => p.saidas.bash_max.map(|n| (n + 2) / 4),
            "docMax" => Some(if p.id == "economico" { 8192 } else { 16384 }),
            _ => None,
        };
        let Some(n) = n else { continue };
        for t in tpl {
            let arg = t.replace("{n}", &n.to_string());
            if seguro(&arg) {
                out.args.push(arg);
            } else {
                tracing::warn!("perfil: argumento recusado para {chave}");
            }
        }
    }
    out
}

/// Perfil de um painel (id guardado em `panes.profile`; None = padrão). Usado na abertura e no respawn.
/// Devolve (perfil, ajustado).
pub fn efetivo_de(s: &AppState, ws_id: &str, id: Option<&str>) -> (Perfil, bool) {
    let prefs = lk(&s.settings).perfis.clone();
    let id = id.filter(|i| IDS.contains(i)).map(String::from).unwrap_or_else(|| prefs.padrao.clone());
    resolve_id(&prefs, ws_id, &id)
}

/// Perfil efetivo de um workspace (e missão, se houver). Ver `resolve`.
pub fn efetivo(s: &AppState, ws_id: &str, mission_id: Option<&str>) -> AppResult<(Perfil, &'static str, bool)> {
    let prefs = lk(&s.settings).perfis.clone();
    let mission_profile = match mission_id {
        Some(m) => match s.db.get_mission(m) {
            Ok(x) => x.profile,
            Err(e) => {
                tracing::warn!("missão {m} não lida para o perfil: {e}");
                None
            }
        },
        None => None,
    };
    Ok(resolve(&prefs, ws_id, mission_profile.as_deref()))
}

/// Grava (ou remove, com None) o patch de um workspace. Usado pelo desfazer de `ajustes`.
pub fn set_patch(s: &AppState, ws_id: &str, patch: Option<Value>) -> AppResult<()> {
    let mut cur = lk(&s.settings);
    gravar_patch(s, &mut cur, ws_id, patch)
}

/// Como `set_patch`, mas só grava se o patch atual ainda é `esperado` (comparação e escrita sob um lock). Devolve se gravou.
pub fn set_patch_if(s: &AppState, ws_id: &str, esperado: Option<&Value>, patch: Option<Value>) -> AppResult<bool> {
    let mut cur = lk(&s.settings);
    if cur.perfis.patches.get(ws_id) != esperado {
        return Ok(false);
    }
    gravar_patch(s, &mut cur, ws_id, patch)?;
    Ok(true)
}

fn gravar_patch(s: &AppState, cur: &mut crate::settings::Settings, ws_id: &str, patch: Option<Value>) -> AppResult<()> {
    match patch {
        Some(p) => cur.perfis.patches.insert(ws_id.to_string(), p),
        None => cur.perfis.patches.remove(ws_id),
    };
    save_json(&s.data.join("settings.json"), &*cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_file_has_the_three_ids() {
        let ids: Vec<String> = presets().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, IDS);
        let eq = preset("equilibrado").unwrap();
        assert_eq!(eq.modelos.implementar.model, "claude-haiku-5-5");
        assert_eq!(eq.modelos.implementar.cli, "claude");
        assert_eq!(eq.contexto.payback_k, 3);
        assert_eq!(preset("precisao").unwrap().saidas.bash_max, None);
    }

    #[test]
    fn escalation_walks_the_ladder_and_stops_at_the_top() {
        let eq = preset("equilibrado").unwrap();
        assert_eq!(escalate(&eq, "claude-haiku-5-5", "low"), Some(("claude-sonnet-5-5".into(), "low".into())));
        assert_eq!(escalate(&eq, "claude-sonnet-5-5", "medium"), Some(("claude-opus-5-5".into(), "high".into())));
        assert_eq!(escalate(&eq, "claude-opus-5-5", "high"), None);
        assert_eq!(escalate(&eq, "fora-da-escada", "low"), None);
    }

    #[test]
    fn merge_patch_follows_rfc_7396() {
        let mut v = json!({"a": 1, "b": {"c": 2, "d": 3}});
        merge_patch(&mut v, &json!({"b": {"c": null, "e": 4}, "a": 9}));
        assert_eq!(v, json!({"a": 9, "b": {"d": 3, "e": 4}}));
    }

    #[test]
    fn resolution_order_is_mission_then_default_and_patch_applies() {
        let mut prefs = PerfisPrefs::default();
        let (p, origem, ajustado) = resolve(&prefs, "ws", None);
        assert_eq!((p.id.as_str(), origem, ajustado), ("equilibrado", "padrao", false));

        let (p, origem, _) = resolve(&prefs, "ws", Some("economico"));
        assert_eq!((p.id.as_str(), origem), ("economico", "missao"));

        prefs.padrao = "precisao".into();
        assert_eq!(resolve(&prefs, "ws", None).0.id, "precisao");

        prefs.patches.insert("ws".into(), json!({"saidas": {"bashMax": 5000}}));
        let (p, _, ajustado) = resolve(&prefs, "ws", None);
        assert_eq!((p.saidas.bash_max, ajustado), (Some(5000), true));
        // outro workspace não herda o patch
        assert!(!resolve(&prefs, "outro", None).2);
    }

    #[test]
    fn unknown_mission_profile_falls_back_to_equilibrado() {
        let (p, origem, _) = resolve(&PerfisPrefs::default(), "ws", Some("inventado"));
        assert_eq!((p.id.as_str(), origem), ("equilibrado", "missao"));
    }

    #[test]
    fn clamp_drops_invalid_patches_and_edits() {
        let mut prefs = PerfisPrefs { padrao: "nao-existe".into(), ..Default::default() };
        prefs.patches.insert("bom".into(), json!({"saidas": {"bashMax": 5000}}));
        prefs.patches.insert("ruim".into(), json!({"saidas": {"bashMax": "muito"}}));
        prefs.editados.insert("economico".into(), json!({"saidas": {"bashMax": "muito"}}));
        prefs.editados.insert("precisao".into(), json!({"modelos": {"planejar": {"effort": "ultra"}}}));
        prefs.editados.insert("equilibrado".into(), json!({"paraleloMax": 3}));
        prefs.clamp();
        assert_eq!(prefs.padrao, "equilibrado");
        assert_eq!(prefs.patches.keys().collect::<Vec<_>>(), vec!["bom"]);
        assert_eq!(prefs.editados.keys().collect::<Vec<_>>(), vec!["equilibrado"]);
    }

    #[test]
    fn invalid_workspace_patch_keeps_the_edited_preset() {
        let prefs = PerfisPrefs {
            editados: BTreeMap::from([("economico".to_string(), json!({"saidas": {"bashMax": 9000}}))]),
            patches: BTreeMap::from([("ws".to_string(), json!({"saidas": {"bashMax": "muito"}}))]),
            ..Default::default()
        };
        let (eco, _, ajustado) = resolve(&prefs, "ws", Some("economico"));
        assert_eq!((eco.saidas.bash_max, ajustado), (Some(9000), true));
        assert!(!patch_valido(&json!({"saidas": {"bashMax": "muito"}}), &prefs.editados));
        // o patch só é válido se couber sobre o preset já editado
        assert!(patch_valido(&json!({"saidas": {"bashMax": 2000}}), &prefs.editados));
    }

    #[test]
    fn bash_max_is_clamped_to_the_documented_ceiling() {
        let mut p = preset("equilibrado").unwrap();
        p.saidas.bash_max = Some(150_000);
        assert!(p.validate().is_ok());
        p.saidas.bash_max = Some(150_001);
        assert!(p.validate().is_err());
    }

    #[test]
    fn efetivo_de_tolerates_unknown_profile_ids() {
        let d = tempfile::tempdir().unwrap();
        let s = crate::runtime::AppState::new(d.path().into(), "C:/x/regente.exe".into(), std::sync::Arc::new(|_: &str, _: Value| {})).unwrap();
        assert_eq!(efetivo_de(&s, "ws", Some("precisao")).0.id, "precisao");
        assert_eq!(efetivo_de(&s, "ws", Some("inventado")).0.id, "equilibrado");
        assert_eq!(efetivo_de(&s, "ws", None).0.id, "equilibrado");
    }

    #[test]
    fn literal_defaults_match_the_equilibrado_preset() {
        assert_eq!(Perfil::default(), preset("equilibrado").unwrap());
    }

    #[test]
    fn papeis_da_missao_nao_sao_opus_em_tudo_e_o_agente_segue_o_slot_do_papel_dele() {
        for p in presets() {
            // só o piloto (e o builder/reviewer da precisão máxima) é Opus: chefes e executores não queimam o limite
            assert_ne!(p.papeis.scout.model, "claude-opus-5-5", "{}", p.id);
            assert_ne!(p.papeis.controller.model, "claude-opus-5-5", "{}", p.id);
            assert!(p.papeis.cruzado.is_none(), "{}: revisão cruzada começa desligada", p.id);
            p.validate().unwrap();
        }
        let p = preset("equilibrado").unwrap();
        assert_eq!(papel_of(&p, "chefe:scout"), Some(&p.papeis.scout));
        assert_eq!(papel_of(&p, "piloto"), Some(&p.papeis.piloto));
        assert_eq!(papel_of(&p, "cruzado"), None);
        assert_eq!(papel_of(&p, "ajudante"), None);
        assert_eq!(slot_do_agente(&p, Some("reviewer")), &p.modelos.revisar);
        assert_eq!(slot_do_agente(&p, None), &p.modelos.implementar);
        // CLI desconhecida num papel é recusada
        let mut q = p.clone();
        q.papeis.builder.cli = "nao-existe".into();
        assert!(q.validate().is_err());
    }

    #[test]
    fn perfil_editado_antes_dos_papeis_ganha_os_papeis_do_preset() {
        let mut v = serde_json::to_value(preset("economico").unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("papeis");
        let prefs = PerfisPrefs { padrao: "economico".into(), editados: [("economico".to_string(), v)].into(), patches: Default::default() };
        let (p, _, _) = resolve(&prefs, "w", None);
        assert_eq!(p.papeis, preset("economico").unwrap().papeis);
    }

    #[test]
    fn old_json_with_missing_fields_still_loads_from_the_preset() {
        let p: Perfil = serde_json::from_value(json!({"id": "economico", "saidas": {"bashMax": 9000}})).unwrap();
        assert_eq!(p.saidas.bash_max, Some(9000));
        assert_eq!(p.saidas.leitura_max, Some(15000), "campo ausente vem do equilibrado");
        assert_eq!(p.modelos.implementar.model, "claude-haiku-5-5");
        let prefs = PerfisPrefs { editados: BTreeMap::from([("economico".to_string(), json!({"saidas": {"bashMax": 9000}}))]), ..Default::default() };
        let (eco, _, _) = resolve(&prefs, "ws", Some("economico"));
        assert_eq!((eco.saidas.bash_max, eco.saidas.leitura_max), (Some(9000), Some(10000)));
    }

    #[test]
    fn explicit_none_survives_the_edit_patch() {
        let mut edited = preset("equilibrado").unwrap();
        edited.saidas.bash_max = None;
        let patch = to_patch("equilibrado", &edited).unwrap();
        assert_eq!(patch, json!({"saidas": {"bashMax": null}}));
        let prefs = PerfisPrefs { editados: BTreeMap::from([("equilibrado".to_string(), patch)]), ..Default::default() };
        assert_eq!(resolve(&prefs, "ws", None).0.saidas.bash_max, None);
        // editar e voltar ao preset deixa o patch vazio
        assert_eq!(to_patch("equilibrado", &preset("equilibrado").unwrap()).unwrap(), json!({}));
    }

    #[test]
    fn validate_checks_values_and_bounds() {
        let mut p = preset("equilibrado").unwrap();
        assert!(p.validate().is_ok());
        p.modelos.planejar.effort = "ultra".into();
        assert!(p.validate().is_err());
        p = preset("equilibrado").unwrap();
        p.paralelo_max = 0;
        assert!(p.validate().is_err());
        p = preset("equilibrado").unwrap();
        p.contexto.janela_longos = 50_000;
        assert!(p.validate().is_err());
        p = preset("equilibrado").unwrap();
        p.modelos.revisar.model = "modelo com aspas\"".into();
        assert!(p.validate().is_err());
        p = preset("equilibrado").unwrap();
        p.modelos.testar.cli = "nao-existe".into();
        assert!(p.validate().is_err());
        p = preset("equilibrado").unwrap();
        p.cache_ttl_longos = "2h".into();
        assert!(p.validate().is_err());
    }

    #[test]
    fn invalid_resolution_falls_back_to_the_preset() {
        let prefs = PerfisPrefs { editados: BTreeMap::from([("economico".to_string(), json!({"paraleloMax": 99}))]), ..Default::default() };
        let (p, _, _) = resolve(&prefs, "ws", Some("economico"));
        assert_eq!(p.paralelo_max, preset("economico").unwrap().paralelo_max);
        let prefs = PerfisPrefs { patches: BTreeMap::from([("ws".to_string(), json!({"paraleloMax": 0}))]), ..Default::default() };
        assert_eq!(resolve(&prefs, "ws", None).0.paralelo_max, 3);
    }

    #[test]
    fn edited_preset_must_be_complete_and_keep_its_id() {
        let mut v = serde_json::to_value(preset("economico").unwrap()).unwrap();
        assert!(validate_editado("economico", &v).is_ok());
        assert!(validate_editado("precisao", &v).is_err());
        v["id"] = json!("outro");
        assert!(validate_editado("economico", &v).is_err());
    }

    #[test]
    fn efetivo_pane_resolution_ignores_unknown_ids() {
        let prefs = PerfisPrefs::default();
        let (p, _, _) = resolve(&prefs, "ws", Some("precisao"));
        assert_eq!(p.id, "precisao");
    }

    fn cli(id: &str) -> CliDef {
        let cfg: crate::providers::ProvidersConfig = serde_json::from_str(crate::settings::PROVIDERS_DEFAULT).unwrap();
        cfg.clis.into_iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn claude_env_golden_per_preset_and_role() {
        let c = cli("claude");
        let eco = aplicado(&c, &preset("economico").unwrap(), true, false);
        assert!(eco.env.contains(&("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "400000".into())));
        assert!(eco.env.contains(&("BASH_MAX_OUTPUT_LENGTH".into(), "15000".into())));
        assert!(eco.env.contains(&("CLAUDE_CODE_SUBAGENT_MODEL".into(), "haiku".into())));
        assert!(eco.env.contains(&("CLAUDE_CODE_DISABLE_TERMINAL_TITLE".into(), "1".into())));
        assert!(eco.env.contains(&("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION".into(), "false".into())));
        // precisão: sem limite de saída, sem janela de 800k só nos longos
        let pre = aplicado(&c, &preset("precisao").unwrap(), true, false);
        assert!(!pre.env.iter().any(|(k, _)| k == "BASH_MAX_OUTPUT_LENGTH"));
        assert!(pre.env.contains(&("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), "800000".into())));
        assert!(!pre.env.iter().any(|(k, _)| k == "CLAUDE_CODE_DISABLE_TERMINAL_TITLE"));
        // papel curto: sem janela, sem TTL, sem título do Econômico
        let short = aplicado(&c, &preset("economico").unwrap(), false, true);
        assert!(!short.env.iter().any(|(k, _)| k == "CLAUDE_CODE_AUTO_COMPACT_WINDOW" || k == "CLAUDE_CODE_PROMPT_CACHE_TTL"));
        assert!(short.env.contains(&("BASH_MAX_OUTPUT_LENGTH".into(), "15000".into())));
    }

    #[test]
    fn cache_ttl_only_for_long_roles_on_api_accounts() {
        let c = cli("claude");
        let p = preset("equilibrado").unwrap();
        let ttl = |longo, api| aplicado(&c, &p, longo, api).env.iter().find(|(k, _)| k == "CLAUDE_CODE_PROMPT_CACHE_TTL").map(|(_, v)| v.clone());
        assert_eq!(ttl(true, true).as_deref(), Some("1h"));
        assert_eq!(ttl(true, false), None, "assinatura já recebe 1 h");
        assert_eq!(ttl(false, true), None, "papel curto não pede");
    }

    #[test]
    fn inherited_api_env_never_downgrades_a_subscription_to_5m() {
        assert!(conta_api(true, "5m"), "rota do próprio painel vale");
        assert!(!conta_api(false, "5m"), "ambiente herdado não rebaixa o TTL de 1 h");
    }

    #[test]
    fn codex_args_only_for_output_limits() {
        let c = cli("codex");
        let eco = aplicado(&c, &preset("economico").unwrap(), true, false);
        assert!(eco.env.is_empty());
        assert_eq!(eco.args, ["-c", "tool_output_token_limit=3750", "-c", "project_doc_max_bytes=8192"]);
        let eq = aplicado(&c, &preset("equilibrado").unwrap(), false, false);
        assert_eq!(eq.args, ["-c", "tool_output_token_limit=5000", "-c", "project_doc_max_bytes=16384"]);
        let pre = aplicado(&c, &preset("precisao").unwrap(), true, false);
        assert_eq!(pre.args, ["-c", "project_doc_max_bytes=16384"]);
        assert!(!format!("{:?}", eq).contains("model_auto_compact_token_limit"));
    }

    #[test]
    fn no_value_reaches_the_command_line_with_quotes_or_spaces() {
        let mut c = cli("claude");
        c.perfil_env.insert("subagenteModelo".into(), "x y".into());
        let mut p = preset("economico").unwrap();
        p.subagente_modelo = Some("\"quoted\"".into());
        let a = aplicado(&c, &p, true, false);
        assert!(!a.env.iter().any(|(k, v)| k == "CLAUDE_CODE_SUBAGENT_MODEL" || v.contains('"') || v.contains(' ')));
    }

    #[test]
    fn long_roles_are_piloto_maestro_planner_and_user_panels() {
        assert!(papel_longo(None) && papel_longo(Some("piloto")) && papel_longo(Some("maestro")) && papel_longo(Some("planner")));
        assert!(!papel_longo(Some("ajudante")) && !papel_longo(Some("reviewer")) && !papel_longo(Some("executor")));
    }

    #[test]
    fn gemini_and_unknown_clis_get_nothing_from_the_profile_env() {
        let p = preset("economico").unwrap();
        let g = aplicado(&cli("gemini"), &p, true, true);
        assert!(g.env.is_empty() && g.args.is_empty());
        let u = aplicado(&CliDef::default(), &p, true, true);
        assert!(u.env.is_empty() && u.args.is_empty());
    }
}
