//! Piloto automático de eficiência (RM-29 / T1, roadmap §4.1, §4.3, §4.4 e R-15/R-16).
//! Regras puras sobre dados já agregados: cada uma devolve no máximo uma `Acao`. `tick` junta os dados de cada
//! workspace, aplica ou propõe e registra tudo no feed (`ajustes`). `desligado` não roda regra e não escreve nada.
//! Roda no ticker (a cada 10 min) e quando a tela de Uso abre; nunca no caminho do PTY.
//! Limites: `explorar` é o único slot que o piloto desce sozinho; `planejar` e `revisar` nunca descem; subir
//! `implementar` é só proposta (R-16).

use crate::ajustes::{self, Novo};
use crate::db::{Ajuste, EcoPane, FaseRow, WasteRow};
use crate::error::{fail, AppResult};
use crate::fases::VERIFICACAO;
use crate::perfil::{self, Perfil};
use crate::runtime::AppState;
use crate::settings::save_json;
use crate::util::{lk, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, TryLockError};

const DIA_MS: i64 = 86_400_000;
/// Amostras mínimas para uma regra agir.
pub const MIN_AMOSTRAS: usize = 5;
const HAIKU: &str = "claude-haiku-5-5";
/// K efetivo = `paybackK` do perfil + o delta aprendido, limitado a [K_MIN; K_MAX].
pub const K_MIN: f64 = 1.0;
pub const K_MAX: f64 = 8.0;
const K_DELTA_MIN: f64 = -7.0;
const K_DELTA_MAX: f64 = 7.0;
const FATOR_MIN: f64 = 0.5;
const FATOR_MAX: f64 = 3.0;
/// Uma mudança recusada (desfeita ou ignorada) não volta por este prazo.
const RECUSA_MS: i64 = 30 * DIA_MS;
/// Compactação sem efeito medido há mais que isto não segura o aprendizado (o painel nunca chegou a 5 turnos).
const SEM_EFEITO_MS: i64 = 7 * DIA_MS;
/// Um passo do piloto por vez: o ticker e a tela de Uso podem pedir juntos.
static PASSO: Mutex<()> = Mutex::new(());

/// K usado numa decisão: o `paybackK` do perfil mais o delta aprendido, limitado a [K_MIN; K_MAX].
/// Sem delta, vale o K do perfil (um K de perfil acima de 8 é escolha do usuário e não se corta). Com delta, o teto
/// é 8, ou o próprio K do perfil se ele for maior.
pub fn k_efetivo(base: u32, delta: Option<f64>) -> f64 {
    let base = f64::from(base);
    match delta {
        None => base,
        Some(d) => (base + d).clamp(K_MIN, K_MAX.max(base)),
    }
}

/// Preferências do piloto em `settings.json` (sem migração: os parâmetros aprendidos ficam aqui).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoAjustePrefs {
    /// aplicar | propor | desligado. Valor desconhecido vale como `propor` (o que não é conhecido não age sozinho).
    pub modo: String,
    pub aprendido: Aprendido,
    /// Mudanças recusadas pelo usuário (desfeitas ou ignoradas): `ws|regra|sha256(depois)` → ms da recusa.
    pub recusados: BTreeMap<String, i64>,
}

impl Default for AutoAjustePrefs {
    fn default() -> Self {
        Self { modo: "aplicar".into(), aprendido: Aprendido::default(), recusados: BTreeMap::new() }
    }
}

impl AutoAjustePrefs {
    pub fn modo(&self) -> &str {
        match self.modo.as_str() {
            "aplicar" => "aplicar",
            "desligado" => "desligado",
            _ => "propor",
        }
    }
}

/// Parâmetros aprendidos. `None` = ainda não aprendeu: vale o do perfil (K) ou o neutro (fator 1, razão da calibração).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Aprendido {
    /// Delta de K sobre o `paybackK` do perfil (não é o valor absoluto).
    pub k: Option<f64>,
    pub r_resumo: Option<f64>,
    pub fator_restante: Option<f64>,
    /// Marco do aprendizado: o rowid da última compactação já aprendida. Linha mais nova fica para a próxima rodada.
    pub visto: i64,
}

/// Uma mudança que o piloto quer fazer num workspace: o patch antes e depois (o desfazer usa os dois).
#[derive(Debug, Clone, PartialEq)]
pub struct Acao {
    /// Identidade da regra, para o limite de uma ação por workspace a cada 24 h.
    pub regra: &'static str,
    /// Kind do feed (`ajustes`).
    pub kind: &'static str,
    pub patch_before: Option<Value>,
    pub patch_after: Value,
    pub motivo: String,
    pub estimativa: Value,
    /// Só propõe, em qualquer modo (R-16 para `implementar`).
    pub so_proposta: bool,
}

/// Os dados de um workspace que as regras leem. Quem monta é `rodar_regra`; os testes montam direto.
pub struct Entrada<'a> {
    pub agora: i64,
    pub perfil: &'a Perfil,
    pub patch: Option<&'a Value>,
    pub editados: &'a BTreeMap<String, Value>,
    /// Achados de desperdício dos últimos 7 dias.
    pub achados: &'a [WasteRow],
    /// Painéis dos últimos 30 dias.
    pub panes: &'a [EcoPane],
    /// Painéis dos últimos 7 dias (amostra mínima da regra de índice).
    pub paineis_7d: usize,
    /// Fases de cada missão recente.
    pub missoes: &'a [Vec<FaseRow>],
    /// Feed do workspace.
    pub feed: &'a [Ajuste],
    /// A guarda de qualidade (RM-23) está verde.
    pub guarda_ok: bool,
}

/// Já houve uma ação desta regra no workspace nas últimas 24 h (aplicada ou proposta).
fn ja_agiu(e: &Entrada, regra: &str) -> bool {
    e.feed.iter().any(|a| e.agora - a.created_at < DIA_MS && a.estimativa_json.as_deref().is_some_and(|j| marcada(j, regra)))
}

/// A estimativa guardada no feed traz a regra que a gerou.
fn marcada(json: &str, regra: &str) -> bool {
    serde_json::from_str::<Value>(json).ok().and_then(|v| v["regra"].as_str().map(|r| r == regra)).unwrap_or(false)
}

fn contagem(achados: &[WasteRow], kind: &str) -> i64 {
    achados.iter().filter(|a| a.kind == kind).map(|a| a.count).sum()
}

/// Patch com a mudança `delta` sobre o patch atual. None se não muda nada ou se o resultado não é um perfil válido.
fn acao_patch(e: &Entrada, regra: &'static str, kind: &'static str, motivo: String, delta: Value, mut estimativa: Value, so_proposta: bool) -> Option<Acao> {
    let antes = e.patch.cloned();
    let mut depois = antes.clone().unwrap_or_else(|| json!({}));
    perfil::merge_patch(&mut depois, &delta);
    if antes.as_ref() == Some(&depois) || !perfil::patch_valido(&depois, e.editados) {
        return None;
    }
    estimativa["regra"] = json!(regra);
    Some(Acao { regra, kind, patch_before: antes, patch_after: depois, motivo, estimativa, so_proposta })
}

