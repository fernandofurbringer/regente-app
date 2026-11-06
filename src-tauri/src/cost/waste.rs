//! Desperdício de tokens (RM-21, H1-10): detectores sobre o JSONL da sessão do Claude.
//! Só lê arquivos; roda na thread do comando (Uso), nunca no caminho da PTY.
//! Os achados são agregados por arquivo. Tokens são estimativas (bytes / 4).

use super::claude::{find_session_file, subagent_files};
use crate::db::{WasteIns, WasteOff};
use crate::error::AppResult;
use crate::perfil;
use crate::runtime::AppState;
use crate::util::now_ms;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::UNIX_EPOCH;

/// Limites dos detectores.
const REPETIDA_MIN: u32 = 3;
const SAIDA_GRANDE: u64 = 10_000;
const CHURN_ESCRITA: u64 = 20_000;
/// Cache lido abaixo de 10% do contexto anterior é churn.
const CHURN_LEITURA_PCT: u64 = 10;
/// TTL de 5 minutos: intervalo maior que isso seguido de escrita grande é cache frio.
const TTL_MS: i64 = 5 * 60_000;
const SEQUENCIA_MIN: usize = 3;
/// Evento (compactação, reinício, troca de perfil) até 2 min antes de um churn explica o churn.
const CAUSA_MS: i64 = 2 * 60_000;
/// Primeira passada de um arquivo: turnos mais velhos que isto são ignorados.
const PRIMEIRA_PASSADA_MS: i64 = 30 * 86_400_000;
/// Linha de JSONL acima disso é pulada até a próxima quebra (não cabe em memória com folga).
pub const LINHA_MAX: usize = 8 * 1024 * 1024;
/// Quantos bytes do começo identificam o arquivo (rotação).
const CABECA: u64 = 64;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub kind: &'static str,
    pub severity: &'static str,
    pub tokens_est: u64,
    pub evidence: Vec<String>,
    pub suggestion_key: &'static str,
    /// Instante do evento que gerou o achado (timestamp do turno; mtime do arquivo quando o turno não tem).
    pub at: i64,
}

/// O que uma passada precisa saber além do arquivo.
pub struct Escopo {
    /// Janela de contexto a partir da qual o turno conta como inchado.
    pub janela: u64,
    /// Instante do arquivo: vale para turnos sem timestamp.
    pub mtime: i64,
    /// Turnos anteriores a isto são ignorados (primeira passada).
    pub corte: Option<i64>,
    /// Eventos do painel com instante: (ms, causa) para explicar churns.
    pub eventos: Vec<(i64, &'static str)>,
}

impl Escopo {
    pub fn simples(janela: u64) -> Self {
        Self { janela, mtime: 0, corte: None, eventos: vec![] }
    }
}

#[derive(Clone, Default)]
struct Turn {
    ts: Option<i64>,
    model: String,
    input: u64,
    cache_r: u64,
    cache_w: u64,
}

#[derive(Clone)]
struct Call {
    id: String,
    name: String,
    key: String,
    path: Option<String>,
}

/// Estado de uma passada sobre um JSONL. Linhas inválidas e blocos desconhecidos são ignorados.
// ponytail: o estado não sobrevive entre passadas (o offset cobre o arquivo todo); um turno na fronteira pode contar a menos.
pub struct Analyzer {
    e: Escopo,
    turns: Vec<Turn>,
    turn_of: HashMap<String, usize>,
    calls: Vec<Call>,
    call_ids: HashSet<String>,
    /// tool_use id -> (é erro, tamanho do texto do resultado em bytes)
    results: HashMap<String, (bool, u64)>,
    /// tool_use de turnos antigos (fora do corte): os resultados deles também são ignorados.
    antigos: HashSet<String>,
    /// Instante do último turno lido (ou mtime).
    ultimo_at: i64,
}

impl Analyzer {
    pub fn new(e: Escopo) -> Self {
        let ultimo_at = e.mtime;
        Self { e, turns: vec![], turn_of: HashMap::new(), calls: vec![], call_ids: HashSet::new(), results: HashMap::new(), antigos: HashSet::new(), ultimo_at }
    }

