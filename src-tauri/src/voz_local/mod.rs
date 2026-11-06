//! Port do helper.rs do supertone-inc/supertonic (MIT). Ver THIRD_PARTY.md.
//! Voz local (Supertonic 3): arquivos fixados, instalação, carga, síntese e a thread que atende os pedidos.
//! Só o WAV sai daqui; a UI toca WAV.

pub mod audio;
pub mod texto;

use crate::error::{fail, AppError, AppResult};
use crate::media::cap_speech;
use crate::runtime::AppState;
use crate::modelos::{baixar, confere, dll_presente, espaco_livre, garantir_dll, hex, iniciar_runtime, pasta, presente, Baixado, Ocupado, Progresso, FOLGA_DISCO, ORT_ZIP};
use crate::util::lk;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use audio::{latente_ruidoso, wav_pcm16, Ruido};
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::State;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use texto::{blocos, ids_e_mascara, preprocessar};

/// Pasta do modelo dentro de `<dados>/models/`.
pub const MODELO: &str = "supertonic-3";
/// Vozes prontas: F = feminina, M = masculina.
pub const VOZES: &[&str] = &["F1", "F2", "F3", "F4", "F5", "M1", "M2", "M3", "M4", "M5"];
/// Pausa entre os blocos de um texto longo.
const PAUSA_S: f32 = 0.3;
/// A thread descarrega o modelo depois de tanto tempo sem uso.
const OCIOSO: Duration = Duration::from_secs(600);
/// Pedidos esperando na thread (acima disso, "ocupada").
const FILA_MAX: usize = 8;