/// 2. `repeated_read` ≥ 3 em 7 dias → liga o índice de código neste workspace.
pub fn indice_codigo(e: &Entrada) -> Option<Acao> {
    if e.perfil.indice_codigo || e.paineis_7d < MIN_AMOSTRAS || ja_agiu(e, "indice_codigo") {
        return None;
    }
    let n = contagem(e.achados, "repeated_read");
    if n < 3 {
        return None;
    }
    acao_patch(e, "indice_codigo", "indice_codigo", format!("{n} leituras repetidas de arquivos em 7 dias; o índice de código evita reler"), json!({"indiceCodigo": true}), json!({"achados": n}), false)
}

/// 3. Tarefas de implementação com escalonamento acima de 15 % (≥ 10 tarefas) → só propõe subir `implementar` um degrau (R-16).
pub fn escalonamento(e: &Entrada) -> Option<Acao> {
    if ja_agiu(e, "escalonamento") {
        return None;
    }
    let tarefas: Vec<&EcoPane> = e.panes.iter().filter(|p| matches!(p.task_type.as_deref(), Some("frontend" | "backend" | "docs"))).collect();
    if tarefas.len() < 10 {
        return None;
    }
    let escalaram = tarefas.iter().filter(|p| p.escalations > 0).count();
    let taxa = escalaram as f64 / tarefas.len() as f64;
    if taxa <= 0.15 {
        return None;
    }
    let atual = &e.perfil.modelos.implementar;
    let (modelo, esforco) = perfil::escalate(e.perfil, &atual.model, &atual.effort)?;
    let motivo = format!("{:.0}% das tarefas de implementação escalaram; subir implementar para {modelo} {esforco}?", taxa * 100.0);
    acao_patch(e, "escalonamento", "escalonamento", motivo, json!({"modelos": {"implementar": {"model": modelo, "effort": esforco}}}), json!({"taxa": taxa, "amostras": tarefas.len()}), true)
}

/// 4. Tarefas mecânicas (pesquisa, mecânico) com ≥ 90 % de acerto no modelo barato (≥ 10 painéis) → o slot `explorar`
/// desce para Haiku. Só `explorar` (R-15); a guarda de qualidade precisa estar verde.
pub fn explorar_barato(e: &Entrada) -> Option<Acao> {
    if !e.guarda_ok || ja_agiu(e, "explorar_barato") {
        return None;
    }
    let mecanicos: Vec<&EcoPane> = e
        .panes
        .iter()
        .filter(|p| matches!(p.task_type.as_deref(), Some("pesquisa" | "mecanico")) && p.model.as_deref().is_some_and(|m| m.contains("haiku")))
        .collect();
    if mecanicos.len() < 10 {
        return None;
    }
    let deram_certo = mecanicos.iter().filter(|p| p.escalations == 0).count();
    if deram_certo * 10 < mecanicos.len() * 9 {
        return None;
    }
    // Haiku só existe no Claude: o slot precisa estar no CLI claude e ainda não estar no barato
    if e.perfil.modelos.explorar.cli != "claude" || e.perfil.modelos.explorar.model == HAIKU {
        return None;
    }
    let motivo = format!("{deram_certo} de {} tarefas mecânicas deram certo no modelo barato; explorar desce para Haiku", mecanicos.len());
    acao_patch(e, "explorar_barato", "perfil_patch", motivo, json!({"modelos": {"explorar": {"model": HAIKU, "effort": "low"}}}), json!({"amostras": mecanicos.len(), "sucesso": deram_certo}), false)
}

/// Tempo total das fases fechadas e o da verificação, em ms.
pub fn tempos(fases: &[FaseRow]) -> (i64, i64) {
    fases.iter().filter_map(|f| f.ended_at.map(|fim| (f, fim))).fold((0, 0), |(t, v), (f, fim)| {
        let d = (fim - f.started_at).max(0);
        (t + d, if f.phase == VERIFICACAO { v + d } else { v })
    })
}

/// 5. Verificação > 40 % do tempo de ≥ 3 missões → `testes.relacionadosPrimeiro` ligado.
pub fn testes_relacionados(e: &Entrada) -> Option<Acao> {
    if e.perfil.testes.relacionados_primeiro || ja_agiu(e, "testes_relacionados") {
        return None;
    }
    let (mut total, mut verif, mut n) = (0i64, 0i64, 0usize);
    for fases in e.missoes {
        let (t, v) = tempos(fases);
        if t > 0 {
            total += t;
            verif += v;
            n += 1;
        }
    }
    if n < 3 || total <= 0 {
        return None;
    }
    let razao = verif as f64 / total as f64;
    if razao <= 0.40 {
        return None;
    }
    let motivo = format!("a verificação ocupa {:.0}% do tempo de {n} missões; testes relacionados primeiro", razao * 100.0);
    acao_patch(e, "testes_relacionados", "testes_relacionados", motivo, json!({"testes": {"relacionadosPrimeiro": true}}), json!({"missoes": n, "razao": razao}), false)
}

/// 6. `huge_output` ≥ 5 em 7 dias → `saidas.resumirAcima` e `saidas.bashMax` descem 25 % (pisos 4000 e 8000).
pub fn limites_saida(e: &Entrada) -> Option<Acao> {
    if ja_agiu(e, "limites_saida") {
        return None;
    }
    let n = contagem(e.achados, "huge_output");
    if n < 5 {
        return None;
    }
    let s = &e.perfil.saidas;
    let mut delta = serde_json::Map::new();
    let resumir = s.resumir_acima as f64;
    let novo = (resumir * 0.75).round().max(4000.0).min(resumir) as u32;
    if novo < s.resumir_acima {
        delta.insert("resumirAcima".into(), json!(novo));
    }
    if let Some(b) = s.bash_max {
        let novo = (b as f64 * 0.75).round().max(8000.0).min(b as f64) as u32;
        if novo < b {
            delta.insert("bashMax".into(), json!(novo));
        }
    }
    if delta.is_empty() {
        return None;
    }
    let motivo = format!("{n} saídas enormes em 7 dias; os limites de saída descem um degrau");
    acao_patch(e, "limites_saida", "limites_saida", motivo, json!({"saidas": delta}), json!({"achados": n}), false)
}

/// Regras que agem por workspace, na ordem de avaliação. O nome é o `estimativa.regra` que o feed guarda.
type Regra = fn(&Entrada) -> Option<Acao>;
const REGRAS: [(&str, Regra); 5] = [
    ("indice_codigo", indice_codigo),
    ("escalonamento", escalonamento),
    ("explorar_barato", explorar_barato),
    ("testes_relacionados", testes_relacionados),
    ("limites_saida", limites_saida),
];

/// Aprendizado de K a partir do efeito medido de cada compactação: (economia medida, economia estimada `n·G − C`).
/// Estimativa ≤ 0 entra também: a conta dizia que não compensava.
pub fn medidas_k(feed: &[Ajuste]) -> Vec<(f64, f64)> {
    feed.iter()
        .filter(|a| a.kind == "compactacao" && a.acao == "aplicado")
        .filter_map(|a| {
            let ef: Value = serde_json::from_str(a.efeito_json.as_deref()?).ok()?;
            let est: Value = serde_json::from_str(a.estimativa_json.as_deref()?).ok()?;
            let (g, c, n) = (est["G"].as_f64()?, est["C"].as_f64()?, ef["turnos"].as_f64()?);
            let estimado = n * g - c;
            let medido = ef["custoSemAcao"].as_f64()? - ef["custoReal"].as_f64()?;
            Some((medido, estimado))
        })
        .collect()
}