    /// Uma linha do JSONL. Pré-filtro por substring antes do parse: a maioria das linhas não interessa.
    pub fn feed(&mut self, line: &str) {
        let usage = line.contains("\"usage\"");
        let resultado = line.contains("\"tool_result\"");
        if !usage && !resultado {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        match v["type"].as_str() {
            Some("assistant") if usage => self.assistente(&v),
            Some("user") if resultado => self.resultados(&v),
            _ => {}
        }
    }

    fn assistente(&mut self, v: &Value) {
        let m = &v["message"];
        let (Some(id), Some(u)) = (m["id"].as_str(), m["usage"].as_object()) else { return };
        let model = m["model"].as_str().unwrap_or_default().to_string();
        // mensagens sintéticas do CLI (modelo `<...>`) não são turnos
        if model.starts_with('<') {
            return;
        }
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let turno = Turn { ts: v["timestamp"].as_str().and_then(parse_ts), model, input: n("input_tokens"), cache_r: n("cache_read_input_tokens"), cache_w: n("cache_creation_input_tokens") };
        if let (Some(corte), Some(t)) = (self.e.corte, turno.ts) {
            if t < corte {
                if let Some(blocos) = m["content"].as_array() {
                    self.antigos.extend(blocos.iter().filter(|b| b["type"] == "tool_use").filter_map(|b| b["id"].as_str().map(String::from)));
                }
                return;
            }
        }
        self.ultimo_at = turno.ts.unwrap_or(self.e.mtime);
        // o mesmo message.id repete por bloco: o último uso vence, o instante é o da primeira linha
        match self.turn_of.get(id) {
            Some(&i) => {
                let ts = self.turns[i].ts;
                self.turns[i] = Turn { ts, ..turno };
            }
            None => {
                self.turn_of.insert(id.to_string(), self.turns.len());
                self.turns.push(turno);
            }
        }
        let Some(blocos) = m["content"].as_array() else { return };
        for b in blocos.iter().filter(|b| b["type"] == "tool_use") {
            let Some(cid) = b["id"].as_str() else { continue };
            if !self.call_ids.insert(cid.to_string()) {
                continue;
            }
            let input = &b["input"];
            self.calls.push(Call { id: cid.into(), name: b["name"].as_str().unwrap_or_default().into(), key: input.to_string(), path: input["file_path"].as_str().map(String::from) });
        }
    }

    fn resultados(&mut self, v: &Value) {
        let Some(blocos) = v["message"]["content"].as_array() else { return };
        for b in blocos.iter().filter(|b| b["type"] == "tool_result") {
            let Some(id) = b["tool_use_id"].as_str() else { continue };
            if self.antigos.contains(id) {
                continue;
            }
            self.results.insert(id.to_string(), (b["is_error"].as_bool().unwrap_or(false), tamanho_texto(&b["content"])));
        }
    }

    /// Tokens estimados de um resultado (bytes / 4).
    fn tokens_de(&self, id: &str) -> u64 {
        self.results.get(id).map_or(0, |r| r.1 / 4)
    }

    /// Instante de um turno (mtime quando o turno não tem timestamp).
    fn at_turno(&self, t: &Turn) -> i64 {
        t.ts.unwrap_or(self.e.mtime)
    }

    /// Causa do churn pelo evento registrado até `CAUSA_MS` antes dele.
    fn causa(&self, ts: Option<i64>) -> Option<&'static str> {
        let t = ts?;
        self.e.eventos.iter().filter(|(e, _)| *e <= t && t - *e <= CAUSA_MS).max_by_key(|(e, _)| *e).map(|(_, c)| *c)
    }

    pub fn finish(self) -> Vec<Finding> {
        [self.repetidas(), self.saida_grande(), self.churn(), self.cache_frio(), self.sequencias(), self.contexto_inchado()].into_iter().flatten().collect()
    }

    /// Mesmo `Read.file_path` pelo menos 3 vezes sem Edit/Write no meio.
    fn repetidas(&self) -> Option<Finding> {
        let mut leituras: HashMap<&str, (u32, u64)> = HashMap::new();
        let mut achados: Vec<(String, u32, u64)> = vec![];
        for c in &self.calls {
            let Some(p) = c.path.as_deref() else { continue };
            match c.name.as_str() {
                "Read" => {
                    let e = leituras.entry(p).or_insert((0, 0));
                    e.0 += 1;
                    if e.0 > 1 {
                        e.1 += self.tokens_de(&c.id);
                    }
                }
                "Edit" | "Write" | "MultiEdit" => {
                    if let Some((n, t)) = leituras.remove(p) {
                        if n >= REPETIDA_MIN {
                            achados.push((p.to_string(), n, t));
                        }
                    }
                }
                _ => {}
            }
        }
        for (p, (n, t)) in leituras {
            if n >= REPETIDA_MIN {
                achados.push((p.to_string(), n, t));
            }
        }
        if achados.is_empty() {
            return None;
        }
        achados.sort_by(|a, b| b.2.cmp(&a.2));
        let tokens = achados.iter().map(|a| a.2).sum();
        let evidence = achados.iter().take(5).map(|(p, n, _)| format!("{p} lido {n} vezes sem edição")).collect();
        Some(finding("repeated_read", tokens, evidence, "indice_codigo", self.ultimo_at))
    }

    /// Resultado de ferramenta com mais de 10k tokens estimados.
    fn saida_grande(&self) -> Option<Finding> {
        let grandes: Vec<u64> = self.results.values().map(|r| r.1 / 4).filter(|t| *t > SAIDA_GRANDE).collect();
        if grandes.is_empty() {
            return None;
        }
        let tokens = grandes.iter().sum();
        Some(finding("huge_output", tokens, vec![format!("{} saídas acima de {SAIDA_GRANDE} tokens estimados", grandes.len())], "limites_saida", self.ultimo_at))
    }