/// Arquivos fixados (tamanho e SHA-256 conferidos em 2026-10-16 no Hugging Face oficial `Supertone/supertonic-3`, revisão `main`).
pub const ARQUIVOS: &[Baixado] = &[
    Baixado { nome: "onnx/duration_predictor.onnx", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/duration_predictor.onnx", sha256: "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db", bytes: 3_700_147 },
    Baixado { nome: "onnx/text_encoder.onnx", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/text_encoder.onnx", sha256: "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff", bytes: 36_416_150 },
    Baixado { nome: "onnx/vector_estimator.onnx", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/vector_estimator.onnx", sha256: "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c", bytes: 256_534_781 },
    Baixado { nome: "onnx/vocoder.onnx", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/vocoder.onnx", sha256: "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba", bytes: 101_424_195 },
    Baixado { nome: "onnx/tts.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/tts.json", sha256: "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09", bytes: 8_253 },
    Baixado { nome: "onnx/unicode_indexer.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/onnx/unicode_indexer.json", sha256: "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f", bytes: 277_676 },
    Baixado { nome: "voice_styles/F1.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F1.json", sha256: "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2", bytes: 292_046 },
    Baixado { nome: "voice_styles/F2.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F2.json", sha256: "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6", bytes: 292_423 },
    Baixado { nome: "voice_styles/F3.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F3.json", sha256: "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab", bytes: 290_794 },
    Baixado { nome: "voice_styles/F4.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F4.json", sha256: "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b", bytes: 291_808 },
    Baixado { nome: "voice_styles/F5.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/F5.json", sha256: "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d", bytes: 291_479 },
    Baixado { nome: "voice_styles/M1.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/M1.json", sha256: "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b", bytes: 291_748 },
    Baixado { nome: "voice_styles/M2.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/M2.json", sha256: "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50", bytes: 292_055 },
    Baixado { nome: "voice_styles/M3.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/M3.json", sha256: "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b", bytes: 290_198 },
    Baixado { nome: "voice_styles/M4.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/M4.json", sha256: "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303", bytes: 291_522 },
    Baixado { nome: "voice_styles/M5.json", url: "https://huggingface.co/Supertone/supertonic-3/resolve/main/voice_styles/M5.json", sha256: "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2", bytes: 291_469 },
];

/// Uma instalação ou remoção da voz por vez.
static INSTALANDO: AtomicBool = AtomicBool::new(false);
/// Cancelamento da instalação em curso: checado a cada pedaço baixado.
static CANCELAR: AtomicBool = AtomicBool::new(false);
/// O modelo carregado, quando há (a thread de voz é a única que carrega e sintetiza).
static MOTOR: Mutex<Option<Motor>> = Mutex::new(None);
/// Espelho de `MOTOR.is_some()` para a tela, sem esperar uma síntese em curso.
static CARREGADA: AtomicBool = AtomicBool::new(false);
/// Canal para a thread `regente-voz`; criado no primeiro pedido.
static FILA: Mutex<Option<SyncSender<Pedido>>> = Mutex::new(None);

#[derive(Deserialize)]
struct Cfg {
    ae: Ae,
    ttl: Ttl,
}

#[derive(Deserialize)]
struct Ae {
    sample_rate: i32,
    base_chunk_size: i32,
}

#[derive(Deserialize)]
struct Ttl {
    chunk_compress_factor: i32,
    latent_dim: i32,
}

#[derive(Deserialize)]
struct Componente {
    data: Vec<Vec<Vec<f32>>>,
    dims: Vec<usize>,
}

#[derive(Deserialize)]
struct EstiloJson {
    style_ttl: Componente,
    style_dp: Componente,
}

/// Os quatro modelos e a configuração, prontos para inferir.
struct Motor {
    taxa: i32,
    base_chunk: i32,
    compress: i32,
    latent_dim: i32,
    indexador: Vec<i64>,
    dp: Session,
    enc: Session,
    est: Session,
    voc: Session,
}

/// Voz escolhida: vetores achatados e a forma `[1, d1, d2]`.
struct Estilo {
    ttl: (Vec<f32>, [usize; 3]),
    dp: (Vec<f32>, [usize; 3]),
}

struct Pedido {
    data: PathBuf,
    texto: String,
    voz: String,
    velocidade: f32,
    passos: u8,
    resposta: SyncSender<AppResult<Vec<u8>>>,
}

// ---------- arquivos

fn pasta_modelo(data: &Path) -> PathBuf {
    pasta(data).join(MODELO)
}

fn erro_ort(e: impl std::fmt::Display) -> AppError {
    AppError::new("ort", e.to_string())
}

/// Lê um arquivo do modelo uma vez e confere o SHA-256 sobre esse mesmo buffer.
fn ler_conferido(data: &Path, nome: &str) -> AppResult<Vec<u8>> {
    let Some(a) = ARQUIVOS.iter().find(|a| a.nome == nome) else { return fail("modelo", format!("{nome} fora da lista fixada")) };
    let bytes = match fs::read(pasta_modelo(data).join(nome)) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return fail("voz_local_ausente", format!("{nome} ausente")),
        Err(e) => return Err(e.into()),
    };
    if !hex(&Sha256::digest(&bytes)).eq_ignore_ascii_case(a.sha256) {
        return fail("hash_invalido", format!("{nome} não bate o SHA-256 fixado"));
    }
    Ok(bytes)
}

fn json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> AppResult<T> {
    serde_json::from_slice(bytes).map_err(|e| AppError::new("modelo", e.to_string()))
}

/// A pasta da voz existe (mesmo incompleta). A DLL do ONNX Runtime é compartilhada: a busca por significado não a apaga enquanto ela existir.
pub fn instalada_parcial(data: &Path) -> bool {
    pasta_modelo(data).is_dir()
}

/// Instalada: a DLL e os arquivos do modelo estão no lugar com o tamanho certo.
pub fn instalada(data: &Path) -> bool {
    dll_presente(data) && ARQUIVOS.iter().all(|a| presente(&pasta_modelo(data).join(a.nome), a.bytes))
}

/// Carregada na memória do processo?
pub fn carregada() -> bool {
    CARREGADA.load(Ordering::SeqCst)
}

/// Baixa a DLL do ONNX Runtime (se faltar) e os arquivos do Supertonic 3. Só roda por ação do usuário.
pub async fn instalar(data: &Path, on: impl Fn(Progresso)) -> AppResult<()> {
    let _o = Ocupado::tomar(&INSTALANDO)?;
    CANCELAR.store(false, Ordering::SeqCst);
    let r = instalar_arquivos(data, &on).await;
    if r.is_err() {
        // cancelado ou falhou antes de qualquer arquivo: a pasta vazia não pode segurar a DLL contra a remoção do embed
        limpar_pastas_vazias(data);
    }
    r
}

/// Tira as pastas da voz que ficaram vazias (de baixo para cima; `remove_dir` não apaga pasta com arquivo).
fn limpar_pastas_vazias(data: &Path) {
    let raiz = pasta_modelo(data);
    for sub in ["onnx", "voice_styles"] {
        let _ = fs::remove_dir(raiz.join(sub));
    }
    let _ = fs::remove_dir(&raiz);
}

async fn instalar_arquivos(data: &Path, on: &impl Fn(Progresso)) -> AppResult<()> {
    fs::create_dir_all(pasta_modelo(data))?;
    if let Some(livre) = espaco_livre(&pasta(data)) {
        let preciso = ARQUIVOS.iter().map(|a| a.bytes).sum::<u64>() + ORT_ZIP.bytes + FOLGA_DISCO;
        if livre < preciso {
            return fail("espaco", format!("Espaço insuficiente no disco: são necessários {} MB livres.", preciso / 1_000_000));
        }
    }
    garantir_dll(data, &CANCELAR, on).await?;
    for a in ARQUIVOS {
        let destino = pasta_modelo(data).join(a.nome);
        if !confere(&destino, a.sha256) {
            if let Some(pai) = destino.parent() {
                fs::create_dir_all(pai)?;
            }
            baixar(a, &destino, &CANCELAR, on).await?;
        }
    }
    on(Progresso::Pronto);
    Ok(())
}

/// Apaga os arquivos da voz e descarrega o modelo. A DLL do ONNX Runtime fica (a busca por significado pode usá-la).
pub fn remover(data: &Path) -> AppResult<()> {
    let _o = Ocupado::tomar(&INSTALANDO)?;
    descarregar();
    match fs::remove_dir_all(pasta_modelo(data)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

// ---------- carga e síntese

/// Carrega o runtime e os quatro modelos, cada um do buffer conferido (nunca pelo caminho).
fn carregar(data: &Path) -> AppResult<Motor> {
    iniciar_runtime(data)?;
    let cfg: Cfg = json(&ler_conferido(data, "onnx/tts.json")?)?;
    let indexador: Vec<i64> = json(&ler_conferido(data, "onnx/unicode_indexer.json")?)?;
    let sessao = |nome: &str| -> AppResult<Session> {
        let bytes = ler_conferido(data, nome)?;
        Session::builder().and_then(|mut b| b.commit_from_memory(&bytes)).map_err(erro_ort)
    };
    Ok(Motor {
        taxa: cfg.ae.sample_rate,
        base_chunk: cfg.ae.base_chunk_size,
        compress: cfg.ttl.chunk_compress_factor,
        latent_dim: cfg.ttl.latent_dim,
        indexador,
        dp: sessao("onnx/duration_predictor.onnx")?,
        enc: sessao("onnx/text_encoder.onnx")?,
        est: sessao("onnx/vector_estimator.onnx")?,
        voc: sessao("onnx/vocoder.onnx")?,
    })
}

fn achatar(c: &Componente) -> AppResult<(Vec<f32>, [usize; 3])> {
    if c.dims.len() != 3 {
        return fail("modelo", "estilo com dimensões inválidas");
    }
    let (d1, d2) = (c.dims[1], c.dims[2]);
    let flat: Vec<f32> = c.data.iter().flatten().flatten().copied().collect();
    if flat.len() != d1 * d2 {
        return fail("modelo", "estilo com tamanho inconsistente");
    }
    Ok((flat, [1, d1, d2]))
}

/// A voz `voz` (já validada) lida do arquivo conferido.
fn estilo(data: &Path, voz: &str) -> AppResult<Estilo> {
    let j: EstiloJson = json(&ler_conferido(data, &format!("voice_styles/{voz}.json"))?)?;
    Ok(Estilo { ttl: achatar(&j.style_ttl)?, dp: achatar(&j.style_dp)? })
}

/// Duração (s) do bloco: finita e entre 0,05 e 60 s. Fora disso o modelo devolveu lixo; não se monta o latente com ele.
fn duracao_segura(d: f32) -> AppResult<f32> {
    if !d.is_finite() {
        return fail("voz_falhou", "duração inválida do modelo");
    }
    Ok(d.clamp(0.05, 60.0))
}

/// Um bloco (já com a tag de idioma): duração, latente com denoising e vocoder. Devolve o WAV do bloco em f32.
fn inferir(m: &mut Motor, texto: &str, est: &Estilo, passos: u8, velocidade: f32) -> AppResult<(Vec<f32>, f32)> {
    let (ids, mascara) = ids_e_mascara(&m.indexador, texto);
    let n = ids.len();

    let duracao = {
        let ids_t = Tensor::from_array(([1usize, n], ids.clone())).map_err(erro_ort)?;
        let mascara_t = Tensor::from_array(([1usize, 1, n], mascara.clone())).map_err(erro_ort)?;
        let dp_t = Tensor::from_array((est.dp.1, est.dp.0.clone())).map_err(erro_ort)?;
        let saida = m.dp.run(ort::inputs! { "text_ids" => &ids_t, "style_dp" => &dp_t, "text_mask" => &mascara_t }).map_err(erro_ort)?;
        let (_, dados) = saida["duration"].try_extract_tensor::<f32>().map_err(erro_ort)?;
        duracao_segura(dados[0] / velocidade)?
    };

    let (emb, forma_emb) = {
        let ids_t = Tensor::from_array(([1usize, n], ids.clone())).map_err(erro_ort)?;
        let mascara_t = Tensor::from_array(([1usize, 1, n], mascara.clone())).map_err(erro_ort)?;
        let ttl_t = Tensor::from_array((est.ttl.1, est.ttl.0.clone())).map_err(erro_ort)?;
        let saida = m.enc.run(ort::inputs! { "text_ids" => &ids_t, "style_ttl" => &ttl_t, "text_mask" => &mascara_t }).map_err(erro_ort)?;
        let (forma, dados) = saida["text_emb"].try_extract_tensor::<f32>().map_err(erro_ort)?;
        (dados.to_vec(), [forma[0] as usize, forma[1] as usize, forma[2] as usize])
    };

    let (mut xt, mascara_lat, forma_lat) = latente_ruidoso(duracao, m.taxa, m.base_chunk, m.compress, m.latent_dim, &mut Ruido::novo());
    for passo in 0..passos {
        let lat_t = Tensor::from_array((forma_lat, std::mem::take(&mut xt))).map_err(erro_ort)?;
        let emb_t = Tensor::from_array((forma_emb, emb.clone())).map_err(erro_ort)?;
        let ttl_t = Tensor::from_array((est.ttl.1, est.ttl.0.clone())).map_err(erro_ort)?;
        let lmask_t = Tensor::from_array(([1usize, 1, forma_lat[2]], mascara_lat.clone())).map_err(erro_ort)?;
        let tmask_t = Tensor::from_array(([1usize, 1, n], mascara.clone())).map_err(erro_ort)?;
        let atual_t = Tensor::from_array(([1usize], vec![passo as f32])).map_err(erro_ort)?;
        let total_t = Tensor::from_array(([1usize], vec![passos as f32])).map_err(erro_ort)?;
        let saida = m
            .est
            .run(ort::inputs! {
                "noisy_latent" => &lat_t,
                "text_emb" => &emb_t,
                "style_ttl" => &ttl_t,
                "latent_mask" => &lmask_t,
                "text_mask" => &tmask_t,
                "current_step" => &atual_t,
                "total_step" => &total_t
            })
            .map_err(erro_ort)?;
        let (_, dados) = saida["denoised_latent"].try_extract_tensor::<f32>().map_err(erro_ort)?;
        xt = dados.to_vec();
    }

    let lat_t = Tensor::from_array((forma_lat, xt)).map_err(erro_ort)?;
    let saida = m.voc.run(ort::inputs! { "latent" => &lat_t }).map_err(erro_ort)?;
    let (_, dados) = saida["wav_tts"].try_extract_tensor::<f32>().map_err(erro_ort)?;
    let mut wav = dados.to_vec();
    wav.truncate((m.taxa as f32 * duracao) as usize);
    Ok((wav, duracao))
}

/// Síntese na thread de voz: carrega o modelo se preciso e lê o texto em blocos, com pausa entre eles.
fn sintetizar_agora(data: &Path, texto: &str, voz: &str, velocidade: f32, passos: u8) -> AppResult<Vec<u8>> {
    let mut g = lk(&MOTOR);
    if g.is_none() {
        *g = Some(carregar(data)?);
        CARREGADA.store(true, Ordering::SeqCst);
    }
    let Some(m) = g.as_mut() else { return fail("voz_local", "modelo não carregado") };
    let est = estilo(data, voz)?;
    let taxa = m.taxa as u32;
    let pausa = vec![0f32; (PAUSA_S * m.taxa as f32) as usize];
    let mut audio = Vec::new();
    for (i, bloco) in blocos(texto, texto::MAX_BLOCO).iter().enumerate() {
        let (wav, _) = inferir(m, &preprocessar(bloco, "pt")?, &est, passos, velocidade)?;
        if i > 0 {
            audio.extend_from_slice(&pausa);
        }
        audio.extend_from_slice(&wav);
    }
    Ok(wav_pcm16(&audio, taxa))
}

fn descarregar() {
    *lk(&MOTOR) = None;
    CARREGADA.store(false, Ordering::SeqCst);
}

/// Thread `regente-voz`: atende os pedidos em ordem e descarrega o modelo depois de `OCIOSO` sem uso.
fn trabalhar(rx: Receiver<Pedido>) {
    let mut ultimo_uso = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(p) => {
                let r = std::panic::catch_unwind(AssertUnwindSafe(|| sintetizar_agora(&p.data, &p.texto, &p.voz, p.velocidade, p.passos)))
                    .unwrap_or_else(|_| Err(AppError::new("voz_local", "a síntese caiu; tente de novo")));
                ultimo_uso = Instant::now();
                let _ = p.resposta.send(r);
            }
            Err(RecvTimeoutError::Timeout) => {
                if carregada() && ultimo_uso.elapsed() > OCIOSO {
                    descarregar();
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Sobe a thread `regente-voz` e devolve o canal de entrada.
fn iniciar_thread() -> AppResult<SyncSender<Pedido>> {
    let (tx, rx) = sync_channel::<Pedido>(FILA_MAX);
    std::thread::Builder::new().name("regente-voz".into()).spawn(move || trabalhar(rx)).map_err(|e| AppError::new("voz_local", e.to_string()))?;
    Ok(tx)
}

/// Envia `p` pelo canal guardado (sobe a thread se não há). Thread morta (receptor caiu): descarta o canal, sobe outra
/// uma vez e reenvia. Fila cheia = "ocupada", sem esperar.
fn despachar(slot: &mut Option<SyncSender<Pedido>>, p: Pedido, nova: impl Fn() -> AppResult<SyncSender<Pedido>>) -> AppResult<()> {
    if slot.is_none() {
        *slot = Some(nova()?);
    }
    let Some(tx) = slot.as_ref() else { return fail("voz_local", "thread da voz indisponível") };
    match tx.try_send(p) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(AppError::new("voz_ocupada", "a voz local está ocupada; tente de novo")),
        Err(TrySendError::Disconnected(p)) => {
            *slot = None;
            let tx = nova()?;
            let r = tx.try_send(p).map_err(|e| match e {
                TrySendError::Full(_) => AppError::new("voz_ocupada", "a voz local está ocupada; tente de novo"),
                TrySendError::Disconnected(_) => AppError::new("voz_local", "a thread da voz parou; tente de novo"),
            });
            *slot = Some(tx);
            r
        }
    }
}

/// Põe o pedido na fila da thread `regente-voz` (ver `despachar`).
fn enfileirar(p: Pedido) -> AppResult<()> {
    despachar(&mut lk(&FILA), p, iniciar_thread)
}

/// Lê `texto` com a voz `voz` (M1–F5), `velocidade` (0,8–1,3) e `passos` (4, 8 ou 16). Devolve um WAV PCM 16-bit.
/// Valida tudo antes de tocar em arquivo; o texto segue o mesmo limite da Mídia.
pub fn sintetizar(data: &Path, texto: &str, voz: &str, velocidade: f32, passos: u8) -> AppResult<Vec<u8>> {
    if !VOZES.contains(&voz) {
        return fail("voz_invalida", format!("voz {voz} desconhecida"));
    }
    if !(0.8..=1.3).contains(&velocidade) {
        return fail("velocidade_invalida", "a velocidade vai de 0,8 a 1,3");
    }
    if !matches!(passos, 4 | 8 | 16) {
        return fail("passos_invalidos", "a qualidade é 4, 8 ou 16 passos");
    }
    let texto = cap_speech(texto);
    if texto.trim().is_empty() {
        return fail("texto_vazio", "nada para ler");
    }
    let (resposta, espera) = sync_channel(1);
    enfileirar(Pedido { data: data.to_path_buf(), texto, voz: voz.into(), velocidade, passos, resposta })?;
    espera.recv().unwrap_or_else(|_| fail("voz_local", "a thread da voz parou; tente de novo"))
}

// ---------- comandos (Tauri)

/// Frase da prova de "Ouvir" nas Configurações.
const FRASE_OUVIR: &str = "Olá! Esta é a voz local do Regente.";

/// Cancela a instalação em curso: o download para no próximo pedaço e o `.part` é apagado pelo erro.
pub fn cancelar() {
    CANCELAR.store(true, Ordering::SeqCst);
}

#[tauri::command]
pub fn voz_local_status(s: State<'_, Arc<AppState>>) -> Value {
    json!({
        "instalada": instalada(&s.data),
        "carregada": carregada(),
        "bytesTotal": ARQUIVOS.iter().map(|a| a.bytes).sum::<u64>(),
        "vozes": VOZES,
    })
}

/// Baixa a voz local.
#[tauri::command(async)]
pub async fn voz_local_instalar(s: State<'_, Arc<AppState>>, on_event: Channel<Progresso>) -> AppResult<()> {
    let data = s.data.clone();
    instalar(&data, move |p| {
        let _ = on_event.send(p);
    })
    .await?;
    Ok(())
}

#[tauri::command]
pub fn voz_local_cancelar() {
    cancelar();
}

/// Apaga os arquivos da voz.
#[tauri::command(async)]
pub fn voz_local_remover(s: State<'_, Arc<AppState>>) -> AppResult<()> {
    remover(&s.data)?;
    Ok(())
}

/// Fala a frase de teste com a voz pedida e as preferências salvas. Devolve o WAV em base64.
#[tauri::command(async)]
pub fn voz_local_ouvir(s: State<'_, Arc<AppState>>, voz: String) -> AppResult<String> {
    let p = lk(&s.settings).voz_local.clone();
    let wav = sintetizar(&s.data, FRASE_OUVIR, &voz, p.velocidade, p.passos)?;
    Ok(B64.encode(wav))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pedido() -> Pedido {
        let (resposta, _) = sync_channel(1);
        Pedido { data: PathBuf::new(), texto: "oi".into(), voz: "F1".into(), velocidade: 1.0, passos: 8, resposta }
    }

    #[test]
    fn duracao_do_modelo_e_limitada() {
        assert_eq!(duracao_segura(f32::NAN).unwrap_err().code(), "voz_falhou");
        assert_eq!(duracao_segura(f32::INFINITY).unwrap_err().code(), "voz_falhou");
        assert_eq!(duracao_segura(-1.0).unwrap(), 0.05);
        assert_eq!(duracao_segura(0.0).unwrap(), 0.05);
        assert_eq!(duracao_segura(1e6).unwrap(), 60.0);
        assert_eq!(duracao_segura(2.5).unwrap(), 2.5);
    }

    #[test]
    fn thread_morta_e_recriada_e_o_pedido_segue() {
        let (morta, rx_morta) = sync_channel::<Pedido>(FILA_MAX);
        drop(rx_morta);
        let mut slot = Some(morta);
        let (viva, rx_viva) = sync_channel::<Pedido>(FILA_MAX);
        let viva = std::cell::RefCell::new(Some(viva));
        despachar(&mut slot, pedido(), || Ok(viva.borrow_mut().take().unwrap())).unwrap();
        assert!(rx_viva.try_recv().is_ok(), "o pedido foi para a thread nova");
        assert!(slot.is_some(), "o canal novo fica guardado");
    }

    fn pasta_vazia() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn voz_invalida_recusada() {
        let d = pasta_vazia();
        assert_eq!(sintetizar(d.path(), "Olá", "X9", 1.0, 8).unwrap_err().code(), "voz_invalida");
        assert!(!d.path().join("models").exists(), "nada foi tocado no disco");
    }

    #[test]
    fn texto_vazio_recusado() {
        let d = pasta_vazia();
        assert_eq!(sintetizar(d.path(), "   ", "F1", 1.0, 8).unwrap_err().code(), "texto_vazio");
    }

    #[test]
    fn nao_instalada_sem_arquivos() {
        assert!(!instalada(pasta_vazia().path()));
    }

    #[test]
    fn instalacao_cancelada_nao_deixa_pasta_vazia_mas_guarda_o_que_baixou() {
        let d = pasta_vazia();
        let raiz = pasta_modelo(d.path());
        fs::create_dir_all(raiz.join("onnx")).unwrap();
        fs::create_dir_all(raiz.join("voice_styles")).unwrap();
        limpar_pastas_vazias(d.path());
        assert!(!raiz.exists(), "pasta vazia sobrou");
        fs::create_dir_all(raiz.join("onnx")).unwrap();
        fs::write(raiz.join("onnx").join("tts.json"), "{}").unwrap();
        limpar_pastas_vazias(d.path());
        assert!(raiz.join("onnx").join("tts.json").exists(), "arquivo baixado foi apagado");
    }

    #[test]
    fn arquivo_com_hash_errado_recusado() {
        let d = pasta_vazia();
        fs::create_dir_all(pasta_modelo(d.path()).join("onnx")).unwrap();
        fs::write(pasta_modelo(d.path()).join("onnx/tts.json"), b"errado").unwrap();
        assert_eq!(ler_conferido(d.path(), "onnx/tts.json").unwrap_err().code(), "hash_invalido");
    }

    #[test]
    #[ignore = "precisa do modelo baixado: REGENTE_VOZ_TESTE_DIR aponta para a pasta de dados com models/"]
    fn sintese_real() {
        let dir = std::env::var("REGENTE_VOZ_TESTE_DIR").expect("defina REGENTE_VOZ_TESTE_DIR");
        let t = Instant::now();
        let wav = sintetizar(Path::new(&dir), "Olá, eu sou o Regente.", "F1", 1.0, 8).unwrap();
        eprintln!("síntese em {:?}", t.elapsed());
        let amostras = (wav.len() - 44) / 2;
        let dur = amostras as f32 / 44_100.0;
        assert!(dur > 0.5, "duração {dur} s");
        assert!(wav[44..].chunks_exact(2).any(|c| (i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0).abs() > 0.01), "silêncio");
    }
}