/// Delta de K: economia medida ≥ estimada (e estimada > 0) em ≥ 3 casos → +0,5; medida < 50 % da estimada, ou
/// estimada ≤ 0, em ≥ 3 casos → −0,5. Precisa de `MIN_AMOSTRAS`. Limitado a [−7; +7]. None se não muda.
pub fn aprender_k(atual: f64, medidas: &[(f64, f64)]) -> Option<f64> {
    if medidas.len() < MIN_AMOSTRAS {
        return None;
    }
    let ruins = medidas.iter().filter(|(m, e)| *e <= 0.0 || *m < 0.5 * e).count();
    let bons = medidas.iter().filter(|(m, e)| *e > 0.0 && m >= e).count();
    let novo = if ruins >= 3 {
        atual - 0.5
    } else if bons >= 3 {
        atual + 0.5
    } else {
        return None;
    };
    let novo = novo.clamp(K_DELTA_MIN, K_DELTA_MAX);
    ((novo - atual).abs() > 1e-9).then_some(novo)
}

/// Fator dos turnos restantes: a mediana de (real ÷ estimado) multiplica o fator atual. Limitado a [0,5; 3]. None se não muda.
pub fn aprender_fator(atual: f64, razoes: &[f64]) -> Option<f64> {
    if razoes.len() < MIN_AMOSTRAS {
        return None;
    }
    let mut v = razoes.to_vec();
    v.sort_by(f64::total_cmp);
    let m = v.len() / 2;
    let mediana = if v.len() % 2 == 1 { v[m] } else { (v[m - 1] + v[m]) / 2.0 };
    let novo = (atual * mediana).clamp(FATOR_MIN, FATOR_MAX);
    ((novo - atual).abs() >= 0.01).then_some(novo)
}

/// Chave da recusa: workspace, regra e hash do estado proposto.
fn chave(ws: &str, regra: &str, depois: &Value) -> String {
    let hash: String = Sha256::digest(depois.to_string().as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    format!("{ws}|{regra}|{hash}")
}

/// Grava que o usuário recusou esta mudança (desfez ou ignorou). Expiradas saem da lista.
pub fn recusar(s: &AppState, ws: &str, regra: &str, depois: &Value) {
    let agora = now_ms();
    let mut cur = lk(&s.settings);
    cur.auto_ajuste.recusados.retain(|_, ts| agora - *ts < RECUSA_MS);
    cur.auto_ajuste.recusados.insert(chave(ws, regra, depois), agora);
    if let Err(e) = save_json(&s.data.join("settings.json"), &*cur) {
        tracing::warn!("recusa do ajuste não gravada: {e}");
    }
}

/// O piloto não age de novo na mudança que o usuário recusou (30 dias) nem repropõe a que já está proposta em aberto.
fn vetada(s: &AppState, ws: &str, a: &Acao, feed: &[Ajuste], agora: i64) -> bool {
    let recusada = lk(&s.settings).auto_ajuste.recusados.get(&chave(ws, a.regra, &a.patch_after)).is_some_and(|ts| agora - ts < RECUSA_MS);
    let em_aberto = feed.iter().any(|f| {
        f.acao == "proposto"
            && f.estimativa_json.as_deref().is_some_and(|j| marcada(j, a.regra))
            && f.depois_json.as_deref().and_then(|j| serde_json::from_str::<Value>(j).ok()).as_ref() == Some(&a.patch_after)
    });
    recusada || em_aberto
}

/// "Ignorar" numa proposta: a mudança sai do caminho do piloto por 30 dias e a linha vira `pulado`.
pub fn ignorar(s: &AppState, id: &str) -> AppResult<()> {
    let a = s.db.get_ajuste(id)?;
    if a.acao != "proposto" {
        return fail("nao_proposta", "este ajuste não é uma proposta");
    }
    let regra = ajustes::regra_de(&a);
    let depois = a.depois_json.as_deref().and_then(|j| serde_json::from_str::<Value>(j).ok());
    let (Some(ws), Some(regra), Some(depois)) = (a.workspace_id.clone(), regra, depois) else {
        return fail("bad_ajuste", "proposta sem o estado proposto");
    };
    recusar(s, &ws, &regra, &depois);
    s.db.set_ajuste_acao(id, "pulado", false)?;
    s.emit("ajuste-atualizado", json!({"workspaceId": ws, "id": id}));
    Ok(())
}

/// Apaga o aprendizado (K, fator e razão de resumo). O marco de linhas aprendidas fica: não reaprende o que já entrou.
pub fn zerar_aprendizado(s: &AppState) -> AppResult<()> {
    let mut cur = lk(&s.settings);
    cur.auto_ajuste.aprendido = Aprendido { visto: cur.auto_ajuste.aprendido.visto, ..Aprendido::default() };
    save_json(&s.data.join("settings.json"), &*cur)?;
    drop(cur);
    s.emit("perfis-changed", json!({}));
    Ok(())
}

/// Grava a razão resumo/contexto calibrada (PostCompact). Em `desligado` não grava nada.
pub fn guardar_r_resumo(s: &AppState, r: f64) {
    let mut cur = lk(&s.settings);
    // só o modo `aplicar` escreve o aprendizado (propor e desligado não mexem em nada aprendido)
    if cur.auto_ajuste.modo() != "aplicar" {
        return;
    }
    cur.auto_ajuste.aprendido.r_resumo = Some(r);
    if let Err(e) = save_json(&s.data.join("settings.json"), &*cur) {
        tracing::warn!("razão de resumo aprendida não gravada: {e}");
    }
}

/// Um passo do piloto: avalia as regras de cada workspace e age (aplica, propõe ou não faz nada).
/// `desligado` sai na hora. Erros de um workspace não param os outros.
pub fn tick(s: &AppState) -> AppResult<()> {
    // o segundo pedido enquanto um passo roda sai na hora (não espera nem repete)
    let _passo = match PASSO.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return Ok(()),
    };
    let modo = lk(&s.settings).auto_ajuste.modo().to_string();
    if modo == "desligado" {
        return Ok(());
    }
    let agora = now_ms();
    if modo == "aplicar" {
        if let Err(e) = aprender(s, agora) {
            tracing::warn!("aprendizado da compactação não atualizado: {e}");
        }
    }
    if let Err(e) = crate::cost::waste::refresh(s) {
        tracing::warn!("achados de desperdício não atualizados: {e}");
    }
    for ws in s.db.list_workspaces()? {
        let mut cache = Cache::default();
        for (nome, regra) in REGRAS {
            if let Err(e) = rodar_regra(s, &ws.id, nome, regra, &modo, agora, &mut cache) {
                tracing::warn!("regra do piloto no workspace {} não avaliada: {e}", ws.id);
            }
        }
    }
    Ok(())
}

/// O que as regras leem de um workspace numa janela: achados desde `w`, painéis e missões desde `p`.
struct Coleta {
    achados: Vec<WasteRow>,
    panes: Vec<EcoPane>,
    paineis_7d: usize,
    missoes: Vec<Vec<FaseRow>>,
}

/// Coletas de um workspace num passo do piloto: regras sem ação recente na janela padrão dividem a mesma leitura.
#[derive(Default)]
struct Cache {
    guarda: Option<bool>,
    coletas: HashMap<(i64, i64), Coleta>,
}

