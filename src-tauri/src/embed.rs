//! Busca por significado local (M1c, RM-38). Opt-in: nada é baixado até o usuário ligar "Busca por significado" (Memória).
//! O ONNX Runtime (uma DLL) e o modelo e5-small vêm das URLs oficiais, com SHA-256 fixado, para `<data>/models/`.
//! A DLL é aberta travada (FILE_SHARE_READ), conferida pelo hash e carregada por caminho absoluto, com dependências
//! só de System32 e da pasta dela (nunca PATH nem diretório atual). O modelo é lido uma vez: o mesmo buffer que foi
//! conferido é o que vai para o runtime.
//! Sem a busca ligada, sem modelo ou sem vetores, tudo cai na busca por palavra (FTS5), sem erro.
//! Vetores: um por memória, escritos pela thread `regente-embed` (a inserção não espera o modelo).
//! O modelo (~800 MB na memória) sobe só quando há o que vetorizar ou buscar e sai depois de `OCIOSO` sem uso.


use crate::db::{Db, Memory};
use crate::error::{fail, AppError, AppResult};
use crate::modelos::{baixar, confere, dll, dll_presente, espaco_livre, garantir_dll, hex, iniciar_runtime, pasta, presente, Baixado, Ocupado, Progresso, FOLGA_DISCO, ORT_ZIP};
use crate::runtime::AppState;
use crate::util::lk;
use fastembed::{InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::State;

pub const MODELO: &str = "multilingual-e5-small";
pub const DIM: usize = 384;
/// Teto de vetores por workspace (a busca é por força bruta).
pub const CAP: i64 = 20_000;
/// Cosseno a partir do qual uma memória ativa funde na ativa mais antiga do mesmo tipo e workspace (M1.4).
pub const COS_DEDUP: f32 = 0.97;
/// Cosseno abaixo do qual um resultado sem palavra casada é descartado (nunca completa o limite com lixo).
pub const PISO_COSSENO: f32 = 0.80;
/// O modelo sai da memória depois de tanto tempo sem uso (a próxima busca o carrega de novo, ~1 s).
const OCIOSO: Duration = Duration::from_secs(600);
/// Tarefas pendentes na thread de vetores; acima disso a tarefa é descartada (a memória segue na busca por palavra).
const FILA: usize = 256;
/// Memórias sem vetor processadas por lote na reconstrução (os lotes seguem até acabar).
const RECONSTRUIR_MAX: i64 = 2000;

/// Modelo intfloat/multilingual-e5-small (ONNX fp32) do Hugging Face oficial.
pub const ARQUIVOS: &[Baixado] = &[
    Baixado { nome: "model.onnx", url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/model.onnx", sha256: "ca456c06b3a9505ddfd9131408916dd79290368331e7d76bb621f1cba6bc8665", bytes: 470_268_510 },
    Baixado { nome: "tokenizer.json", url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/tokenizer.json", sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39", bytes: 17_082_730 },
    Baixado { nome: "config.json", url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/config.json", sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959", bytes: 655 },
    Baixado { nome: "special_tokens_map.json", url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/special_tokens_map.json", sha256: "d05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7", bytes: 167 },
    Baixado { nome: "tokenizer_config.json", url: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/tokenizer_config.json", sha256: "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b", bytes: 443 },
];

/// Remoção da DLL que falhou (ela estava carregada): o próximo boot termina, se a busca seguir desligada.
const MARCADOR_REMOCAO: &str = "remocao.pendente";

/// Busca ligada (espelho de `settings.busca_semantica.ativo` para o caminho de escrita, que não tem o `AppState`).
static LIGADA: AtomicBool = AtomicBool::new(false);
/// Uma operação de arquivos por vez (instalar, remover, ligar): quem tem o `Ocupado` é o dono até soltar.
static INSTALANDO: AtomicBool = AtomicBool::new(false);
/// Cancelamento da instalação em curso: checado a cada pedaço baixado.
static CANCELAR: AtomicBool = AtomicBool::new(false);
static FILA_TX: OnceLock<SyncSender<Tarefa>> = OnceLock::new();
/// O modelo carregado, quando há.
static MODELO_RT: OnceLock<Mutex<Option<TextEmbedding>>> = OnceLock::new();
/// Pasta de dados, para carregar o modelo sob demanda fora da thread (guardada em `iniciar`).
static DADOS: OnceLock<PathBuf> = OnceLock::new();
/// Uma carga por vez: a busca e a thread podem pedir o modelo juntas (duas cargas = o dobro de memória no pico).
static CARGA: Mutex<()> = Mutex::new(());
/// A carga sob demanda falhou (DLL, VC++, hash): não tenta de novo a cada busca. Ligar a busca zera.
static FALHOU: AtomicBool = AtomicBool::new(false);
/// Último uso do modelo (carga ou vetor); a thread o descarrega depois de `OCIOSO`.
static ULTIMO_USO: Mutex<Option<Instant>> = Mutex::new(None);

/// Preferências (Configurações → Memória). Desligada por padrão; ligar exige a instalação.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EmbedPrefs {
    pub ativo: bool,
    pub modelo: String,
}

impl Default for EmbedPrefs {
    fn default() -> Self {
        Self { ativo: false, modelo: MODELO.into() }
    }
}

/// Trabalho da thread de vetores.
#[derive(Debug, Clone, PartialEq)]
pub enum Tarefa {
    /// Carrega o modelo (fora da thread de comandos) e avisa o resultado pelo evento `busca-semantica`.
    Carregar,
    /// Gera (ou regera) o vetor de uma memória e confere a fusão por cosseno.
    Vetorizar(i64),
    /// Gera os vetores que faltam no workspace (ao ligar a busca).
    Reconstruir(String),
}

// ---------- arquivos e integridade
fn pasta_modelo(data: &Path) -> PathBuf {
    pasta(data).join(MODELO)
}

fn marcador_remocao(data: &Path) -> PathBuf {
    pasta(data).join(MARCADOR_REMOCAO)
}

/// Instalada: a DLL e os arquivos do modelo estão no lugar com o tamanho certo.
pub fn instalada(data: &Path) -> bool {
    dll_presente(data) && ARQUIVOS.iter().all(|a| presente(&pasta_modelo(data).join(a.nome), a.bytes))
}

/// Carregado na memória do processo?
pub fn pronto() -> bool {
    lk(MODELO_RT.get_or_init(|| Mutex::new(None))).is_some()
}

fn descarregar() {
    *lk(MODELO_RT.get_or_init(|| Mutex::new(None))) = None;
}

/// Lê um arquivo do modelo uma vez e confere o hash sobre esse mesmo buffer.
fn ler_conferido(data: &Path, nome: &str) -> AppResult<Vec<u8>> {
    let Some(a) = ARQUIVOS.iter().find(|a| a.nome == nome) else { return fail("modelo", format!("{nome} fora da lista fixada")) };
    let bytes = match fs::read(pasta_modelo(data).join(nome)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return fail("modelo_ausente", format!("{nome} ausente")),
        Err(e) => return Err(e.into()),
    };
    if !hex(&Sha256::digest(&bytes)).eq_ignore_ascii_case(a.sha256) {
        tracing::warn!("busca por significado: {nome} não bate o SHA-256 fixado; não carregado");
        return fail("hash_invalido", format!("{nome} não bate o SHA-256 fixado"));
    }
    Ok(bytes)
}

/// Tamanho que a instalação grava no total (zip, DLL e modelo).
fn tamanho_total_bytes() -> u64 {
    ARQUIVOS.iter().map(|a| a.bytes).sum::<u64>() + ORT_ZIP.bytes
}

/// Baixa e verifica o ONNX Runtime e o modelo. Só roda por ação do usuário ("Ligar busca por significado").
pub async fn instalar(data: &Path, on: impl Fn(Progresso)) -> AppResult<()> {
    fs::create_dir_all(pasta_modelo(data))?;
    if let Some(livre) = espaco_livre(&pasta(data)) {
        let preciso = tamanho_total_bytes() + FOLGA_DISCO;
        if livre < preciso {
            return fail("espaco", format!("Espaço insuficiente no disco: são necessários {} MB livres.", preciso / 1_000_000));
        }
    }
    garantir_dll(data, &CANCELAR, &on).await?;
    for a in ARQUIVOS {
        let destino = pasta_modelo(data).join(a.nome);
        if !confere(&destino, a.sha256) {
            baixar(a, &destino, &CANCELAR, &on).await?;
        }
    }
    on(Progresso::Pronto);
    Ok(())
}

/// Carrega o runtime (DLL travada, conferida, pré-carregada) e o modelo (um buffer por arquivo). Erro = a busca fica na palavra.
/// Sem cache de falha: a próxima ligação tenta de novo.
pub fn carregar(data: &Path) -> AppResult<()> {
    let _carga = lk(&CARGA);
    if pronto() {
        return Ok(());
    }
    iniciar_runtime(data)?;
    let modelo = UserDefinedEmbeddingModel::new(
        ler_conferido(data, "model.onnx")?,
        TokenizerFiles {
            tokenizer_file: ler_conferido(data, "tokenizer.json")?,
            config_file: ler_conferido(data, "config.json")?,
            special_tokens_map_file: ler_conferido(data, "special_tokens_map.json")?,
            tokenizer_config_file: ler_conferido(data, "tokenizer_config.json")?,
        },
    )
    .with_pooling(Pooling::Mean);
    let te = TextEmbedding::try_new_from_user_defined(modelo, InitOptionsUserDefined::new()).map_err(|e| AppError::new("modelo", e.to_string()))?;
    *lk(MODELO_RT.get_or_init(|| Mutex::new(None))) = Some(te);
    *lk(&ULTIMO_USO) = Some(Instant::now());
    Ok(())
}

/// Busca ligada (o modelo pode estar fora da memória: `garantir` o carrega).
pub fn ligada() -> bool {
    LIGADA.load(Ordering::Relaxed)
}

/// O modelo na memória, carregado agora se preciso. False = segue na busca por palavra (desligada ou carga falhou).
fn garantir() -> bool {
    if !ligada() || FALHOU.load(Ordering::Relaxed) {
        return false;
    }
    if pronto() {
        return true;
    }
    let Some(data) = DADOS.get() else { return false };
    match carregar(data) {
        Ok(()) => true,
        Err(e) => {
            FALHOU.store(true, Ordering::Relaxed);
            tracing::info!("busca por significado indisponível: {e}");
            false
        }
    }
}

/// Sem uso há `OCIOSO` ou mais (nunca usado = não está carregado, nada a fazer).
fn ocioso(ultimo: Option<Instant>, agora: Instant) -> bool {
    ultimo.is_some_and(|t| agora.saturating_duration_since(t) >= OCIOSO)
}

/// Vetores dos textos (e5 usa os prefixos "query: " e "passage: " no próprio texto).
pub fn vetorizar(textos: &[String]) -> AppResult<Vec<Vec<f32>>> {
    let mut g = lk(MODELO_RT.get_or_init(|| Mutex::new(None)));
    let Some(m) = g.as_mut() else { return fail("modelo_ausente", "modelo não carregado") };
    *lk(&ULTIMO_USO) = Some(Instant::now());
    m.embed(textos, None).map_err(|e| AppError::new("embed", e.to_string()))
}

// ---------- vetores, ranking e dedup (funções puras onde dá)

/// f32 little-endian, como vai para o BLOB.
pub fn para_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn de_bytes(b: &[u8]) -> Option<Vec<f32>> {
    (b.len() % 4 == 0).then(|| b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap_or([0; 4]))).collect())
}

pub fn cosseno(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let (mut ab, mut aa, mut bb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 {
        0.0
    } else {
        ab / (aa.sqrt() * bb.sqrt())
    }
}

/// Pura: funde a busca por palavra e o cosseno. `bm25_norm` divide pelo maior bm25 da consulta; o cosseno é normalizado
/// (min-max) entre os candidatos. Score = `0,5·bm25_norm + 0,5·cos_norm`. Candidato sem palavra casada e com cosseno
/// bruto abaixo de `PISO_COSSENO` sai da lista (nunca completa o limite com irrelevante).
/// Ordem: nota decrescente, depois id decrescente (determinística).
pub fn fundir(fts: &[(i64, f32)], vetores: &[(i64, Vec<f32>)], q: &[f32]) -> Vec<(i64, f32)> {
    let max = fts.iter().map(|x| x.1).fold(0f32, f32::max);
    // (bm25 normalizado, casou por palavra, cosseno bruto)
    let mut notas: BTreeMap<i64, (f32, bool, f32)> = BTreeMap::new();
    for (id, s) in fts {
        let e = notas.entry(*id).or_insert((0.0, false, 0.0));
        e.0 = if max > 0.0 { s / max } else { 0.0 };
        e.1 = true;
    }
    for (id, v) in vetores {
        notas.entry(*id).or_insert((0.0, false, 0.0)).2 = cosseno(q, v);
    }
    let (minimo, maximo) = notas.values().fold((f32::MAX, f32::MIN), |(lo, hi), e| (lo.min(e.2), hi.max(e.2)));
    let amplitude = maximo - minimo;
    let mut out: Vec<(i64, f32)> = notas
        .into_iter()
        .filter(|(_, (_, casou, cos))| *casou || *cos >= PISO_COSSENO)
        .map(|(id, (b, _, cos))| {
            let cos_norm = if amplitude > 0.0 { (cos - minimo) / amplitude } else if cos > 0.0 { 1.0 } else { 0.0 };
            (id, 0.5 * b + 0.5 * cos_norm)
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    out
}

/// Busca híbrida: `texto` vira vetor de consulta, `fts` (expressão FTS5) dá o bm25. `None` = não dá (sem modelo,
/// sem vetores, ou o modelo falhou): quem chama usa a busca por palavra.
fn hibrida(db: &Db, ws: &str, fts: &str, texto: &str, limite: usize, aceita: &dyn Fn(&Memory) -> bool) -> AppResult<Option<Vec<Memory>>> {
    if !ligada() {
        return Ok(None);
    }
    let vetores: Vec<(i64, Vec<f32>)> = db
        .vectors(ws, None, MODELO)?
        .into_iter()
        .filter(|(_, _, status, _)| status == "ativa")
        .filter_map(|(id, _, _, b)| de_bytes(&b).map(|v| (id, v)))
        .collect();
    if vetores.is_empty() || !garantir() {
        return Ok(None);
    }
    let mut q = match vetorizar(&[format!("query: {texto}")]) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("busca por significado: consulta sem vetor, usando palavra: {e}");
            return Ok(None);
        }
    };
    let Some(qv) = q.pop() else { return Ok(None) };
    let fts_notas = if fts.is_empty() { vec![] } else { db.fts_scores(ws, fts, 200)? };
    let mut out = vec![];
    for (id, _) in fundir(&fts_notas, &vetores, &qv) {
        if out.len() >= limite {
            break;
        }
        if let Some(m) = db.get_memory(id)?.filter(|m| aceita(m)) {
            out.push(m);
        }
    }
    Ok(Some(out))
}

/// Busca por significado (Memória e MCP): híbrida quando dá, senão a busca por palavra de sempre.
pub fn buscar(db: &Db, ws: &str, q: &str, limite: i64) -> AppResult<Vec<Memory>> {
    let fts = fts_expressao(q);
    match hibrida(db, ws, &fts, q, limite.max(0) as usize, &|_| true)? {
        Some(v) => Ok(v),
        None => db.search_memories(ws, q, limite),
    }
}

/// Relevância do brief (M1.2): fato/decisão/risco do escopo da missão, híbrida quando dá; senão `relevant_memories`.
pub fn relevantes(db: &Db, ws: &str, mission_id: Option<&str>, fts_or: &str, texto: &str, limite: usize) -> AppResult<Vec<Memory>> {
    let aceita = |m: &Memory| matches!(m.kind.as_str(), "fato" | "decisao" | "risco") && (m.mission_id.is_none() || m.mission_id.as_deref() == mission_id);
    if let Some(v) = hibrida(db, ws, fts_or, texto, limite, &aceita)? {
        return Ok(v);
    }
    if fts_or.is_empty() {
        return Ok(vec![]);
    }
    db.relevant_memories(ws, mission_id, fts_or, limite as i64)
}

/// Expressão FTS5 de uma busca livre: termos entre aspas, todos (como `search_memories`).
fn fts_expressao(q: &str) -> String {
    q.split_whitespace().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" ")
}

// ---------- escrita: agendar e processar

/// Agenda o vetor de uma memória (inserção e edição não esperam o modelo). Sem busca ligada ou sem thread, não faz nada.
pub fn agendar(t: Tarefa) {
    if !LIGADA.load(Ordering::Relaxed) {
        return;
    }
    if let Some(tx) = FILA_TX.get() {
        if let Err(e) = tx.try_send(t) {
            tracing::warn!("busca por significado: tarefa não agendada: {e}");
        }
    }
}

/// Sobe a thread `regente-embed`. Com a busca ligada e instalada, já carrega o modelo.
/// Com a busca desligada e a remoção pendente (a DLL estava carregada na última vez), termina a remoção.
pub fn iniciar(s: Arc<AppState>) {
    let (tx, rx) = sync_channel::<Tarefa>(FILA);
    if FILA_TX.set(tx).is_err() {
        return;
    }
    let ligada = lk(&s.settings).busca_semantica.ativo;
    LIGADA.store(ligada, Ordering::Relaxed);
    let _ = DADOS.set(s.data.clone());
    // o modelo não sobe no boot: a primeira busca ou vetor o carrega
    let spawn = std::thread::Builder::new().name("regente-embed".into()).spawn(move || {
        if !ligada && marcador_remocao(&s.data).exists() {
            if let Err(e) = apagar_arquivos(&s.data) {
                tracing::warn!("busca por significado: remoção pendente não concluída: {e}");
            }
        }
        loop {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(t) => {
                    if let Err(e) = std::panic::catch_unwind(AssertUnwindSafe(|| processar(&s, t))) {
                        tracing::error!("busca por significado: tarefa caiu: {e:?}");
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if pronto() && ocioso(*lk(&ULTIMO_USO), Instant::now()) {
                        descarregar();
                        tracing::info!("busca por significado: modelo descarregado por ociosidade");
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    if let Err(e) = spawn {
        tracing::error!("busca por significado: thread não iniciou: {e}");
    }
}

fn processar(s: &AppState, t: Tarefa) {
    if !lk(&s.settings).busca_semantica.ativo {
        return;
    }
    let r = match t {
        Tarefa::Carregar => {
            FALHOU.store(false, Ordering::Relaxed);
            let erro = carregar(&s.data).err().map(|e| {
                tracing::info!("busca por significado indisponível: {e}");
                e.to_string()
            });
            s.emit("busca-semantica", json!({"ativo": true, "pronto": pronto(), "erro": erro}));
            return;
        }
        Tarefa::Vetorizar(id) => vetorizar_memoria(s, id),
        Tarefa::Reconstruir(ws) => reconstruir(s, &ws),
    };
    if let Err(e) = r {
        tracing::warn!("busca por significado: {e}");
    }
}

fn vetorizar_memoria(s: &AppState, id: i64) -> AppResult<()> {
    let Some(m) = s.db.get_memory(id)? else { return Ok(()) };
    if m.status == "arquivada" || s.is_sensitive(&m.workspace_id) || !garantir() {
        return Ok(());
    }
    // no teto, só as memórias novas ficam de fora: uma que já tem vetor é regerada
    if s.db.count_vectors(&m.workspace_id)? >= CAP && !s.db.tem_vetor(id)? {
        tracing::warn!("busca por significado: workspace com {CAP} vetores, memória {id} sem vetor");
        return Ok(());
    }
    let Some(v) = vetorizar(&[format!("passage: {}", m.text)])?.into_iter().next() else { return Ok(()) };
    s.db.put_vector(id, MODELO, v.len(), &para_bytes(&v))?;
    fundir_duplicata(s, &m, &v)
}

/// Dedup por cosseno (M1.4, limiar `COS_DEDUP`): só `ativa` contra `ativa`, mesmo tipo e workspace. A mais antiga vira
/// `arquivada` com o motivo "duplicata de #id da nova"; a nova mantém o id (nada é apagado).
fn fundir_duplicata(s: &AppState, m: &Memory, v: &[f32]) -> AppResult<()> {
    if m.status != "ativa" {
        return Ok(());
    }
    let melhor = s
        .db
        .vectors(&m.workspace_id, Some(&m.kind), MODELO)?
        .into_iter()
        .filter(|(oid, _, status, _)| *oid < m.id && status == "ativa")
        .filter_map(|(oid, _, _, b)| de_bytes(&b).map(|ov| (oid, cosseno(v, &ov))))
        .filter(|(_, c)| *c >= COS_DEDUP)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let Some((oid, _)) = melhor else { return Ok(()) };
    s.db.arquivar_duplicata(oid, m.id)
}

/// Gera os vetores que faltam no workspace, em lotes. Keyset por id: um lote que falha não trava a continuação.
/// Sem o modelo carregado, não faz nada (a ligação agenda o carregamento antes).
fn reconstruir(s: &AppState, ws: &str) -> AppResult<()> {
    let mut apos = 0;
    loop {
        let lote = s.db.memories_sem_vetor(ws, apos, RECONSTRUIR_MAX)?;
        let Some(ultimo) = lote.last() else { break };
        // o modelo só sobe quando há o que vetorizar
        if !garantir() {
            break;
        }
        apos = ultimo.id;
        for m in &lote {
            if let Err(e) = vetorizar_memoria(s, m.id) {
                tracing::warn!("busca por significado: memória {} sem vetor: {e}", m.id);
            }
        }
    }
    Ok(())
}

// ---------- comandos

fn tamanho_total_mb() -> u64 {
    tamanho_total_bytes() / 1_000_000
}

#[tauri::command]
pub fn embed_status(s: State<'_, Arc<AppState>>) -> Value {
    let prefs = lk(&s.settings).busca_semantica.clone();
    json!({
        "ativo": prefs.ativo,
        "modelo": MODELO,
        "instalado": instalada(&s.data),
        "pronto": pronto(),
        "tamanhoMb": tamanho_total_mb(),
        "modeloMb": ARQUIVOS.iter().map(|a| a.bytes).sum::<u64>() / 1_000_000,
        "onnxruntimeMb": ORT_ZIP.bytes / 1_000_000,
    })
}

/// Baixa e confere o ONNX Runtime e o modelo. Progresso pelo Channel.
#[tauri::command(async)]
pub async fn embed_install(s: State<'_, Arc<AppState>>, on_event: Channel<Progresso>) -> AppResult<()> {
    let _o = Ocupado::tomar(&INSTALANDO)?;
    CANCELAR.store(false, Ordering::SeqCst);
    let data = s.data.clone();
    instalar(&data, move |p| {
        let _ = on_event.send(p);
    })
    .await
}

/// Cancela a instalação em curso: a flag para o download no próximo pedaço, e o `.part` é apagado pelo erro.
#[tauri::command]
pub fn embed_cancel() {
    CANCELAR.store(true, Ordering::SeqCst);
}

/// Liga ou desliga a busca. Ligar exige a instalação; o carregamento do modelo roda na thread `regente-embed`
/// e avisa o resultado pelo evento `busca-semantica` (uma falha não impede a ligação: a busca fica na palavra).
#[tauri::command(async)]
pub fn embed_set_ativo(s: State<'_, Arc<AppState>>, ativo: bool) -> AppResult<()> {
    let _o = Ocupado::tomar(&INSTALANDO)?;
    if ativo {
        if !instalada(&s.data) {
            return fail("modelo_ausente", "instale a busca por significado antes de ligar");
        }
    } else {
        descarregar();
    }
    {
        let mut cur = lk(&s.settings);
        cur.busca_semantica.ativo = ativo;
        crate::settings::save_json(&s.data.join("settings.json"), &*cur)?;
    }
    LIGADA.store(ativo, Ordering::Relaxed);
    if ativo {
        agendar(Tarefa::Carregar);
        for w in s.db.list_workspaces()? {
            agendar(Tarefa::Reconstruir(w.id));
        }
    }
    Ok(())
}

/// Apaga os arquivos baixados. A DLL pode estar carregada (Windows não apaga): nesse caso fica um marcador e o próximo
/// boot, com a busca desligada, termina a remoção. Os vetores ficam (o mesmo modelo os reaproveita).
fn apagar_arquivos(data: &Path) -> AppResult<()> {
    let apagar = |p: PathBuf| match fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    for a in ARQUIVOS {
        apagar(pasta_modelo(data).join(a.nome))?;
        apagar(pasta_modelo(data).join(format!("{}.part", a.nome)))?;
    }
    apagar(pasta(data).join(format!("{}.part", ORT_ZIP.nome)))?;
    apagar(pasta(data).join(ORT_ZIP.nome))?;
    let marcador = marcador_remocao(data);
    // a DLL do ONNX Runtime é compartilhada com a voz local: enquanto a pasta da voz existir, ela fica
    if crate::voz_local::instalada_parcial(data) {
        tracing::info!("busca por significado: DLL mantida, a voz local usa o ONNX Runtime");
        let _ = apagar(marcador);
    } else {
        match apagar(dll(data)) {
            Ok(()) => {
                let _ = apagar(marcador);
            }
            Err(e) => {
                tracing::warn!("busca por significado: DLL em uso, a remoção termina no próximo boot: {e}");
                let _ = fs::write(&marcador, b"");
            }
        }
    }
    let _ = fs::remove_dir(pasta_modelo(data));
    Ok(())
}

/// Desliga e apaga os arquivos baixados. As configurações só são salvas como desligadas depois da remoção.
#[tauri::command(async)]
pub fn embed_remove(s: State<'_, Arc<AppState>>) -> AppResult<()> {
    let _o = Ocupado::tomar(&INSTALANDO)?;
    descarregar();
    LIGADA.store(false, Ordering::Relaxed);
    apagar_arquivos(&s.data)?;
    {
        let mut cur = lk(&s.settings);
        cur.busca_semantica.ativo = false;
        crate::settings::save_json(&s.data.join("settings.json"), &*cur)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modelos::{carregar_dll, Parcial, ORT_OK};
    #[cfg(windows)]
    use crate::modelos::FLAGS_CARGA_DLL;
    #[cfg(windows)]
    use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32};
    use crate::memory::fts_query;

    #[test]
    fn remocao_da_busca_mantem_a_dll_enquanto_a_voz_local_existe() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path();
        fs::create_dir_all(pasta(data)).unwrap();
        fs::write(dll(data), b"dll").unwrap();
        fs::create_dir_all(pasta(data).join(crate::voz_local::MODELO)).unwrap();
        apagar_arquivos(data).unwrap();
        assert!(dll(data).exists(), "a DLL é do ONNX Runtime e a voz local usa");
        assert!(!marcador_remocao(data).exists(), "sem marcador de remoção para a DLL");
        // sem a pasta da voz, a DLL sai como antes
        fs::remove_dir(pasta(data).join(crate::voz_local::MODELO)).unwrap();
        apagar_arquivos(data).unwrap();
        assert!(!dll(data).exists());
    }

    fn state() -> (tempfile::TempDir, Arc<AppState>) {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        (d, s)
    }

    fn mem(kind: &str, text: &str, at: i64) -> Memory {
        Memory { workspace_id: "w".into(), kind: kind.into(), text: text.into(), created_at: at, ..Default::default() }
    }

    #[test]
    fn hybrid_ranking_mixes_bm25_and_cosine() {
        // id 1: só palavra forte; id 2: palavra fraca e vetor igual à consulta; id 3: só vetor igual
        let fts = [(1, 4.0), (2, 1.0)];
        let vetores = [(2, vec![1.0, 0.0]), (3, vec![1.0, 0.0])];
        let ordem: Vec<i64> = fundir(&fts, &vetores, &[1.0, 0.0]).into_iter().map(|x| x.0).collect();
        assert_eq!(ordem, vec![2, 3, 1], "2 = 0,625; 3 e 1 = 0,5 (empate: id maior primeiro)");
        let so_palavra: Vec<i64> = fundir(&fts, &[], &[1.0, 0.0]).into_iter().map(|x| x.0).collect();
        assert_eq!(so_palavra, vec![1, 2], "sem vetores, a ordem é a da palavra");
    }

    #[test]
    fn relevance_floor_drops_unrelated_vectors_without_word_match() {
        // id 1: cosseno 0,2 e sem palavra (sai); id 2: cosseno 0,9 (fica); id 3: palavra casada, cosseno 0 (fica)
        let fts = [(3, 2.0)];
        let vetores = [(1, vec![0.2, 0.98]), (2, vec![0.9, 0.436])];
        let ids: Vec<i64> = fundir(&fts, &vetores, &[1.0, 0.0]).into_iter().map(|x| x.0).collect();
        assert_eq!(ids.len(), 2, "não completa o resultado com irrelevante");
        assert!(!ids.contains(&1));
        assert!(ids.contains(&2) && ids.contains(&3));
    }

    #[test]
    fn modelo_sai_da_memoria_so_depois_de_ocioso() {
        let agora = Instant::now() + OCIOSO * 2;
        assert!(!ocioso(None, agora), "nunca usado: não há o que descarregar");
        assert!(!ocioso(Some(agora - OCIOSO + Duration::from_secs(1)), agora));
        assert!(ocioso(Some(agora - OCIOSO), agora));
        // relógio para trás (uso registrado "no futuro") não descarrega
        assert!(!ocioso(Some(agora + Duration::from_secs(5)), agora));
    }

    #[test]
    fn search_without_model_falls_back_to_fts_without_errors() {
        let (_d, s) = state();
        s.db.insert_memory(&mem("fato", "o banco é postgres 16", 1)).unwrap();
        s.db.insert_memory(&mem("fato", "usar pnpm", 2)).unwrap();
        assert!(!pronto());
        let por_palavra = buscar(&s.db, "w", "postgres", 10).unwrap();
        assert_eq!(por_palavra.len(), 1);
        assert_eq!(por_palavra[0].text, "o banco é postgres 16");
        // o brief também cai no FTS quando não há modelo
        let rel = relevantes(&s.db, "w", None, &fts_query("postgres"), "postgres", 5).unwrap();
        assert_eq!(rel.len(), 1);
    }

    #[test]
    fn cosine_dedup_archives_the_older_and_keeps_the_newer_id() {
        let (_d, s) = state();
        let antiga = s.db.insert_memory(&mem("fato", "usa pnpm", 1)).unwrap();
        let nova = s.db.insert_memory(&mem("fato", "usa pnpm no projeto inteiro", 2)).unwrap();
        s.db.put_vector(antiga, MODELO, 3, &para_bytes(&[1.0, 0.0, 0.0])).unwrap();
        let v = [0.99, 0.05, 0.0];
        s.db.put_vector(nova, MODELO, 3, &para_bytes(&v)).unwrap();
        let m = s.db.get_memory(nova).unwrap().unwrap();
        fundir_duplicata(&s, &m, &v).unwrap();
        let n = s.db.get_memory(nova).unwrap().unwrap();
        assert_eq!((n.status.as_str(), n.text.as_str()), ("ativa", "usa pnpm no projeto inteiro"), "a nova fica, com o id e o texto dela");
        let a = s.db.get_memory(antiga).unwrap().unwrap();
        assert_eq!(a.status, "arquivada");
        assert_eq!(s.db.status_motivo(antiga).unwrap().as_deref(), Some(format!("duplicata de #{nova}").as_str()));
    }

    #[test]
    fn negated_pair_at_0_95_is_not_merged() {
        // "usa pnpm" e "não usa pnpm" são parecidos no e5, mas 0,95 < 0,97: as duas ficam
        let (_d, s) = state();
        let a = s.db.insert_memory(&mem("fato", "usa pnpm", 1)).unwrap();
        let b = s.db.insert_memory(&mem("fato", "não usa pnpm", 2)).unwrap();
        let va = [1.0, 0.0];
        let vb = [0.95, (1.0f32 - 0.95 * 0.95).sqrt()];
        s.db.put_vector(a, MODELO, 2, &para_bytes(&va)).unwrap();
        s.db.put_vector(b, MODELO, 2, &para_bytes(&vb)).unwrap();
        fundir_duplicata(&s, &s.db.get_memory(b).unwrap().unwrap(), &vb).unwrap();
        assert_eq!(s.db.get_memory(a).unwrap().unwrap().status, "ativa");
        assert_eq!(s.db.get_memory(b).unwrap().unwrap().status, "ativa");
    }

    #[test]
    fn cosine_below_threshold_does_not_merge() {
        let (_d, s) = state();
        let a = s.db.insert_memory(&mem("fato", "usa pnpm", 1)).unwrap();
        let b = s.db.insert_memory(&mem("fato", "deploy na sexta", 2)).unwrap();
        s.db.put_vector(a, MODELO, 2, &para_bytes(&[1.0, 0.0])).unwrap();
        let v = [0.0, 1.0];
        s.db.put_vector(b, MODELO, 2, &para_bytes(&v)).unwrap();
        fundir_duplicata(&s, &s.db.get_memory(b).unwrap().unwrap(), &v).unwrap();
        assert!(s.db.get_memory(b).unwrap().is_some());
        assert_eq!(s.db.get_memory(a).unwrap().unwrap().status, "ativa");
    }

    #[test]
    fn deleting_a_memory_deletes_its_vector_and_a_workspace_its_vectors() {
        let (_d, s) = state();
        let a = s.db.insert_memory(&mem("fato", "usa pnpm", 1)).unwrap();
        s.db.put_vector(a, MODELO, 2, &para_bytes(&[1.0, 0.0])).unwrap();
        s.db.delete_memory(a).unwrap();
        assert_eq!(s.db.count_vectors("w").unwrap(), 0, "o gatilho apaga o vetor junto");
        let b = s.db.insert_memory(&mem("fato", "usa npm", 2)).unwrap();
        s.db.put_vector(b, MODELO, 2, &para_bytes(&[1.0, 0.0])).unwrap();
        s.db.delete_vectors_of_workspace("w").unwrap();
        assert_eq!(s.db.count_vectors("w").unwrap(), 0, "workspace sensível ou removido não guarda vetor");
    }

    #[test]
    fn backfill_pages_by_id_past_the_batch() {
        let (_d, s) = state();
        for i in 0..3 {
            s.db.insert_memory(&mem("fato", &format!("memória {i}"), i)).unwrap();
        }
        let primeiro = s.db.memories_sem_vetor("w", 0, 2).unwrap();
        assert_eq!(primeiro.len(), 2);
        let resto = s.db.memories_sem_vetor("w", primeiro[1].id, 2).unwrap();
        assert_eq!(resto.len(), 1, "o lote seguinte começa depois do último id");
    }

    #[test]
    fn backfill_does_nothing_while_disabled() {
        let (_d, s) = state();
        s.db.insert_memory(&mem("fato", "usa pnpm", 1)).unwrap();
        assert!(!lk(&s.settings).busca_semantica.ativo);
        processar(&s, Tarefa::Reconstruir("w".into()));
        assert_eq!(s.db.count_vectors("w").unwrap(), 0);
    }

    #[test]
    fn model_with_wrong_hash_is_refused_before_any_load() {
        // arquivos falsos no lugar certo: o hash não bate, então nada é carregado (e nada é baixado)
        let (d, _s) = state();
        fs::create_dir_all(pasta_modelo(d.path())).unwrap();
        fs::write(dll(d.path()), b"nao e uma dll").unwrap();
        for a in ARQUIVOS {
            fs::write(pasta_modelo(d.path()).join(a.nome), b"x").unwrap();
        }
        assert!(!instalada(d.path()), "tamanho não confere, então não conta como instalada");
        let e = carregar(d.path()).unwrap_err();
        assert_eq!(e.code(), "hash_invalido", "arquivo presente com conteúdo errado é erro de integridade, não ausência");
        assert!(!pronto());
    }

    #[test]
    fn failed_load_is_not_cached() {
        // sem arquivos: falha; a segunda tentativa também tenta (e falha pelo mesmo motivo, sem travar o estado)
        let d = tempfile::tempdir().unwrap();
        assert_eq!(carregar(d.path()).unwrap_err().code(), "modelo_ausente");
        assert_eq!(carregar(d.path()).unwrap_err().code(), "modelo_ausente");
        assert!(!ORT_OK.load(Ordering::SeqCst));
    }

    #[test]
    fn partial_download_guard_removes_the_part_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("model.onnx.part");
        fs::write(&p, b"parcial").unwrap();
        {
            let _g = Parcial(p.clone());
        }
        assert!(!p.exists());
    }

    #[cfg(windows)]
    #[test]
    fn dll_search_flags_never_include_the_altered_search_path() {
        // LOAD_WITH_ALTERED_SEARCH_PATH (0x8) reabriria o PATH: a carga usa só a pasta da DLL e System32
        assert_eq!(FLAGS_CARGA_DLL, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        assert_eq!(FLAGS_CARGA_DLL & 0x8, 0);
        // a carga real usa essas flags: um arquivo que não é DLL falha com erro do sistema, não com pânico
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("onnxruntime.dll");
        fs::write(&p, b"nao e uma dll").unwrap();
        assert_eq!(carregar_dll(&p).unwrap_err().code(), "ort");
    }

    #[cfg(windows)]
    #[test]
    fn free_space_is_reported_for_an_existing_folder() {
        let d = tempfile::tempdir().unwrap();
        assert!(espaco_livre(d.path()).is_some_and(|b| b > 0));
    }

    #[test]
    fn sha_mismatch_is_detected_by_file_hash() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.bin");
        fs::write(&p, b"abc").unwrap();
        assert!(confere(&p, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"), "sha256(\"abc\")");
        assert!(!confere(&p, &"0".repeat(64)));
    }

    #[test]
    fn f32_blob_roundtrips_and_rejects_odd_lengths() {
        let v = vec![0.5f32, -1.25, 3.0];
        assert_eq!(de_bytes(&para_bytes(&v)), Some(v));
        assert_eq!(de_bytes(&[1, 2, 3]), None);
    }
}