    /// Escrita de cache grande com leitura de cache mínima em relação ao contexto anterior.
    fn churn(&self) -> Option<Finding> {
        let (mut n, mut trocas, mut tokens, mut at) = (0u32, 0u32, 0u64, self.e.mtime);
        let (mut comp, mut rec, mut perf, mut sem) = (0u32, 0u32, 0u32, 0u32);
        for par in self.turns.windows(2) {
            let (prev, cur) = (&par[0], &par[1]);
            let ctx_prev = prev.input + prev.cache_r + prev.cache_w;
            if cur.cache_w > CHURN_ESCRITA && cur.cache_r * 100 < ctx_prev * CHURN_LEITURA_PCT {
                n += 1;
                tokens += cur.cache_w;
                at = self.at_turno(cur);
                if cur.model != prev.model {
                    trocas += 1;
                    continue;
                }
                match self.causa(cur.ts) {
                    Some("compactação") => comp += 1,
                    Some("reinício") => rec += 1,
                    Some(_) => perf += 1,
                    None => sem += 1,
                }
            }
        }
        if n == 0 {
            return None;
        }
        let causa = format!("{trocas} com troca de modelo; {comp} após compactação; {rec} após reinício; {perf} após troca de perfil; {sem} sem causa identificada");
        Some(finding("cache_churn", tokens, vec![format!("{n} churns de cache: {causa}")], "escolher_modelo", at))
    }

    /// Intervalo maior que o TTL seguido de escrita grande de cache.
    fn cache_frio(&self) -> Option<Finding> {
        let (mut n, mut tokens, mut at) = (0u32, 0u64, self.e.mtime);
        for par in self.turns.windows(2) {
            let (prev, cur) = (&par[0], &par[1]);
            let (Some(a), Some(b)) = (prev.ts, cur.ts) else { continue };
            if b - a > TTL_MS && cur.cache_w > CHURN_ESCRITA {
                n += 1;
                tokens += cur.cache_w;
                at = self.at_turno(cur);
            }
        }
        (n > 0).then(|| finding("cold_cache_gap", tokens, vec![format!("{n} retomadas depois de mais de 5 min parado")], "cache_ttl", at))
    }

    /// Três ou mais chamadas idênticas seguidas, ou erro seguido da mesma chamada.
    fn sequencias(&self) -> Option<Finding> {
        let (mut n, mut tokens) = (0u32, 0u64);
        let mut i = 0;
        while i < self.calls.len() {
            let mut j = i + 1;
            while j < self.calls.len() && self.calls[j].name == self.calls[i].name && self.calls[j].key == self.calls[i].key {
                j += 1;
            }
            let run = j - i;
            let erro = self.results.get(&self.calls[i].id).is_some_and(|r| r.0);
            if run >= SEQUENCIA_MIN || (run >= 2 && erro) {
                n += 1;
                tokens += (i + 1..j).map(|k| self.tokens_de(&self.calls[k].id)).sum::<u64>();
            }
            i = j;
        }
        (n > 0).then(|| finding("retry_loop", tokens, vec![format!("{n} sequências de chamadas idênticas")], "dividir_tarefa", self.ultimo_at))
    }

    /// Turnos com contexto acima da janela (o excesso é o que se paga a mais).
    fn contexto_inchado(&self) -> Option<Finding> {
        let mut n = 0u32;
        let (mut excesso, mut pico, mut at) = (0u64, 0u64, self.e.mtime);
        for t in &self.turns {
            let ctx = t.input + t.cache_r + t.cache_w;
            if ctx > self.e.janela {
                n += 1;
                excesso += ctx - self.e.janela;
                pico = pico.max(ctx);
                at = self.at_turno(t);
            }
        }
        (n > 0).then(|| finding("bloated_context", excesso, vec![format!("{n} turnos acima de {} tokens; pico {pico}", self.e.janela)], "compactacao", at))
    }
}

fn finding(kind: &'static str, tokens: u64, evidence: Vec<String>, suggestion_key: &'static str, at: i64) -> Finding {
    let severity = if tokens >= 100_000 {
        "alta"
    } else if tokens >= 20_000 {
        "media"
    } else {
        "baixa"
    };
    Finding { kind, severity, tokens_est: tokens, evidence, suggestion_key, at }
}

/// Tamanho do texto de um resultado: string direta ou blocos `text`. Imagens e outros blocos não contam.
fn tamanho_texto(c: &Value) -> u64 {
    match c {
        Value::String(s) => s.len() as u64,
        Value::Array(blocos) => blocos.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).map(|t| t.len() as u64).sum(),
        _ => 0,
    }
}

/// Limite de contexto dos achados: a janela longa do perfil, ou 80% da janela que a statusline informa quando ela é menor.
pub fn limite_contexto(janela_perfil: u64, janela_cli: Option<i64>) -> u64 {
    match janela_cli.filter(|w| *w > 0) {
        Some(w) => janela_perfil.min(w as u64 * 8 / 10),
        None => janela_perfil,
    }
}