fn coleta(s: &AppState, ws: &str, w: i64, p: i64) -> AppResult<Coleta> {
    let mut missoes = vec![];
    for m in s.db.list_missions(ws)? {
        if m.created_at >= p {
            missoes.push(s.db.fases_da_missao(&m.id)?);
        }
    }
    Ok(Coleta { achados: s.db.waste_rows(w, Some(ws))?, panes: s.db.economy_panes(p, Some(ws))?, paineis_7d: s.db.economy_panes(w, Some(ws))?.len(), missoes })
}

/// Junta os dados do workspace (já com o patch, o perfil e o feed atuais, para o próximo passo ver o que a regra anterior fez).
/// Sem compor: a regra só conta amostras mais novas que o último ajuste dela no workspace.
fn rodar_regra(s: &AppState, ws: &str, nome: &str, regra: Regra, modo: &str, agora: i64, cache: &mut Cache) -> AppResult<()> {
    let (perfil, _, _) = perfil::efetivo(s, ws, None)?;
    let (patch, editados) = {
        let cur = lk(&s.settings);
        (cur.perfis.patches.get(ws).cloned(), cur.perfis.editados.clone())
    };
    // 1000 linhas: com muitas compactações no workspace, a marca de 24 h precisa caber na janela
    let feed = s.db.list_ajustes(Some(ws), 1000)?;
    let ultimo = feed.iter().filter(|a| a.estimativa_json.as_deref().is_some_and(|j| marcada(j, nome))).map(|a| a.created_at).max().unwrap_or(0);
    let (w, p) = ((agora - 7 * DIA_MS).max(ultimo + 1), (agora - 30 * DIA_MS).max(ultimo + 1));
    if !cache.coletas.contains_key(&(w, p)) {
        let c = coleta(s, ws, w, p)?;
        cache.coletas.insert((w, p), c);
    }
    let guarda_ok = match cache.guarda {
        Some(g) => g,
        None => {
            let g = crate::economia::quality(s, ws).is_ok_and(|g| g.ok);
            cache.guarda = Some(g);
            g
        }
    };
    let c = &cache.coletas[&(w, p)];
    let e = Entrada { agora, perfil: &perfil, patch: patch.as_ref(), editados: &editados, achados: &c.achados, panes: &c.panes, paineis_7d: c.paineis_7d, missoes: &c.missoes, feed: &feed, guarda_ok };
    match regra(&e) {
        Some(a) if !vetada(s, ws, &a, &feed, agora) => aplicar(s, ws, a, modo),
        _ => Ok(()),
    }
}

/// Aplica (com o patch conferido sob o lock) ou registra a proposta. Propostas guardam antes e depois para o "Aplicar".
/// Só aplica o que dá para desfazer: antes ou depois grandes demais (ou redigidos) viram proposta.
fn aplicar(s: &AppState, ws: &str, a: Acao, modo: &str) -> AppResult<()> {
    let estimativa = Some(a.estimativa.clone());
    let desfazivel = ajustes::cabe(a.patch_before.as_ref()) && ajustes::cabe(Some(&a.patch_after));
    // `propor` e modo desconhecido nunca aplicam
    if a.so_proposta || modo != "aplicar" || !desfazivel {
        ajustes::registrar(s, Novo {
            workspace_id: Some(ws.to_string()), mission_id: None, pane_id: None, kind: a.kind, acao: "proposto",
            motivo: a.motivo, antes: a.patch_before, depois: Some(a.patch_after), estimativa, desfazivel: false,
        })?;
        return Ok(());
    }
    // não pisa numa mudança feita depois da leitura: se o patch mudou, a próxima rodada decide de novo
    if !perfil::set_patch_if(s, ws, a.patch_before.as_ref(), Some(a.patch_after.clone()))? {
        return Ok(());
    }
    let registro = ajustes::registrar(s, Novo {
        workspace_id: Some(ws.to_string()), mission_id: None, pane_id: None, kind: a.kind, acao: "aplicado",
        motivo: a.motivo, antes: a.patch_before.clone(), depois: Some(a.patch_after.clone()), estimativa, desfazivel: true,
    });
    if let Err(e) = registro {
        // mudança sem registro não pode ficar: volta o patch (só se ele ainda é o aplicado) e devolve o erro
        perfil::set_patch_if(s, ws, Some(&a.patch_after), a.patch_before)?;
        return Err(e);
    }
    s.emit("perfis-changed", json!({"workspaceId": ws}));
    Ok(())
}

/// "Aplicar" numa proposta do feed: grava o depois se o patch ainda é o antes; o ajuste passa a ser desfazível.
pub fn aplicar_proposta(s: &AppState, id: &str) -> AppResult<()> {
    let a = s.db.get_ajuste(id)?;
    if a.acao != "proposto" {
        return fail("nao_proposta", "este ajuste não é uma proposta");
    }
    let Some(ws) = a.workspace_id.clone() else { return fail("bad_ajuste", "proposta sem workspace") };
    let ler = |t: &Option<String>| -> AppResult<Option<Value>> {
        t.as_deref().map(serde_json::from_str::<Value>).transpose().map(|v| v.filter(|v| !v.is_null())).map_err(|e| crate::error::AppError::new("bad_ajuste", format!("proposta ilegível: {e}")))
    };
    let Some(depois) = ler(&a.depois_json)? else { return fail("bad_ajuste", "proposta sem o estado proposto") };
    let antes = ler(&a.antes_json)?;
    let editados = lk(&s.settings).perfis.editados.clone();
    if !perfil::patch_valido(&depois, &editados) {
        return fail("bad_ajuste", "a proposta não é um perfil válido");
    }
    if !perfil::set_patch_if(s, &ws, antes.as_ref(), Some(depois))? {
        return fail("stale", "o perfil mudou depois desta proposta");
    }
    s.db.set_ajuste_acao(id, "aplicado", true)?;
    s.emit("perfis-changed", json!({"workspaceId": ws}));
    // não é um ajuste novo do piloto: a linha que já existia mudou (sem o aviso de "ajuste automático")
    s.emit("ajuste-atualizado", json!({"workspaceId": ws, "id": id}));
    Ok(())
}

/// Aprendizado de K e do fator de turnos restantes, a partir do feed. Só em `aplicar`, no máximo uma vez a cada 24 h.
fn aprender(s: &AppState, agora: i64) -> AppResult<()> {
    let feed = s.db.list_ajustes(None, 1000)?;
    if feed.iter().any(|a| agora - a.created_at < DIA_MS && a.estimativa_json.as_deref().is_some_and(|j| marcada(j, "aprender"))) {
        return Ok(());
    }
    let visto = lk(&s.settings).auto_ajuste.aprendido.visto;
    if s.db.contar_compactacoes_depois(visto)? == 0 {
        return Ok(());
    }
    // cada amostra vale uma vez: o marco é o rowid. Uma linha sem efeito medido segura o marco (até 7 dias), e as de
    // trás dela esperam; a linha que ficou sem medida nesse prazo passa sem entrar
    let mut novos: Vec<Ajuste> = vec![];
    let mut marco = visto;
    for (rid, a) in s.db.compactacoes_depois(visto, 1000)? {
        if a.efeito_json.is_none() {
            if agora - a.created_at < SEM_EFEITO_MS {
                break;
            }
        } else {
            novos.push(a);
        }
        marco = rid;
    }
    let atual = lk(&s.settings).auto_ajuste.aprendido.clone();
    let k = aprender_k(atual.k.unwrap_or(0.0), &medidas_k(&novos));
    let fator = aprender_fator(atual.fator_restante.unwrap_or(1.0), &razoes_restante(s, &novos));
    let (k_final, f_final) = {
        let mut cur = lk(&s.settings);
        if let Some(v) = k {
            cur.auto_ajuste.aprendido.k = Some(v);
        }
        if let Some(v) = fator {
            cur.auto_ajuste.aprendido.fator_restante = Some(v);
        }
        cur.auto_ajuste.aprendido.visto = marco;
        save_json(&s.data.join("settings.json"), &*cur)?;
        (cur.auto_ajuste.aprendido.k, cur.auto_ajuste.aprendido.fator_restante)
    };
    // só o marco andou: nada mudou no que se aprendeu, e o feed não ganha linha
    if k.is_none() && fator.is_none() {
        return Ok(());
    }
    ajustes::registrar(s, Novo {
        workspace_id: None, mission_id: None, pane_id: None, kind: "compactacao", acao: "aprendido",
        motivo: "piloto atualizou o que aprendeu sobre a compactação (delta de K e fator de turnos restantes)".into(),
        antes: None, depois: None, estimativa: Some(json!({"regra": "aprender", "deltaK": k_final, "fator": f_final})), desfazivel: false,
    })?;
    Ok(())
}