/// `2026-10-15T12:00:00.123Z` -> milissegundos desde a época. None se o formato não bate.
pub fn parse_ts(s: &str) -> Option<i64> {
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let frac = s.get(20..).unwrap_or("").trim_end_matches('Z');
    let digitos: String = frac.chars().take(3).take_while(char::is_ascii_digit).collect();
    let ms = digitos.parse::<i64>().unwrap_or(0) * 10i64.pow(3 - digitos.len() as u32);
    Some(days_from_civil(y, mo, d) * 86_400_000 + h * 3_600_000 + mi * 60_000 + se * 1000 + ms)
}

/// Dias desde 1970-01-01 (algoritmo de civil_from_days, sem fuso).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Lê uma linha com teto de `max` bytes guardados. Devolve (bytes consumidos, terminou com quebra, passou do teto).
/// Linha longa é consumida até a quebra mesmo assim, sem guardar o texto.
fn ler_linha(rd: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> std::io::Result<(u64, bool, bool)> {
    buf.clear();
    let (mut total, mut longa) = (0u64, false);
    loop {
        let chunk = rd.fill_buf()?;
        if chunk.is_empty() {
            return Ok((total, false, longa));
        }
        let (usar, achou) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        if !longa {
            if buf.len() + usar > max {
                longa = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..usar]);
            }
        }
        rd.consume(usar);
        total += usar as u64;
        if achou {
            return Ok((total, true, longa));
        }
    }
}

/// Resultado de uma passada: achados, até onde foi lido (só linhas completas), tamanho do arquivo e linhas puladas.
pub struct Passada {
    pub achados: Vec<Finding>,
    pub pos: u64,
    pub len: u64,
    pub ignoradas: u64,
}

/// Lê o JSONL a partir de `from` (só linhas completas) e devolve os achados e o novo offset.
pub fn scan_file(path: &Path, from: u64, e: Escopo) -> std::io::Result<Passada> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    // arquivo menor que o offset guardado: foi recriado, começa do zero
    let from = if from > len { 0 } else { from };
    f.seek(SeekFrom::Start(from))?;
    let mut rd = BufReader::new(f);
    let mut an = Analyzer::new(e);
    let (mut pos, mut ignoradas) = (from, 0u64);
    let mut buf = Vec::new();
    loop {
        let (n, completa, longa) = ler_linha(&mut rd, &mut buf, LINHA_MAX)?;
        // a última linha sem quebra ainda está sendo escrita: fica para a próxima passada
        if !completa {
            break;
        }
        pos += n;
        if longa {
            ignoradas += 1;
            continue;
        }
        if let Ok(line) = std::str::from_utf8(&buf) {
            an.feed(line);
        }
    }
    Ok(Passada { achados: an.finish(), pos, len, ignoradas })
}

/// Hash FNV-1a dos primeiros `n` bytes (no máximo `CABECA`). Identifica o começo do arquivo: ele não muda enquanto o arquivo só cresce.
fn hash_inicio(path: &Path, n: u64) -> std::io::Result<u64> {
    let mut buf = vec![0u8; n.min(CABECA) as usize];
    std::fs::File::open(path)?.read_exact(&mut buf)?;
    Ok(buf.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3)))
}

/// Eventos com instante do painel que explicam churns: compactação, reinício (recomeço) e troca de perfil.
fn eventos_do_painel(s: &AppState, pane: &str, ws: &str) -> AppResult<Vec<(i64, &'static str)>> {
    let ajustes = s.db.list_ajustes(Some(ws), 200)?;
    Ok(ajustes
        .iter()
        .filter(|a| a.acao == "aplicado" && a.pane_id.as_deref().is_none_or(|p| p == pane))
        .filter_map(|a| {
            let causa = match a.kind.as_str() {
                "compactacao" => "compactação",
                "recomeco" => "reinício",
                "perfil_patch" => "troca de perfil",
                _ => return None,
            };
            Some((a.created_at, causa))
        })
        .collect())
}

/// Serializa as atualizações: duas chamadas simultâneas não gravam os mesmos achados duas vezes.
static PASSADA: Mutex<()> = Mutex::new(());