/// Turnos restantes reais ÷ estimados, de compactações cujo painel já fechou.
fn razoes_restante(s: &AppState, feed: &[Ajuste]) -> Vec<f64> {
    feed.iter()
        .filter(|a| a.kind == "compactacao" && a.acao == "aplicado")
        .filter_map(|a| {
            let est: Value = serde_json::from_str(a.estimativa_json.as_deref()?).ok()?;
            let (r, t0) = (est["R"].as_f64()?, est["turnos"].as_i64()?);
            let pane = s.db.get_pane(a.pane_id.as_deref()?).ok()?;
            pane.closed_at?;
            let fim = s.db.pane_metrics(&pane.id).ok()??.turns;
            let real = (fim - t0) as f64;
            (r > 0.0 && real >= 0.0).then_some(real / r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{WasteIns, WasteOff};
    use crate::util::new_id;
    use std::sync::Arc;

    const AGORA: i64 = 1_800_000_000_000;

    fn pane(task: &str, model: &str, esc: i64) -> EcoPane {
        EcoPane { profile: None, task_type: Some(task.into()), model: Some(model.into()), mission_id: None, cost_usd: 1.0, tok_in: 1, tok_out: 1, cache_r: 1, cache_w: 1, turns: 5, compactions: 0, fresh_starts: 0, escalations: esc, first_turn_tokens: Some(1000) }
    }

    fn achado(kind: &str, count: i64) -> WasteRow {
        WasteRow { kind: kind.into(), count, panes: 1, tokens_est: 1, last_at: AGORA, exemplo: None, removidos: 0 }
    }

    fn fase(phase: &str, inicio: i64, fim: Option<i64>) -> FaseRow {
        FaseRow { id: 1, mission_id: "m".into(), pane_id: None, task_id: None, phase: phase.into(), started_at: inicio, ended_at: fim, tok_in: 0, tok_out: 0, cost_usd: 0.0 }
    }

    fn feed_com(regra: &str, ha_ms: i64) -> Ajuste {
        Ajuste {
            id: "x".into(), created_at: AGORA - ha_ms, workspace_id: Some("ws".into()), mission_id: None, pane_id: None, kind: "perfil_patch".into(),
            acao: "aplicado".into(), motivo: String::new(), antes_json: None, depois_json: None, estimativa_json: Some(json!({"regra": regra}).to_string()),
            efeito_json: None, desfazivel: true, desfeito_at: None,
        }
    }

    fn eq() -> Perfil {
        perfil::preset("equilibrado").unwrap()
    }

    #[test]
    fn indice_codigo_thresholds_and_guards() {
        let mut p = eq();
        // 2 leituras repetidas: abaixo do limite
        let duas = vec![achado("repeated_read", 2)];
        let tres = vec![achado("repeated_read", 3)];
        let r = |a: &[WasteRow], paineis: usize| {
            let (p, ed, m, f) = (eq(), BTreeMap::new(), Vec::<Vec<FaseRow>>::new(), Vec::<Ajuste>::new());
            indice_codigo(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: a, panes: &[], paineis_7d: paineis, missoes: &m, feed: &f, guarda_ok: true })
        };
        assert!(r(&duas, 10).is_none());
        let a = r(&tres, 10).expect("no limite");
        assert_eq!(a.patch_after, json!({"indiceCodigo": true}));
        assert_eq!(a.kind, "indice_codigo");
        assert!(!a.so_proposta, "aplica sozinho");
        assert!(r(&tres, MIN_AMOSTRAS - 1).is_none(), "amostra mínima");
        p.indice_codigo = true;
        let ja_ligado = Entrada { agora: AGORA, perfil: &p, patch: None, editados: &BTreeMap::new(), achados: &tres, panes: &[], paineis_7d: 10, missoes: &[], feed: &[], guarda_ok: true };
        assert!(indice_codigo(&ja_ligado).is_none());
    }

    #[test]
    fn throttle_is_one_action_per_rule_per_day() {
        let feed_recente = vec![feed_com("indice_codigo", 3_600_000)];
        let feed_antigo = vec![feed_com("indice_codigo", 25 * 3_600_000)];
        let (p, ed, m) = (eq(), BTreeMap::new(), Vec::<Vec<FaseRow>>::new());
        let tres = vec![achado("repeated_read", 3)];
        let mk = |f: &[Ajuste]| indice_codigo(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &tres, panes: &[], paineis_7d: 10, missoes: &m, feed: f, guarda_ok: true });
        assert!(mk(&feed_recente).is_none());
        assert!(mk(&feed_antigo).is_some());
    }

    #[test]
    fn escalonamento_only_proposes_above_fifteen_percent_with_ten_tasks() {
        let mk = |panes: &[EcoPane]| {
            let (p, ed, m, f) = (eq(), BTreeMap::new(), Vec::<Vec<FaseRow>>::new(), Vec::<Ajuste>::new());
            escalonamento(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &[], panes, paineis_7d: 10, missoes: &m, feed: &f, guarda_ok: true })
        };
        let nove: Vec<EcoPane> = (0..9).map(|_| pane("backend", "claude-haiku-5-5", 1)).collect();
        assert!(mk(&nove).is_none(), "9 tarefas: sem amostra");
        let dez_uma: Vec<EcoPane> = (0..10).map(|i| pane("backend", "claude-haiku-5-5", i64::from(i < 1))).collect();
        assert!(mk(&dez_uma).is_none(), "10% não passa de 15%");
        let dez_duas: Vec<EcoPane> = (0..10).map(|i| pane("backend", "claude-haiku-5-5", i64::from(i < 2))).collect();
        let a = mk(&dez_duas).expect("20% passa");
        assert!(a.so_proposta, "R-16: implementar só é proposto");
        assert_eq!(a.patch_after["modelos"]["implementar"]["model"], "claude-sonnet-5-5");
        assert_eq!(a.patch_after["modelos"]["implementar"]["effort"], "low");
        let fora_da_implementacao: Vec<EcoPane> = (0..10).map(|_| pane("pesquisa", "claude-haiku-5-5", 1)).collect();
        assert!(mk(&fora_da_implementacao).is_none(), "só tarefas de implementação contam");
    }

    fn explorar(panes: &[EcoPane], guarda: bool, slot: &str) -> Option<Acao> {
        let mut p = eq();
        p.modelos.explorar.model = slot.into();
        let (ed, m, f) = (BTreeMap::new(), Vec::<Vec<FaseRow>>::new(), Vec::<Ajuste>::new());
        explorar_barato(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &[], panes, paineis_7d: 10, missoes: &m, feed: &f, guarda_ok: guarda })
    }

    #[test]
    fn explorar_goes_down_only_with_ninety_percent_on_haiku() {
        let nove_de_dez: Vec<EcoPane> = (0..10).map(|i| pane("mecanico", "claude-haiku-5-5", i64::from(i < 1))).collect();
        let a = explorar(&nove_de_dez, true, "claude-sonnet-5-5").expect("90% no limite");
        assert!(!a.so_proposta, "explorar desce sozinho");
        assert_eq!(a.patch_after, json!({"modelos": {"explorar": {"model": HAIKU, "effort": "low"}}}));
        let oito: Vec<EcoPane> = (0..10).map(|i| pane("mecanico", "claude-haiku-5-5", i64::from(i < 2))).collect();
        assert!(explorar(&oito, true, "claude-sonnet-5-5").is_none(), "80% não basta");
        assert!(explorar(&nove_de_dez[..9], true, "claude-sonnet-5-5").is_none(), "9 painéis: sem amostra");
        assert!(explorar(&nove_de_dez, false, "claude-sonnet-5-5").is_none(), "guarda de qualidade vermelha bloqueia");
        assert!(explorar(&nove_de_dez, true, HAIKU).is_none(), "já está no barato");
        let sonnet: Vec<EcoPane> = (0..10).map(|_| pane("mecanico", "claude-sonnet-5-5", 0)).collect();
        assert!(explorar(&sonnet, true, "claude-sonnet-5-5").is_none(), "só conta painel no barato");
    }

    #[test]
    fn r15_planejar_and_revisar_are_never_lowered() {
        let limpos: Vec<EcoPane> = (0..10).map(|_| pane("mecanico", "claude-haiku-5-5", 0)).collect();
        let mut p = eq();
        p.modelos.planejar.model = "claude-opus-5-5".into();
        p.modelos.revisar.model = "claude-opus-5-5".into();
        let (ed, m, f) = (BTreeMap::new(), Vec::<Vec<FaseRow>>::new(), Vec::<Ajuste>::new());
        let a = explorar_barato(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &[], panes: &limpos, paineis_7d: 10, missoes: &m, feed: &f, guarda_ok: true }).unwrap();
        let tocados: Vec<&String> = a.patch_after["modelos"].as_object().unwrap().keys().collect();
        assert_eq!(tocados, vec!["explorar"]);
    }

    fn fases_de(verif: i64, outras: i64) -> Vec<FaseRow> {
        vec![fase("execucao", 0, Some(outras)), fase(VERIFICACAO, outras, Some(outras + verif))]
    }

    #[test]
    fn testes_relacionados_needs_three_missions_over_forty_percent() {
        let mk = |missoes: &[Vec<FaseRow>]| {
            // o equilibrado já vem com relacionadosPrimeiro ligado: o caso de teste parte de desligado
            let mut p = eq();
            p.testes.relacionados_primeiro = false;
            let (ed, f) = (BTreeMap::new(), Vec::<Ajuste>::new());
            testes_relacionados(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &[], panes: &[], paineis_7d: 10, missoes, feed: &f, guarda_ok: true })
        };
        // 40 % exatos não passam (limite estrito)
        let quarenta: Vec<Vec<FaseRow>> = (0..3).map(|_| fases_de(40, 60)).collect();
        assert!(mk(&quarenta).is_none());
        let cinquenta: Vec<Vec<FaseRow>> = (0..3).map(|_| fases_de(50, 50)).collect();
        let a = mk(&cinquenta).expect("50% com 3 missões");
        assert_eq!(a.patch_after, json!({"testes": {"relacionadosPrimeiro": true}}));
        assert!(mk(&cinquenta[..2]).is_none(), "duas missões não bastam");
        let ligado = {
            let mut p = eq();
            p.testes.relacionados_primeiro = true;
            let (ed, f) = (BTreeMap::new(), Vec::<Ajuste>::new());
            testes_relacionados(&Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &[], panes: &[], paineis_7d: 10, missoes: &cinquenta, feed: &f, guarda_ok: true })
        };
        assert!(ligado.is_none());
    }

    #[test]
    fn limites_saida_step_down_with_floors() {
        let mk = |n: i64, p: &Perfil| {
            let (ed, m, f) = (BTreeMap::new(), Vec::<Vec<FaseRow>>::new(), Vec::<Ajuste>::new());
            let a = vec![achado("huge_output", n)];
            limites_saida(&Entrada { agora: AGORA, perfil: p, patch: None, editados: &ed, achados: &a, panes: &[], paineis_7d: 10, missoes: &m, feed: &f, guarda_ok: true })
        };
        assert!(mk(4, &eq()).is_none());
        let a = mk(5, &eq()).expect("5 saídas enormes");
        assert_eq!(a.patch_after, json!({"saidas": {"resumirAcima": 9000, "bashMax": 15000}}));
        let mut no_piso = eq();
        no_piso.saidas.resumir_acima = 4000;
        no_piso.saidas.bash_max = Some(8000);
        assert!(mk(9, &no_piso).is_none(), "nos pisos não desce mais");
    }

    #[test]
    fn k_learning_is_a_delta_that_clamps_and_needs_samples() {
        let bons: Vec<(f64, f64)> = vec![(2.0, 1.0); 5];
        let ruins: Vec<(f64, f64)> = vec![(0.1, 1.0); 5];
        assert_eq!(aprender_k(0.0, &bons[..4]), None, "4 amostras");
        assert_eq!(aprender_k(0.0, &bons), Some(0.5));
        assert_eq!(aprender_k(0.0, &ruins), Some(-0.5));
        assert_eq!(aprender_k(K_DELTA_MIN, &ruins), None, "piso do delta");
        assert_eq!(aprender_k(K_DELTA_MAX, &bons), None, "teto do delta");
        let misto: Vec<(f64, f64)> = vec![(2.0, 1.0), (0.6, 1.0), (0.6, 1.0), (2.0, 1.0), (0.6, 1.0)];
        assert_eq!(aprender_k(0.0, &misto), None, "sem maioria de 3");
    }

    #[test]
    fn k_goes_down_when_the_estimate_said_not_worth_it() {
        // estimado ≤ 0: a conta já dizia que não compensava; estimativa negativa nunca conta como boa
        assert_eq!(aprender_k(0.0, &vec![(0.0, -1.0); 5]), Some(-0.5));
        assert_eq!(aprender_k(0.0, &vec![(2.0, -1.0); 5]), Some(-0.5));
    }

    #[test]
    fn medidas_k_keeps_the_negative_estimates() {
        let ajuste = compactacao(json!({"G": 0.5, "C": 1.0}), json!({"turnos": 1, "custoSemAcao": 2.0, "custoReal": 0.1}));
        let medidas = medidas_k(&[ajuste]);
        assert_eq!(medidas.len(), 1);
        assert!(medidas[0].1 < 0.0, "estimado 1·0,5 − 1 = −0,5");
    }

    #[test]
    fn restante_factor_learning_uses_the_median_and_clamps() {
        assert_eq!(aprender_fator(1.0, &[2.0; 4]), None);
        assert_eq!(aprender_fator(1.0, &[2.0; 5]), Some(2.0));
        // a média seria 2,8; a mediana (1) não move o fator
        assert_eq!(aprender_fator(1.0, &[10.0, 1.0, 1.0, 1.0, 1.0]), None);
        assert_eq!(aprender_fator(2.0, &[10.0; 5]), Some(FATOR_MAX));
        assert_eq!(aprender_fator(1.0, &[0.1; 5]), Some(FATOR_MIN));
    }

    fn compactacao(est: Value, efeito: Value) -> Ajuste {
        Ajuste {
            id: new_id(), created_at: AGORA, workspace_id: Some("ws".into()), mission_id: None, pane_id: None, kind: "compactacao".into(),
            acao: "aplicado".into(), motivo: String::new(), antes_json: None, depois_json: None, estimativa_json: Some(est.to_string()),
            efeito_json: Some(efeito.to_string()), desfazivel: false, desfeito_at: None,
        }
    }

    fn registra_compactacao(s: &AppState, est: Value) -> String {
        ajustes::registrar(s, Novo {
            workspace_id: Some("ws".into()), mission_id: None, pane_id: None, kind: "compactacao", acao: "aplicado",
            motivo: "teste".into(), antes: None, depois: None, estimativa: Some(est), desfazivel: false,
        })
        .unwrap()
    }

    #[test]
    fn learning_waits_for_the_late_measure_and_never_counts_twice() {
        let (_d, s) = state();
        let agora = now_ms();
        let est = json!({"C": 1.0, "G": 0.5, "R": 9.0, "turnos": 2});
        let ids: Vec<String> = (0..5).map(|_| registra_compactacao(&s, est.clone())).collect();
        // nenhuma medida ainda: a primeira linha segura o marco e nada é aprendido
        aprender(&s, agora).unwrap();
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.k, None);
        for id in &ids {
            s.db.set_ajuste_efeito(id, &json!({"turnos": 5, "custoReal": 0.1, "custoSemAcao": 2.0}).to_string()).unwrap();
        }
        aprender(&s, agora).unwrap();
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.k, Some(0.5), "medidas tardias entram");
        // dois dias depois, sem linha nova: as mesmas amostras não contam de novo
        aprender(&s, agora + 2 * DIA_MS).unwrap();
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.k, Some(0.5), "cada amostra vale uma vez");
    }

    #[test]
    fn a_rule_counts_only_samples_newer_than_its_last_action() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let agora = now_ms();
        let achados: Vec<WasteIns> = (0..5).map(|_| WasteIns { pane: "p1", workspace: &ws, session: None, kind: "huge_output", tokens: 10, evidence: None, at: agora - 3_600_000 }).collect();
        s.db.waste_commit("sessao.jsonl", false, &achados, WasteOff { pos: 0, len: 0, head: 0 }).unwrap();
        rodar_regra(&s, &ws, "limites_saida", limites_saida, "aplicar", agora, &mut Cache::default()).unwrap();
        let depois = lk(&s.settings).perfis.patches.get(&ws).cloned();
        assert!(depois.is_some(), "5 saídas enormes aplicam");
        // no dia seguinte, sem achado novo, os mesmos 5 já foram contados: nada acontece
        rodar_regra(&s, &ws, "limites_saida", limites_saida, "aplicar", agora + DIA_MS + 1_000, &mut Cache::default()).unwrap();
        assert_eq!(lk(&s.settings).perfis.patches.get(&ws).cloned(), depois);
        assert_eq!(s.db.list_ajustes(Some(&ws), 10).unwrap().len(), 1);
    }

    #[test]
    fn undo_and_ignore_keep_the_rule_from_coming_back() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        aplicar(&s, &ws, acao_de_teste(json!({"indiceCodigo": true})), "aplicar").unwrap();
        let id = s.db.list_ajustes(Some(&ws), 5).unwrap()[0].id.clone();
        ajustes::desfazer(&s, &id).unwrap();
        let agora = now_ms();
        let de_novo = acao_de_teste(json!({"indiceCodigo": true}));
        assert!(vetada(&s, &ws, &de_novo, &[], agora + DIA_MS), "24 h depois ainda vetada");
        assert!(!vetada(&s, &ws, &de_novo, &[], agora + 31 * DIA_MS), "a recusa expira em 30 dias");
        // proposta ignorada: sai do feed como pulado e não volta
        let outra = Acao { regra: "limites_saida", kind: "limites_saida", estimativa: json!({"regra": "limites_saida"}), ..acao_de_teste(json!({"saidas": {"bashMax": 9000}})) };
        aplicar(&s, &ws, outra.clone(), "propor").unwrap();
        let id = s.db.list_ajustes(Some(&ws), 5).unwrap()[0].id.clone();
        ignorar(&s, &id).unwrap();
        assert_eq!(s.db.get_ajuste(&id).unwrap().acao, "pulado");
        assert!(vetada(&s, &ws, &outra, &s.db.list_ajustes(Some(&ws), 10).unwrap(), agora), "ignorada: sem nova proposta");
    }

    #[test]
    fn an_open_proposal_is_not_proposed_again() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let a = acao_de_teste(json!({"indiceCodigo": true}));
        aplicar(&s, &ws, a.clone(), "propor").unwrap();
        let feed = s.db.list_ajustes(Some(&ws), 5).unwrap();
        assert!(vetada(&s, &ws, &a, &feed, now_ms()));
    }

    #[test]
    fn unknown_mode_proposes_and_never_applies() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let modo = AutoAjustePrefs { modo: "talvez".into(), ..Default::default() };
        assert_eq!(modo.modo(), "propor");
        aplicar(&s, &ws, acao_de_teste(json!({"indiceCodigo": true})), modo.modo()).unwrap();
        assert!(lk(&s.settings).perfis.patches.get(&ws).is_none());
        assert_eq!(s.db.list_ajustes(Some(&ws), 5).unwrap()[0].acao, "proposto");
    }

    #[test]
    fn explorar_barato_only_touches_the_claude_cli() {
        let limpos: Vec<EcoPane> = (0..10).map(|_| pane("mecanico", "claude-haiku-5-5", 0)).collect();
        let mut p = eq();
        p.modelos.explorar.model = "claude-sonnet-5-5".into();
        p.modelos.explorar.cli = "codex".into();
        let (ed, m, f) = (BTreeMap::new(), Vec::<Vec<FaseRow>>::new(), Vec::<Ajuste>::new());
        let e = Entrada { agora: AGORA, perfil: &p, patch: None, editados: &ed, achados: &[], panes: &limpos, paineis_7d: 10, missoes: &m, feed: &f, guarda_ok: true };
        assert!(explorar_barato(&e).is_none(), "Haiku só existe no Claude");
        assert!(explorar(&limpos, true, "claude-sonnet-5-5").is_some(), "no Claude desce");
    }

    #[test]
    fn a_second_tick_while_one_runs_returns_at_once() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let _passo = PASSO.lock().unwrap();
        tick(&s).unwrap();
        assert!(lk(&s.settings).perfis.patches.get(&ws).is_none());
        assert!(s.db.list_ajustes(None, 10).unwrap().is_empty());
    }

    #[test]
    fn a_change_too_big_to_undo_is_only_proposed() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let grande = json!({"indiceCodigo": true, "x": "y".repeat(5000)});
        aplicar(&s, &ws, acao_de_teste(grande), "aplicar").unwrap();
        assert!(lk(&s.settings).perfis.patches.get(&ws).is_none(), "não aplica o que não se desfaz");
        assert_eq!(s.db.list_ajustes(Some(&ws), 5).unwrap()[0].acao, "proposto");
    }

    #[test]
    fn undo_of_an_older_stacked_change_names_the_newer_one() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        aplicar(&s, &ws, acao_de_teste(json!({"indiceCodigo": true})), "aplicar").unwrap();
        let primeiro = s.db.list_ajustes(Some(&ws), 5).unwrap()[0].id.clone();
        let mut segundo = acao_de_teste(json!({"indiceCodigo": true, "testes": {"relacionadosPrimeiro": true}}));
        segundo.patch_before = Some(json!({"indiceCodigo": true}));
        segundo.motivo = "testes primeiro".into();
        segundo.regra = "testes_relacionados";
        segundo.kind = "testes_relacionados";
        aplicar(&s, &ws, segundo, "aplicar").unwrap();
        let msg = ajustes::desfazer(&s, &primeiro).unwrap_err().to_string();
        assert!(msg.contains("Desfaça primeiro o ajuste mais recente (testes primeiro)"), "{msg}");
    }

    #[test]
    fn zerar_keeps_only_the_watermark() {
        let (_d, s) = state();
        {
            let mut cur = lk(&s.settings);
            cur.auto_ajuste.aprendido = Aprendido { k: Some(1.5), r_resumo: Some(0.1), fator_restante: Some(2.0), visto: 42 };
        }
        zerar_aprendizado(&s).unwrap();
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido, Aprendido { visto: 42, ..Aprendido::default() });
    }

    #[test]
    fn k_efetivo_adds_the_delta_to_the_profile_and_clamps() {
        assert_eq!(k_efetivo(3, None), 3.0);
        assert_eq!(k_efetivo(3, Some(0.5)), 3.5);
        assert_eq!(k_efetivo(3, Some(-7.0)), K_MIN);
        assert_eq!(k_efetivo(3, Some(7.0)), K_MAX);
        // K de perfil acima do teto não é cortado sem delta; com delta, o teto é o próprio K
        assert_eq!(k_efetivo(10, None), 10.0);
        assert_eq!(k_efetivo(10, Some(7.0)), 10.0);
    }

    fn state() -> (tempfile::TempDir, Arc<AppState>) {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        (d, s)
    }

    fn acao_de_teste(depois: Value) -> Acao {
        Acao { regra: "indice_codigo", kind: "indice_codigo", patch_before: None, patch_after: depois, motivo: "teste".into(), estimativa: json!({"regra": "indice_codigo"}), so_proposta: false }
    }

    #[test]
    fn desligado_writes_nothing() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        lk(&s.settings).auto_ajuste.modo = "desligado".into();
        let antes = lk(&s.settings).clone();
        tick(&s).unwrap();
        assert_eq!(*lk(&s.settings), antes);
        assert!(s.db.list_ajustes(None, 10).unwrap().is_empty());
    }

    #[test]
    fn apply_then_undo_restores_the_exact_previous_patch() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        perfil::set_patch(&s, &ws, Some(json!({"saidas": {"bashMax": 5000}}))).unwrap();
        let antes = lk(&s.settings).perfis.patches.get(&ws).cloned();
        let depois = json!({"saidas": {"bashMax": 5000}, "indiceCodigo": true});
        aplicar(&s, &ws, Acao { patch_before: antes.clone(), ..acao_de_teste(depois.clone()) }, "aplicar").unwrap();
        assert_eq!(lk(&s.settings).perfis.patches.get(&ws), Some(&depois));
        let id = s.db.list_ajustes(Some(&ws), 5).unwrap()[0].id.clone();
        ajustes::desfazer(&s, &id).unwrap();
        assert_eq!(lk(&s.settings).perfis.patches.get(&ws).cloned(), antes);
    }

    #[test]
    fn propor_only_proposes_and_the_proposal_applies_and_undoes() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let depois = json!({"indiceCodigo": true});
        aplicar(&s, &ws, acao_de_teste(depois.clone()), "propor").unwrap();
        assert!(lk(&s.settings).perfis.patches.get(&ws).is_none(), "propor não escreve");
        let id = s.db.list_ajustes(Some(&ws), 5).unwrap()[0].id.clone();
        assert_eq!(s.db.get_ajuste(&id).unwrap().acao, "proposto");
        aplicar_proposta(&s, &id).unwrap();
        assert_eq!(lk(&s.settings).perfis.patches.get(&ws), Some(&depois));
        assert!(s.db.get_ajuste(&id).unwrap().desfazivel);
        ajustes::desfazer(&s, &id).unwrap();
        assert!(lk(&s.settings).perfis.patches.get(&ws).is_none());
    }

    #[test]
    fn proposal_refuses_when_the_patch_changed_since() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        aplicar(&s, &ws, acao_de_teste(json!({"indiceCodigo": true})), "propor").unwrap();
        let id = s.db.list_ajustes(Some(&ws), 5).unwrap()[0].id.clone();
        perfil::set_patch(&s, &ws, Some(json!({"saidas": {"bashMax": 1}}))).unwrap();
        assert_eq!(aplicar_proposta(&s, &id).unwrap_err().code(), "stale");
    }

    #[test]
    fn learning_persists_k_once_per_day() {
        let (_d, s) = state();
        let agora = now_ms();
        for _ in 0..5 {
            let id = ajustes::registrar(&s, Novo {
                workspace_id: Some("ws".into()), mission_id: None, pane_id: None, kind: "compactacao", acao: "aplicado",
                motivo: "teste".into(), antes: None, depois: None, estimativa: Some(json!({"C": 1.0, "G": 0.5, "R": 9.0, "turnos": 2})), desfazivel: false,
            }).unwrap();
            s.db.set_ajuste_efeito(&id, &json!({"turnos": 5, "custoReal": 0.1, "custoSemAcao": 2.0}).to_string()).unwrap();
        }
        aprender(&s, agora).unwrap();
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.k, Some(0.5));
        aprender(&s, agora).unwrap();
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.k, Some(0.5), "uma vez por dia");
    }

    #[test]
    fn propor_never_writes_learned_params() {
        let (_d, s) = state();
        lk(&s.settings).auto_ajuste.modo = "propor".into();
        guardar_r_resumo(&s, 0.07);
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.r_resumo, None);
        lk(&s.settings).auto_ajuste.modo = "aplicar".into();
        guardar_r_resumo(&s, 0.07);
        assert_eq!(lk(&s.settings).auto_ajuste.aprendido.r_resumo, Some(0.07));
    }
}