/// Atualiza os achados de todos os painéis Claude locais (sessão + subagentes), a partir do último offset.
pub fn refresh(s: &AppState) -> AppResult<()> {
    let _uma_por_vez = PASSADA.lock().unwrap_or_else(PoisonError::into_inner);
    let agora = now_ms();
    for p in s.db.claude_panes()? {
        if crate::remote::parse_ws_path(&p.cwd).is_some() {
            continue; // painel remoto: o JSONL fica na máquina remota (fora desta versão)
        }
        let Some(sid) = p.session_id.as_deref() else { continue };
        let dir = s.config_dir_for("claude", p.account.as_deref());
        let Some(file) = find_session_file(&dir, sid) else { continue };
        let sensivel = s.is_sensitive(&p.workspace_id);
        if sensivel {
            s.db.waste_clear_evidence(&p.workspace_id)?;
        }
        let janela_perfil = perfil::efetivo_de(s, &p.workspace_id, p.profile.as_deref()).0.contexto.janela_longos;
        let janela_cli = s.rt().ctx.atual.get(&p.id).and_then(|m| m.janela);
        let janela = limite_contexto(janela_perfil, janela_cli);
        let eventos = eventos_do_painel(s, &p.id, &p.workspace_id)?;
        let mut arquivos = vec![file.clone()];
        arquivos.extend(subagent_files(&file));
        for arq in arquivos {
            let chave = arq.to_string_lossy().to_string();
            let Ok(meta) = std::fs::metadata(&arq) else { continue };
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(agora, |d| d.as_millis() as i64);
            let antigo = s.db.waste_state(&chave)?;
            // rotação: o arquivo encolheu, ou o começo que já foi lido mudou (arquivo recriado)
            let reset = match antigo {
                None => false,
                Some(o) => meta.len() < o.len || o.pos > meta.len() || match hash_inicio(&arq, o.pos) {
                    Ok(h) => h != o.head,
                    Err(_) => continue,
                },
            };
            let from = if reset { 0 } else { antigo.map_or(0, |o| o.pos) };
            let escopo = Escopo { janela, mtime, corte: (from == 0).then_some(agora - PRIMEIRA_PASSADA_MS), eventos: eventos.clone() };
            let Ok(pas) = scan_file(&arq, from, escopo) else { continue };
            // em workspace sensível só os contadores ficam guardados, nunca o texto da evidência
            let textos: Vec<Option<String>> = pas.achados.iter().map(|a| if sensivel { Ok(None) } else { serde_json::to_string(&a.evidence).map(Some) }).collect::<Result<_, _>>()?;
            let rows: Vec<WasteIns> = pas
                .achados
                .iter()
                .zip(&textos)
                .map(|(a, ev)| WasteIns { pane: &p.id, workspace: &p.workspace_id, session: Some(sid), kind: a.kind, tokens: a.tokens_est, evidence: ev.as_deref(), at: a.at })
                .collect();
            let Ok(head) = hash_inicio(&arq, pas.pos) else { continue };
            s.db.waste_commit(&chave, reset, &rows, WasteOff { pos: pas.pos, len: pas.len, head })?;
        }
    }
    // arquivos que sumiram do disco não precisam mais de offset
    for f in s.db.waste_files()? {
        if !Path::new(&f).exists() {
            s.db.waste_forget(&f)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn linha_msg(id: &str, ts: &str, model: &str, input: u64, cr: u64, cw: u64, content: &str) -> String {
        format!(r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"cache_read_input_tokens":{cr},"cache_creation_input_tokens":{cw},"output_tokens":5}},"content":[{content}]}}}}"#)
    }

    fn tool(id: &str, name: &str, path: &str) -> String {
        format!(r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{{"file_path":"{path}"}}}}"#)
    }

    fn resultado(id: &str, tamanho: usize, erro: bool) -> String {
        format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":{erro},"content":"{}"}}]}}}}"#, "x".repeat(tamanho))
    }

    fn roda(linhas: &[String]) -> Vec<Finding> {
        let mut an = Analyzer::new(Escopo::simples(200_000));
        for l in linhas {
            an.feed(l);
        }
        an.finish()
    }

    fn ach<'a>(fs: &'a [Finding], kind: &str) -> Option<&'a Finding> {
        fs.iter().find(|f| f.kind == kind)
    }

    #[test]
    fn repeated_read_needs_three_reads_without_edit() {
        let duas: Vec<String> = (0..2).map(|i| linha_msg(&format!("m{i}"), "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, &tool(&format!("t{i}"), "Read", "a.rs"))).collect();
        assert!(ach(&roda(&duas), "repeated_read").is_none());
        let mut tres = duas.clone();
        tres.push(linha_msg("m2", "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, &tool("t2", "Read", "a.rs")));
        assert!(ach(&roda(&tres), "repeated_read").is_some());
        // Edit no meio zera a contagem
        let mut com_edit = duas.clone();
        com_edit.push(linha_msg("e", "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, &tool("te", "Edit", "a.rs")));
        com_edit.push(linha_msg("m2", "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, &tool("t2", "Read", "a.rs")));
        assert!(ach(&roda(&com_edit), "repeated_read").is_none());
    }

    #[test]
    fn huge_output_threshold_is_ten_thousand_tokens() {
        // 40_000 bytes = 10_000 tokens estimados: não passa; 40_004 passa
        assert!(ach(&roda(&[resultado("a", 40_000, false)]), "huge_output").is_none());
        assert!(ach(&roda(&[resultado("a", 40_004, false)]), "huge_output").is_some());
    }

    #[test]
    fn huge_output_counts_text_only_not_images() {
        let imagem = format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"a","is_error":false,"content":[{{"type":"image","source":{{"data":"{}"}}}}]}}]}}}}"#, "A".repeat(60_000));
        assert!(ach(&roda(&[imagem]), "huge_output").is_none(), "imagem não é texto");
        let texto = format!(r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"a","is_error":false,"content":[{{"type":"text","text":"{}"}}]}}]}}}}"#, "x".repeat(40_004));
        assert!(ach(&roda(&[texto]), "huge_output").is_some());
    }

    #[test]
    fn churn_needs_big_write_and_cold_read() {
        let base = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 10, 300_000, 0, "");
        let churn = linha_msg("2", "2026-10-15T10:01:00.000Z", "sonnet", 10, 1_000, 30_000, "");
        let fs = roda(&[base.clone(), churn]);
        let f = ach(&fs, "cache_churn").expect("churn");
        assert_eq!(f.tokens_est, 30_000);
        assert!(f.evidence[0].contains("1 com troca de modelo"));
        // leitura igual a 10% do contexto anterior (300_010) não é churn: o limite é estrito
        let limite = linha_msg("3", "2026-10-15T10:01:00.000Z", "opus", 10, 30_001, 30_000, "");
        assert!(ach(&roda(&[base, limite]), "cache_churn").is_none());
    }

    #[test]
    fn churn_cause_comes_from_recorded_events() {
        let base = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 10, 300_000, 0, "");
        let churn = linha_msg("2", "2026-10-15T10:01:00.000Z", "opus", 10, 1_000, 30_000, "");
        let mut e = Escopo::simples(200_000);
        e.eventos = vec![(parse_ts("2026-10-15T10:00:30.000Z").unwrap(), "compactação")];
        let mut an = Analyzer::new(e);
        [base.clone(), churn.clone()].iter().for_each(|l| an.feed(l));
        let fs = an.finish();
        assert!(ach(&fs, "cache_churn").unwrap().evidence[0].contains("1 após compactação"));
        // evento de 10 minutos antes não explica o churn
        let mut longe = Escopo::simples(200_000);
        longe.eventos = vec![(parse_ts("2026-10-15T09:50:00.000Z").unwrap(), "reinício")];
        let mut an = Analyzer::new(longe);
        [base, churn].iter().for_each(|l| an.feed(l));
        assert!(ach(&an.finish(), "cache_churn").unwrap().evidence[0].contains("1 sem causa identificada"));
    }

    #[test]
    fn cold_gap_needs_gap_over_ttl() {
        let a = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, "");
        let no_limite = linha_msg("2", "2026-10-15T10:05:00.000Z", "opus", 1, 0, 30_000, "");
        assert!(ach(&roda(&[a.clone(), no_limite]), "cold_cache_gap").is_none());
        let depois = linha_msg("2", "2026-10-15T10:05:00.001Z", "opus", 1, 0, 30_000, "");
        assert!(ach(&roda(&[a, depois]), "cold_cache_gap").is_some());
    }

    #[test]
    fn retry_loop_three_identical_or_error_then_same() {
        let call = |id: &str| linha_msg(id, "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, &tool(id, "Bash", "x"));
        let tres: Vec<String> = vec![call("a"), call("b"), call("c")];
        assert!(ach(&roda(&tres), "retry_loop").is_some());
        let erro_depois: Vec<String> = vec![call("a"), resultado("a", 10, true), call("b")];
        assert!(ach(&roda(&erro_depois), "retry_loop").is_some());
        let dois_ok: Vec<String> = vec![call("a"), resultado("a", 10, false), call("b")];
        assert!(ach(&roda(&dois_ok), "retry_loop").is_none());
    }

    #[test]
    fn bloated_context_compares_to_window() {
        let grande = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 150_000, 60_000, 0, "");
        let pequeno = linha_msg("2", "2026-10-15T10:00:00.000Z", "opus", 100_000, 0, 0, "");
        let fs = roda(&[grande, pequeno]);
        let f = ach(&fs, "bloated_context").expect("bloated");
        assert_eq!(f.tokens_est, 10_000);
    }

    #[test]
    fn window_limit_is_eighty_percent_of_the_statusline_window() {
        assert_eq!(limite_contexto(400_000, None), 400_000);
        assert_eq!(limite_contexto(400_000, Some(200_000)), 160_000, "80% de 200k");
        assert_eq!(limite_contexto(400_000, Some(1_000_000)), 400_000, "a janela do perfil ainda vale quando é menor");
    }

    #[test]
    fn findings_take_the_turn_time_and_fall_back_to_mtime() {
        let turno = linha_msg("1", "2026-10-01T10:00:00.000Z", "opus", 300_000, 0, 0, "");
        let sem_ts = linha_msg("2", "2026-10-02T10:00:00.000Z", "opus", 300_000, 0, 0, "").replace(r#""timestamp":"2026-10-02T10:00:00.000Z","#, "");
        let mut e = Escopo::simples(200_000);
        e.mtime = 42;
        let mut an = Analyzer::new(e);
        an.feed(&turno);
        let fs = an.finish();
        assert_eq!(ach(&fs, "bloated_context").unwrap().at, parse_ts("2026-10-01T10:00:00.000Z").unwrap());
        let mut e = Escopo::simples(200_000);
        e.mtime = 42;
        let mut an = Analyzer::new(e);
        an.feed(&sem_ts);
        assert_eq!(ach(&an.finish(), "bloated_context").unwrap().at, 42);
    }

    #[test]
    fn first_pass_skips_turns_older_than_the_cut() {
        let velho = linha_msg("1", "2026-09-01T10:00:00.000Z", "opus", 300_000, 0, 0, &tool("t1", "Read", "a.rs"));
        let novo = linha_msg("2", "2026-10-12T10:00:00.000Z", "opus", 300_000, 0, 0, "");
        let mut e = Escopo::simples(200_000);
        e.corte = parse_ts("2026-10-10T00:00:00.000Z");
        let mut an = Analyzer::new(e);
        [velho, novo].iter().for_each(|l| an.feed(l));
        let fs = an.finish();
        let f = ach(&fs, "bloated_context").expect("só o turno novo conta");
        assert_eq!(f.tokens_est, 100_000);
        assert_eq!(f.at, parse_ts("2026-10-12T10:00:00.000Z").unwrap());
    }

    #[test]
    fn dedupes_message_ids_and_keeps_last_usage() {
        let primeira = linha_msg("m", "2026-10-15T10:00:00.000Z", "opus", 1, 0, 0, "");
        let ultima = linha_msg("m", "2026-10-15T10:00:00.000Z", "opus", 300_000, 0, 0, "");
        let fs = roda(&[primeira, ultima]);
        assert_eq!(ach(&fs, "bloated_context").unwrap().tokens_est, 100_000);
    }

    #[test]
    fn malformed_and_unknown_lines_are_ignored() {
        let fs = roda(&[
            "{truncado".into(),
            r#"{"type":"assistant","message":{"id":"x","usage":{"input_tokens":1}}}"#.into(),
            r#"{"type":"unknown","usage":{}}"#.into(),
            linha_msg("ok", "2026-10-15T10:00:00.000Z", "opus", 300_000, 0, 0, r#"{"type":"thinking","thinking":"x"}"#),
        ]);
        assert_eq!(ach(&fs, "bloated_context").unwrap().tokens_est, 100_000);
    }

    #[test]
    fn parse_ts_matches_epoch() {
        assert_eq!(parse_ts("1970-01-01T00:00:01.250Z"), Some(1250));
        assert_eq!(parse_ts("2026-10-15T00:00:00.000Z").unwrap() % 86_400_000, 0);
        assert_eq!(parse_ts("lixo"), None);
    }

    #[test]
    fn long_lines_are_skipped_and_the_position_still_advances() {
        let mut rd = std::io::Cursor::new(b"aaaaaaaaaaaaaaaa\nok\nsem quebra".to_vec());
        let mut buf = Vec::new();
        assert_eq!(ler_linha(&mut rd, &mut buf, 5).unwrap(), (17, true, true), "passou do teto: consumida e marcada");
        assert_eq!(ler_linha(&mut rd, &mut buf, 5).unwrap(), (3, true, false));
        assert_eq!(buf, b"ok\n");
        assert_eq!(ler_linha(&mut rd, &mut buf, 5).unwrap(), (10, false, true), "sem quebra no fim: fica para depois");
    }

    #[test]
    fn scan_file_counts_ignored_long_lines() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.jsonl");
        let grande = "x".repeat(LINHA_MAX + 10);
        let bloco = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 1_500_000, 0, 0, "");
        let texto = format!("{grande}\n{bloco}\n");
        std::fs::write(&p, &texto).unwrap();
        let pas = scan_file(&p, 0, Escopo::simples(200_000)).unwrap();
        assert_eq!(pas.ignoradas, 1);
        assert_eq!(pas.pos as usize, texto.len(), "a linha longa não trava a leitura");
        assert!(ach(&pas.achados, "bloated_context").is_some());
    }

    #[test]
    fn scan_file_is_incremental_and_skips_partial_line() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.jsonl");
        let l1 = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 300_000, 0, 0, "");
        std::fs::write(&p, format!("{l1}\n{{\"type\":\"assi")).unwrap();
        let p1 = scan_file(&p, 0, Escopo::simples(200_000)).unwrap();
        assert!(ach(&p1.achados, "bloated_context").is_some());
        assert_eq!(p1.pos as usize, l1.len() + 1, "a linha sem quebra fica para depois");
        let p2 = scan_file(&p, p1.pos, Escopo::simples(200_000)).unwrap();
        assert!(p2.achados.is_empty());
    }

    /// Estado com um painel Claude de sessão `sid` cujo JSONL fica numa conta de teste.
    fn estado_com_sessao(d: &Path, sid: &str) -> (Arc<AppState>, std::path::PathBuf) {
        use crate::db::PaneRow;
        let s = AppState::new(d.join("data"), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        crate::util::lk(&s.providers).clis.push(crate::providers::CliDef { id: "claude".into(), label: "Claude".into(), command: "cmd.exe".into(), account_env: Some("CLAUDE_CONFIG_DIR".into()), ..Default::default() });
        let ws = s.db.upsert_workspace(&d.join("proj").to_string_lossy()).unwrap();
        s.db.insert_pane(&PaneRow { id: "p".into(), workspace_id: ws.id.clone(), kind: "agent".into(), name: "p".into(), cli: Some("claude".into()), cwd: "C:/".into(), status: "ocioso".into(), account: Some("teste".to_string()), session_id: Some(sid.into()), ..Default::default() }).unwrap();
        let pasta = d.join("data").join("accounts").join("claude").join("teste").join("projects").join("x");
        std::fs::create_dir_all(&pasta).unwrap();
        (s, pasta.join(format!("{sid}.jsonl")))
    }

    fn total_achados(s: &AppState) -> i64 {
        s.db.waste_rows(0, None).unwrap().iter().map(|r| r.count).sum()
    }

    #[test]
    fn concurrent_refreshes_count_each_finding_once() {
        let d = tempfile::tempdir().unwrap();
        let (s, arq) = estado_com_sessao(d.path(), "sess-1");
        let bloco = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 1_500_000, 0, 0, "");
        std::fs::write(&arq, format!("{bloco}\n")).unwrap();
        let (a, b) = (s.clone(), s.clone());
        let t1 = std::thread::spawn(move || refresh(&a).unwrap());
        let t2 = std::thread::spawn(move || refresh(&b).unwrap());
        t1.join().unwrap();
        t2.join().unwrap();
        assert_eq!(total_achados(&s), 1);
    }

    #[test]
    fn rotated_file_replaces_its_old_findings() {
        let d = tempfile::tempdir().unwrap();
        let (s, arq) = estado_com_sessao(d.path(), "sess-2");
        let antigo = linha_msg("1", "2026-10-15T10:00:00.000Z", "opus", 1_500_000, 0, 0, "");
        std::fs::write(&arq, format!("{antigo}\n")).unwrap();
        refresh(&s).unwrap();
        assert_eq!(total_achados(&s), 1);
        // recriado com outro começo (data diferente nos primeiros 64 bytes): o offset antigo não vale, os achados antigos somem
        let novo = linha_msg("9", "2026-10-14T09:00:00.000Z", "sonnet", 1_500_000, 0, 0, "");
        let outro = linha_msg("8", "2026-10-14T09:00:00.000Z", "sonnet", 1, 0, 0, "");
        std::fs::write(&arq, format!("{outro}\n{novo}\n")).unwrap();
        refresh(&s).unwrap();
        assert_eq!(total_achados(&s), 1, "um achado do arquivo novo, nenhum do antigo");
    }

    #[test]
    fn finding_from_a_removed_pane_still_counts_and_is_marked() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        let f = Finding { kind: "bloated_context", severity: "baixa", tokens_est: 5, evidence: vec![], suggestion_key: "compactacao", at: 1 };
        let ev = Some("[\"x\"]");
        s.db.waste_commit("arq", false, &[WasteIns { pane: "sumiu", workspace: "ws", session: None, kind: f.kind, tokens: f.tokens_est, evidence: ev, at: now_ms() }], WasteOff { pos: 1, len: 1, head: 0 }).unwrap();
        let linhas = s.db.waste_rows(0, Some("ws")).unwrap();
        assert_eq!(linhas[0].count, 1);
        assert_eq!(linhas[0].removidos, 1);
        // workspace sensível: a evidência some, o contador fica
        s.db.waste_clear_evidence("ws").unwrap();
        let depois = s.db.waste_rows(0, Some("ws")).unwrap();
        assert_eq!(depois[0].exemplo, None);
        assert_eq!(depois[0].count, 1);
    }

    /// 50 MB de linhas reais em tamanho: a passada inteira tem que caber em 2 s (build de release).
    #[test]
    #[ignore = "perf: rodar com --release --ignored"]
    fn fifty_megabytes_under_two_seconds() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("grande.jsonl");
        let linha = linha_msg("m", "2026-10-15T10:00:00.000Z", "opus", 1000, 5000, 0, &tool("t", "Read", "src/a.rs"));
        let mut texto = String::new();
        let mut i = 0;
        while texto.len() < 50 * 1024 * 1024 {
            texto.push_str(&linha.replace("\"id\":\"m\"", &format!("\"id\":\"m{i}\"")));
            texto.push('\n');
            i += 1;
        }
        std::fs::write(&p, texto).unwrap();
        let t = std::time::Instant::now();
        let _ = scan_file(&p, 0, Escopo::simples(200_000)).unwrap();
        let ms = t.elapsed().as_millis();
        eprintln!("50 MB: {ms} ms");
        assert!(ms < 2000, "{ms} ms");
    }
}
