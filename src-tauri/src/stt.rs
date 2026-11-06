//! Transcrição de voz (ditado no painel e ditado global). Motores: Groq e compatível com OpenAI (HTTP,
//! multipart) e whisper.cpp local (processo `whisper-server` sob demanda + ffmpeg para converter o áudio).
//! Config em `data/voice.json`. A chave da API fica no cofre e nunca é logada nem devolvida.

use crate::error::{fail, AppError, AppResult};
use crate::runtime::AppState;
use crate::settings::save_json;
use crate::util::{cmd, lk, new_id};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Largest reply read from a transcription engine, in bytes.
const REPLY_MAX: usize = 1024 * 1024;
/// Largest audio accepted, in bytes (about 25 minutes of webm/opus).
pub const MAX_AUDIO: usize = 25 * 1024 * 1024;
/// ffmpeg gets this long to convert; past it the process is killed.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct VoiceConfig {
    /// groq | openai | local
    pub engine: String,
    pub base_url: String,
    pub model_chat: String,
    pub model_code: String,
    /// Name of the vault secret with the API key.
    pub secret: Option<String>,
    /// Path to `whisper-server.exe` (local engine).
    pub whisper_server: Option<String>,
    /// Path to the whisper.cpp `.bin` model (local engine).
    pub whisper_model: Option<String>,
    /// Modelo do Whisper local embutido: preciso | leve (o outro é usado se o escolhido não estiver baixado).
    pub whisper_modelo: String,
    /// Minutos sem ditado até soltar a memória do modelo: 0 (logo após cada ditado), 2, 5 ou 15.
    pub whisper_ocioso_min: u32,
    /// auto | pt | en | es
    pub language: String,
    /// codigo | planejamento | conversa
    pub mode: String,
    pub to_english: bool,
    pub terms: Vec<String>,
    pub auto_enter: bool,
    pub hotkey: String,
    pub hold: bool,
    pub global_hotkey: String,
    /// chat | code
    pub global_mode: String,
    /// Dictation into any app (global hotkey). Off until the user turns it on in Settings.
    pub global_enabled: bool,
    /// Hold the hotkey to talk (true) or press to start and again to stop (false).
    pub global_hold: bool,
    /// Price per minute of audio (US$), only to total the voice spend in Uso. 0 = not priced.
    pub usd_per_min: f64,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            engine: "groq".into(),
            base_url: "https://api.groq.com/openai/v1".into(),
            model_chat: "whisper-large-v3-turbo".into(),
            model_code: "whisper-large-v3".into(),
            secret: Some("voice.groq".into()),
            whisper_server: None,
            whisper_model: None,
            // leve por padrão: no notebook de referência (só CPU) o preciso leva 20–35 s por ditado curto, o leve ~5 s
            whisper_modelo: "leve".into(),
            whisper_ocioso_min: 2,
            language: "auto".into(),
            mode: "codigo".into(),
            to_english: false,
            terms: vec![],
            auto_enter: false,
            hotkey: "Ctrl+Space".into(),
            hold: true,
            global_hotkey: "Ctrl+Shift+Space".into(),
            global_mode: "chat".into(),
            global_enabled: false,
            global_hold: true,
            usd_per_min: 0.0,
        }
    }
}

pub struct SttOpts {
    pub mode: String,
    pub language: Option<String>,
    pub to_english: bool,
    pub prompt: String,
}

/// Vocabulary hint for Whisper: a style line for the mode, then the user's terms.
pub fn whisper_prompt(mode: &str, terms: &[String]) -> String {
    let style = match mode {
        "planejamento" => "Ditado de planejamento de software: requisitos, tarefas, decisões e riscos.",
        "conversa" => "Conversa em linguagem natural.",
        _ => "Ditado de programação: nomes de arquivos, comandos, flags, bibliotecas e trechos de código.",
    };
    let terms: Vec<&str> = terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
    if terms.is_empty() {
        style.to_string()
    } else {
        format!("{style} Termos: {}", terms.join(", "))
    }
}

/// The config on disk; written with the defaults when missing. Invalid JSON falls back to the defaults
/// (see `settings::load_or_default`), so a bad file never blocks the app.
pub fn load(data: &Path) -> VoiceConfig {
    let path = data.join("voice.json");
    if !path.exists() {
        let _ = save_json(&path, &VoiceConfig::default());
    }
    let mut c: VoiceConfig = crate::settings::load_or_default(&path);
    // voice.json editado à mão: valor fora da lista volta ao padrão, senão o salvar falharia pela validação
    if !matches!(c.whisper_modelo.as_str(), "preciso" | "leve") {
        c.whisper_modelo = "leve".into();
    }
    if !matches!(c.whisper_ocioso_min, 0 | 2 | 5 | 15) {
        c.whisper_ocioso_min = 2;
    }
    c
}

pub fn save(data: &Path, c: &VoiceConfig) -> AppResult<()> {
    validate(c)?;
    std::fs::create_dir_all(data)?;
    save_json(&data.join("voice.json"), c)
}

/// The endpoint: https, or http only for loopback (the key must not travel in clear text).
fn check_url(raw: &str) -> AppResult<()> {
    let url = reqwest::Url::parse(raw).map_err(|_| AppError::new("bad_voice_url", "URL do motor de voz inválida"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => fail("bad_voice_url", "a URL do motor de voz precisa ser https (http só em localhost)"),
    }
}

fn validate(c: &VoiceConfig) -> AppResult<()> {
    if !matches!(c.engine.as_str(), "groq" | "openai" | "local") {
        return fail("bad_engine", "motor de voz: use groq, openai ou local");
    }
    if !matches!(c.language.as_str(), "auto" | "pt" | "en" | "es") {
        return fail("bad_language", "idioma: auto, pt, en ou es");
    }
    if !matches!(c.mode.as_str(), "codigo" | "planejamento" | "conversa") {
        return fail("bad_mode", "modo: codigo, planejamento ou conversa");
    }
    if !matches!(c.global_mode.as_str(), "chat" | "code") {
        return fail("bad_mode", "ditado global: chat ou code");
    }
    if !matches!(c.whisper_modelo.as_str(), "preciso" | "leve") {
        return fail("bad_whisper_modelo", "modelo do Whisper local: preciso ou leve");
    }
    if !matches!(c.whisper_ocioso_min, 0 | 2 | 5 | 15) {
        return fail("bad_whisper_ocioso", "tempo para liberar a memória: 0, 2, 5 ou 15 minutos");
    }
    if !c.usd_per_min.is_finite() || c.usd_per_min < 0.0 {
        return fail("bad_voice_price", "preço por minuto: use zero ou um valor positivo");
    }
    if !c.base_url.trim().is_empty() {
        check_url(c.base_url.trim())?;
    }
    Ok(())
}

/// One HTTP client for the transcription calls: a long timeout (audio uploads), no redirects.
pub fn client() -> AppResult<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::new("stt_failed", e.to_string()))?;
    Ok(CLIENT.get_or_init(|| c))
}

/// A transcription: the text, and the audio seconds when the engine reports them (Whisper's verbose_json).
#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub seconds: Option<f64>,
}

/// Seconds of audio in a WAV written by `to_wav` (16 kHz, mono, 16-bit: 32000 bytes a second after the 44-byte header).
pub fn wav_seconds(wav_len: usize) -> f64 {
    wav_len.saturating_sub(44) as f64 / 32_000.0
}

/// O whisper-server morre junto com o Regente, até num fechamento forçado (sem `stop_local`): ele entra num Job Object
/// com KILL_ON_JOB_CLOSE, cujo handle só fecha quando o processo do Regente acaba. Sem isso sobravam servidores órfãos
/// com o modelo na memória (~750 MB cada) e os arquivos travados. Falha aqui não impede o ditado.
#[cfg(windows)]
fn amarrar_ao_app(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // o HANDLE é um ponteiro (não é Sync): guardado como número, vive até o fim do processo
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: atributos e nome nulos são válidos; `info` é zerada (estado válido) e só o campo de limites é preenchido.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let tam = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if SetInformationJobObject(h, JobObjectExtendedLimitInformation, (&raw const info).cast(), tam) == 0 {
                return 0;
            }
            h as usize
        }
    });
    // SAFETY: `job` é um handle de job válido (nunca fechado) e o handle do filho vive enquanto `child` existir.
    if job == 0 || unsafe { AssignProcessToJobObject(job as _, child.as_raw_handle() as _) } == 0 {
        tracing::warn!("whisper local: servidor fora do Job Object (pode sobrar se o app fechar à força)");
    }
}

#[cfg(not(windows))]
fn amarrar_ao_app(_child: &Child) {}

/// Idioma pedido ao whisper-server: o configurado, ou "auto" (detecção). Nunca vazio: o padrão do servidor é inglês.
fn idioma_local(configurado: Option<&str>) -> String {
    configurado.map(str::trim).filter(|l| !l.is_empty()).unwrap_or("auto").to_string()
}

// Sem `audio_ctx` reduzido: medido em 2026-10-16, cortar o contexto do Whisper deixa o resultado instável (o modelo
// preciso devolveu "a" ou errou o idioma com 576 e 896 quadros num ditado de 5 s, conforme a execução). A janela cheia
// é a única confiável; quem quer velocidade escolhe o modelo leve.

/// US$ of an audio length at a price per minute (0 when the price is 0).
pub fn audio_cost(seconds: f64, usd_per_min: f64) -> f64 {
    seconds / 60.0 * usd_per_min
}

/// The text of a reply. The body is never included in errors (it may echo the request).
async fn read_text(resp: reqwest::Response) -> AppResult<Transcript> {
    let status = resp.status();
    if !status.is_success() {
        return fail("stt_failed", format!("o motor de voz respondeu HTTP {}", status.as_u16()));
    }
    if resp.content_length().is_some_and(|n| n > REPLY_MAX as u64) {
        return fail("stt_failed", "resposta do motor de voz acima de 1 MB");
    }
    let v: Value = resp.json().await.map_err(|e| AppError::new("stt_failed", format!("resposta inválida: {}", e.without_url())))?;
    let text = v["text"].as_str().map(|t| t.trim().to_string()).ok_or_else(|| AppError::new("stt_failed", "resposta sem texto"))?;
    Ok(Transcript { text, seconds: v["duration"].as_f64().filter(|d| d.is_finite() && *d >= 0.0) })
}

/// groq/openai: multipart POST `{base}/audio/transcriptions` (or `/audio/translations` for English output).
/// Pura: pede `verbose_json` (que traz `duration`) só ao Groq e aos modelos whisper; os outros recebem `json`.
pub fn wants_verbose(engine: &str, model: &str) -> bool {
    engine == "groq" || model.starts_with("whisper")
}

pub async fn transcribe_remote(client: &reqwest::Client, base: &str, key: &str, model: &str, audio: Vec<u8>, mime: &str, engine: &str, o: &SttOpts) -> AppResult<Transcript> {
    let verbose = wants_verbose(engine, model);
    let part = reqwest::multipart::Part::bytes(audio)
        .file_name(if mime.contains("wav") { "audio.wav" } else { "audio.webm" })
        .mime_str(mime)
        .map_err(|_| AppError::new("stt_failed", "tipo de áudio inválido"))?;
    // verbose_json adds `duration` (the audio seconds), which the voice spend in Uso needs; o resto só recebe o texto
    let format = if verbose { "verbose_json" } else { "json" };
    let mut form = reqwest::multipart::Form::new().part("file", part).text("model", model.to_string()).text("response_format", format);
    if !o.prompt.is_empty() {
        form = form.text("prompt", o.prompt.clone());
    }
    // /audio/translations always outputs English and rejects the language hint: only transcriptions get it
    if let Some(lang) = o.language.as_deref().filter(|l| *l != "auto" && !o.to_english) {
        form = form.text("language", lang.to_string());
    }
    let path = if o.to_english { "translations" } else { "transcriptions" };
    let base = base.trim_end_matches('/');
    let resp = client
        .post(format!("{base}/audio/{path}"))
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| AppError::new("stt_failed", format!("falha de rede: {}", e.without_url())))?;
    let t = read_text(resp).await?;
    // sem verbose_json não há duração: o gasto dessa resposta fica sem segundos
    Ok(Transcript { seconds: if verbose { t.seconds } else { None }, ..t })
}

/// Servidor e modelo do motor local: os embutidos (Configurações → Voz → Whisper local) quando baixados e sem caminho
/// manual; senão os caminhos manuais (avançado).
#[derive(Debug, PartialEq)]
enum Fonte {
    /// `modelo` é o id em uso (o escolhido, ou o outro se o escolhido não estiver baixado).
    Embutido { modelo: &'static str },
    Manual { server: String, model: String },
}

fn fonte(data: &Path, cfg: &VoiceConfig) -> AppResult<Fonte> {
    let manual = cfg.whisper_server.as_deref().is_some_and(|p| !p.trim().is_empty());
    if !manual {
        if let Some(modelo) = crate::whisper_local::escolher(data, &cfg.whisper_modelo) {
            return Ok(Fonte::Embutido { modelo });
        }
    }
    let server = cfg
        .whisper_server
        .clone()
        .filter(|p| Path::new(p).is_file())
        .ok_or_else(|| AppError::new("whisper_missing", "Baixe o Whisper local em Configurações → Voz (ou informe o caminho do whisper-server.exe)"))?;
    let model = cfg
        .whisper_model
        .clone()
        .filter(|p| Path::new(p).is_file())
        .ok_or_else(|| AppError::new("whisper_missing", "modelo do whisper.cpp não encontrado: configure o arquivo .bin em Configurações → Voz"))?;
    Ok(Fonte::Manual { server, model })
}

/// Two running whisper-server processes would fight over the model, so there is one, reused while its model matches.
/// Com o embutido, os arquivos ficam travados (FILE_SHARE_READ) enquanto o servidor vive.
struct Local {
    child: Child,
    port: u16,
    /// `"{server}|{model}"`: o servidor só é reaproveitado se ambos baterem.
    chave: String,
    /// Prefixo secreto das rotas (`--request-path`): uma página no navegador não acha o `/inference` nem o `/load`.
    token: String,
    ultimo: Instant,
    /// Prazo sem ditado até o servidor sair (da config no momento do pedido); zero = sair logo após cada ditado.
    ocioso: Duration,
    /// Pedidos em curso (guardas `Uso` vivas): o ticker nunca encerra um servidor que está transcrevendo.
    em_uso: u32,
    /// Identifica este servidor: a guarda de um servidor já trocado não mexe no contador do novo.
    geracao: u64,
    _travas: std::sync::Arc<Vec<std::fs::File>>,
}
static LOCAL: Mutex<Option<Local>> = Mutex::new(None);
static GERACAO: AtomicU64 = AtomicU64::new(1);

/// O prazo de `VoiceConfig.whisper_ocioso_min` (minutos) como duração.
fn prazo(min: u32) -> Duration {
    Duration::from_secs(u64::from(min) * 60)
}

/// Parado há pelo menos `prazo`? Com prazo zero, sempre.
fn servidor_ocioso(ultimo: Instant, agora: Instant, prazo: Duration) -> bool {
    agora.saturating_duration_since(ultimo) >= prazo
}

/// Só sai o servidor sem pedido em curso e parado além do prazo dele.
fn pode_parar(em_uso: u32, ultimo: Instant, agora: Instant, prazo: Duration) -> bool {
    em_uso == 0 && servidor_ocioso(ultimo, agora, prazo)
}

/// Chamado pelo ticker: encerra o servidor local parado além do prazo dele (o modelo segura memória: 0,85 a 1,2 GB).
pub fn parar_se_ocioso() {
    // se há um pedido ou início em curso, pula o ciclo (não trava o ticker)
    let mut g = match LOCAL.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::WouldBlock) => return,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
    };
    if g.as_ref().is_some_and(|l| pode_parar(l.em_uso, l.ultimo, Instant::now(), l.ocioso)) {
        if let Some(mut l) = g.take() {
            let _ = l.child.kill();
            let _ = l.child.wait();
            tracing::info!("whisper local encerrado por ociosidade");
        }
    }
}

/// Pedido em curso no servidor local. Nasce junto com o servidor (ou ao reusá-lo), sob o lock `LOCAL`, e fica viva até o
/// fim de `transcribe_local`. Ao sair de escopo (sucesso, erro, retorno antecipado ou pânico) desconta o pedido: a
/// ociosidade conta a partir daqui e, com prazo zero e nenhum outro pedido, o servidor sai na hora e a memória volta.
struct Uso {
    geracao: u64,
}

impl Uso {
    /// Conta mais um pedido em `l` (chamar com o lock `LOCAL` tomado).
    fn abrir(l: &mut Local) -> Uso {
        l.em_uso += 1;
        l.ultimo = Instant::now();
        Uso { geracao: l.geracao }
    }
}

impl Drop for Uso {
    fn drop(&mut self) {
        let mut g = lk(&LOCAL);
        // servidor já trocado ou ausente: nada a descontar
        let acabou = match g.as_mut().filter(|l| l.geracao == self.geracao) {
            Some(l) => {
                l.em_uso = l.em_uso.saturating_sub(1);
                l.ultimo = Instant::now();
                l.em_uso == 0 && l.ocioso.is_zero()
            }
            None => false,
        };
        if acabou {
            if let Some(mut l) = g.take() {
                let _ = l.child.kill();
                let _ = l.child.wait();
                tracing::info!("whisper local encerrado após o ditado");
            }
        }
    }
}

/// Stops the local server (called at app exit).
pub fn stop_local() {
    if let Some(mut l) = lk(&LOCAL).take() {
        let _ = l.child.kill();
        let _ = l.child.wait();
    }
}

/// Port of a running whisper-server for the chosen source; starts one (on a free loopback port) when needed.
/// Devolve a porta, o token e a guarda do pedido (que já conta como em uso: ver `Uso`).
fn ensure_server(data: &Path, f: &Fonte, ocioso_min: u32) -> AppResult<(u16, String, Uso)> {
    if matches!(f, Fonte::Embutido { .. }) && crate::whisper_local::ocupado() {
        return fail("ocupado", "o Whisper local está sendo instalado ou removido");
    }
    let (server, model) = match f {
        Fonte::Embutido { modelo } => {
            let (s, m) = crate::whisper_local::caminhos(data, modelo)?;
            (s.to_string_lossy().into_owned(), m.to_string_lossy().into_owned())
        }
        Fonte::Manual { server, model } => (server.clone(), model.clone()),
    };
    let chave = format!("{server}|{model}");
    let mut g = lk(&LOCAL);
    if let Some(l) = g.as_mut() {
        if l.chave == chave && matches!(l.child.try_wait(), Ok(None)) {
            l.ocioso = prazo(ocioso_min);
            return Ok((l.port, l.token.clone(), Uso::abrir(l)));
        }
    }
    if let Some(mut old) = g.take() {
        let _ = old.child.kill();
        let _ = old.child.wait();
    }
    // conferir e travar os arquivos (hash do modelo, segundos) só ao iniciar o servidor, não a cada ditado
    let (travas, extra, dir) = match f {
        Fonte::Embutido { modelo } => {
            let travas = crate::whisper_local::travar_sessao(data, modelo)?;
            let n = std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).clamp(1, 8);
            (travas, vec!["-t".to_string(), n.to_string()], Some(crate::whisper_local::pasta_whisper(data)))
        }
        Fonte::Manual { .. } => (std::sync::Arc::new(vec![]), vec![], None),
    };
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let token = new_id();
    let mut c = cmd(&server);
    c.args(["-m", &model, "--host", "127.0.0.1", "--port", &port.to_string(), "--request-path", &format!("/{token}")])
        .args(&extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(d) = &dir {
        c.current_dir(d);
        // o embutido só enxerga o System32 (nada do PATH do usuário entra na busca de DLLs)
        let raiz = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        c.env("PATH", raiz.join("System32"));
    }
    let mut child = c.spawn().map_err(|e| match f {
        Fonte::Embutido { .. } => AppError::new("whisper_failed", format!("o Whisper local não iniciou: {e}. Se faltar o Microsoft Visual C++ Redistributable (x64), instale e tente de novo.")),
        Fonte::Manual { .. } => AppError::new("whisper_missing", format!("não foi possível iniciar o whisper-server: {e}")),
    })?;
    amarrar_ao_app(&child);
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return fail("whisper_failed", "o whisper-server encerrou ao iniciar (modelo inválido?)");
        }
        if t0.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            return fail("whisper_failed", "o whisper-server não respondeu em 60 s");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // a porta atendeu: confirma que quem atendeu é o nosso filho (outro processo pode ter pegado a porta livre)
    let vivo = |c: &mut Child| matches!(c.try_wait(), Ok(None));
    let nosso = vivo(&mut child) && {
        std::thread::sleep(Duration::from_millis(300));
        vivo(&mut child)
    };
    if !nosso {
        return fail("whisper_failed", "outro processo ocupou a porta do Whisper local; tente de novo");
    }
    let geracao = GERACAO.fetch_add(1, Ordering::SeqCst);
    let mut novo = Local { child, port, chave, token: token.clone(), ultimo: Instant::now(), ocioso: prazo(ocioso_min), em_uso: 0, geracao, _travas: travas };
    let uso = Uso::abrir(&mut novo);
    *g = Some(novo);
    Ok((port, token, uso))
}

/// webm (what the UI records) to 16 kHz mono wav, through ffmpeg in the app data folder. ffmpeg gets 60 s.
fn to_wav(data: &Path, audio: Vec<u8>) -> AppResult<Vec<u8>> {
    let dir = data.join("voice-tmp");
    std::fs::create_dir_all(&dir)?;
    let id = new_id();
    let (input, output) = (dir.join(format!("{id}.webm")), dir.join(format!("{id}.wav")));
    std::fs::write(&input, &audio)?;
    let ran = run_ffmpeg(&input, &output);
    let _ = std::fs::remove_file(&input);
    if let Err(e) = ran {
        let _ = std::fs::remove_file(&output);
        return Err(e);
    }
    let bytes = std::fs::read(&output);
    let _ = std::fs::remove_file(&output);
    Ok(bytes?)
}

fn run_ffmpeg(input: &Path, output: &Path) -> AppResult<()> {
    let mut child = cmd("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-ar", "16000", "-ac", "1"])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::new("ffmpeg_missing", format!("ffmpeg não encontrado: {e}")))?;
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(_)) => return fail("ffmpeg_failed", "o ffmpeg não converteu o áudio"),
            Ok(None) if t0.elapsed() > FFMPEG_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return fail("ffmpeg_failed", "o ffmpeg passou de 60 s e foi encerrado");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return fail("ffmpeg_failed", format!("ffmpeg: {e}")),
        }
    }
}

/// O áudio em WAV: o que a UI grava já chega WAV 16 kHz mono (vai direto); outro formato passa pelo ffmpeg.
/// Cabeçalho canônico (fmt logo após RIFF/WAVE, como o frontend grava): PCM, 1 canal, 16000 Hz, 16 bits.
fn wav_16k_mono(b: &[u8]) -> bool {
    let u16le = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32le = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    b.len() >= 44 && &b[0..4] == b"RIFF" && &b[8..16] == b"WAVEfmt " && u16le(20) == 1 && u16le(22) == 1 && u32le(24) == 16000 && u16le(34) == 16
}

fn preparar_wav(data: &Path, audio: Vec<u8>, mime: &str) -> AppResult<Vec<u8>> {
    if mime == "audio/wav" && wav_16k_mono(&audio) {
        return Ok(audio);
    }
    to_wav(data, audio)
}

/// local: starts whisper-server under demand, converts the audio and POSTs to `/inference`.
/// Devolve também o rótulo do gasto, da fonte realmente usada. Ao fim do pedido (com sucesso ou erro) o servidor com
/// prazo zero é encerrado na hora; os demais esperam o prazo no ticker.
pub async fn transcribe_local(s: &AppState, audio: Vec<u8>, mime: &str, o: &SttOpts) -> AppResult<(Transcript, &'static str)> {
    let cfg = load(&s.data);
    let f = fonte(&s.data, &cfg)?;
    let label = if matches!(f, Fonte::Embutido { .. }) { "whisper-local" } else { "whisper.cpp" };
    let ocioso_min = cfg.whisper_ocioso_min;
    let data = s.data.clone();
    let mime = mime.to_string();
    // a guarda `_uso` vive até o fim desta função: sai do escopo em qualquer caminho, inclusive em pânico
    let (port, token, wav, _uso) = tokio::task::spawn_blocking(move || -> AppResult<(u16, String, Vec<u8>, Uso)> {
        let (port, token, uso) = ensure_server(&data, &f, ocioso_min)?;
        let wav = preparar_wav(&data, audio, &mime)?;
        Ok((port, token, wav, uso))
    })
    .await
    .map_err(|e| AppError::new("whisper_failed", e.to_string()))??;

    let seconds = wav_seconds(wav.len());
    let part = reqwest::multipart::Part::bytes(wav).file_name("out.wav").mime_str("audio/wav").expect("valid mime");
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("response_format", "json")
        .text("translate", if o.to_english { "true" } else { "false" });
    if !o.prompt.is_empty() {
        form = form.text("prompt", o.prompt.clone());
    }
    // o idioma vai sempre: sem ele o whisper-server assume "en" (não detecta) e o modelo leve traduzia para inglês
    form = form.text("language", idioma_local(o.language.as_deref()));
    let r = async {
        let resp = client()?
            .post(format!("http://127.0.0.1:{port}/{token}/inference"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| AppError::new("whisper_failed", format!("falha ao falar com o whisper-server: {}", e.without_url())))?;
        // the local engine's length is the converted WAV's, not what the server reports
        AppResult::Ok(Transcript { seconds: Some(seconds), ..read_text(resp).await? })
    }
    .await;
    Ok((r?, label))
}

/// Transcribes with the configured engine. `purpose` (pane | global) only selects the caller and is logged.
pub async fn transcribe(s: &AppState, audio: Vec<u8>, mime: &str, purpose: &str) -> AppResult<String> {
    transcribe_as(s, audio, mime, purpose, None).await
}

/// `to_english` overrides the configured output language for this call (the global code mode asks for the
/// Portuguese text when the IA auxiliar rewrites it, or for Whisper's English when none is configured).
pub async fn transcribe_as(s: &AppState, audio: Vec<u8>, mime: &str, purpose: &str, to_english: Option<bool>) -> AppResult<String> {
    if audio.len() > MAX_AUDIO {
        return fail("audio_too_large", "áudio acima de 25 MB: grave em trechos menores");
    }
    if !matches!(purpose, "pane" | "global") {
        return fail("bad_purpose", "destino da voz desconhecido");
    }
    if audio.is_empty() {
        return fail("empty_audio", "nenhum áudio gravado");
    }
    let cfg = load(&s.data);
    let o = SttOpts { mode: cfg.mode.clone(), language: Some(cfg.language.clone()), to_english: to_english.unwrap_or(cfg.to_english), prompt: whisper_prompt(&cfg.mode, &cfg.terms) };
    tracing::info!("transcrição: motor {}, destino {purpose}, {} bytes", cfg.engine, audio.len());
    let (label, t) = match cfg.engine.as_str() {
        "local" => {
            let (t, label) = transcribe_local(s, audio, mime, &o).await?;
            (label.to_string(), t)
        }
        "groq" | "openai" => {
            let name = cfg.secret.as_deref().ok_or_else(|| AppError::new("missing_secret", "configure a chave do motor de voz no Cofre"))?;
            let key = s.secret(name)?.ok_or_else(|| AppError::new("missing_secret", format!("configure a chave '{name}' no Cofre")))?;
            let base = cfg.base_url.trim();
            if base.is_empty() {
                return fail("bad_voice_url", "informe a URL do motor de voz");
            }
            let model = if cfg.mode == "conversa" { &cfg.model_chat } else { &cfg.model_code };
            (format!("{}:{model}", cfg.engine), transcribe_remote(client()?, base, &key, model, audio, mime, &cfg.engine, &o).await?)
        }
        other => return fail("bad_engine", format!("motor de voz desconhecido: {other}")),
    };
    // the spend is recorded only when the audio length is known; a failed account never loses the text
    // segundos ficam em tok_in (rótulo "min" na Uso), mesmo sem preço; o custo só entra com preço
    if let Some(secs) = t.seconds {
        let _ = s.db.add_usage(&crate::util::today(), "stt", &label, "-", secs.round() as i64, 0, audio_cost(secs, cfg.usd_per_min), 0);
    }
    Ok(t.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use serde_json::json;
    use std::sync::Arc;

    type Seen = Arc<Mutex<Vec<(String, HeaderMap, String)>>>;

    /// Mock of the engine's HTTP API on 127.0.0.1: records each request (path, headers, body) and replies.
    async fn mock(status: StatusCode, reply: Value) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let rec = seen.clone();
        let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: axum::body::Bytes| {
            let rec = rec.clone();
            let reply = reply.clone();
            async move {
                lk(&rec).push((uri.path().to_string(), headers, String::from_utf8_lossy(&body).into_owned()));
                (status, axum::Json(reply))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://127.0.0.1:{port}/openai/v1"), seen)
    }

    fn opts(to_english: bool) -> SttOpts {
        SttOpts { mode: "codigo".into(), language: Some("pt".into()), to_english, prompt: "Termos: Regente".into() }
    }

    #[test]
    fn whisper_prompt_has_mode_style_and_terms() {
        let p = whisper_prompt("planejamento", &["Tauri".into(), " axum ".into(), "".into()]);
        assert!(p.starts_with("Ditado de planejamento"));
        assert!(p.ends_with("Termos: Tauri, axum"));
        assert!(!whisper_prompt("conversa", &[]).contains("Termos"));
    }

    #[test]
    fn audio_seconds_and_cost_for_the_spend() {
        // 1 s of 16 kHz mono 16-bit audio is 32000 bytes after the 44-byte header
        assert_eq!(wav_seconds(44 + 32_000 * 90), 90.0);
        assert_eq!(wav_seconds(10), 0.0, "a header-only file has no audio");
        assert!((audio_cost(90.0, 0.04) - 0.06).abs() < 1e-12);
        assert_eq!(audio_cost(90.0, 0.0), 0.0, "an unpriced engine adds nothing");
    }

    #[test]
    fn negative_or_nan_voice_price_is_refused() {
        let mut c = VoiceConfig::default();
        assert!(validate(&c).is_ok());
        c.usd_per_min = -1.0;
        assert_eq!(validate(&c).unwrap_err().code(), "bad_voice_price");
        c.usd_per_min = f64::NAN;
        assert_eq!(validate(&c).unwrap_err().code(), "bad_voice_price");
    }

    #[test]
    fn config_fills_missing_fields_with_defaults() {
        let c: VoiceConfig = serde_json::from_str(r#"{"engine":"openai","language":"en"}"#).unwrap();
        assert_eq!(c.engine, "openai");
        assert_eq!(c.model_code, "whisper-large-v3");
        assert_eq!(c.secret.as_deref(), Some("voice.groq"));
        assert_eq!(c.hotkey, "Ctrl+Space");
    }

    #[test]
    fn voice_json_antigo_ganha_modelo_e_prazo_padrao() {
        let c: VoiceConfig = serde_json::from_str(r#"{"engine":"local"}"#).unwrap();
        assert_eq!(c.whisper_modelo, "leve");
        assert_eq!(c.whisper_ocioso_min, 2);
        let c: VoiceConfig = serde_json::from_str(r#"{"whisperModelo":"leve","whisperOciosoMin":0}"#).unwrap();
        assert_eq!((c.whisper_modelo.as_str(), c.whisper_ocioso_min), ("leve", 0));
    }

    #[test]
    fn validate_confere_modelo_e_prazo_do_whisper() {
        let mut c = VoiceConfig::default();
        for m in ["preciso", "leve"] {
            c.whisper_modelo = m.into();
            assert!(validate(&c).is_ok(), "{m}");
        }
        c.whisper_modelo = "enorme".into();
        assert_eq!(validate(&c).unwrap_err().code(), "bad_whisper_modelo");
        c.whisper_modelo = "preciso".into();
        for min in [0, 2, 5, 15] {
            c.whisper_ocioso_min = min;
            assert!(validate(&c).is_ok(), "{min}");
        }
        for min in [1, 3, 10, 60] {
            c.whisper_ocioso_min = min;
            assert_eq!(validate(&c).unwrap_err().code(), "bad_whisper_ocioso", "{min}");
        }
    }

    #[test]
    fn load_normaliza_modelo_e_prazo_invalidos_do_voice_json() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("voice.json"), r#"{"whisperModelo":"x","whisperOciosoMin":7}"#).unwrap();
        let c = load(d.path());
        assert_eq!((c.whisper_modelo.as_str(), c.whisper_ocioso_min), ("leve", 2));
        // com os valores corrigidos, o salvar passa
        save(d.path(), &c).unwrap();
    }

    #[test]
    fn save_rejects_bad_values_and_plain_http_remote() {
        let d = tempfile::tempdir().unwrap();
        let mut c = VoiceConfig::default();
        c.engine = "vosk".into();
        assert_eq!(save(d.path(), &c).unwrap_err().code(), "bad_engine");
        c.engine = "groq".into();
        c.base_url = "http://api.exemplo.com/v1".into();
        assert_eq!(save(d.path(), &c).unwrap_err().code(), "bad_voice_url");
        c.base_url = "http://localhost:8080/v1".into();
        save(d.path(), &c).unwrap();
        assert!(d.path().join("voice.json").is_file());
    }

    #[tokio::test]
    async fn remote_posts_multipart_with_bearer_and_reads_text() {
        let (base, seen) = mock(StatusCode::OK, json!({"text": " olá mundo "})).await;
        let text = transcribe_remote(client().unwrap(), &base, "chave-secreta", "whisper-large-v3", b"audio".to_vec(), "audio/webm", "groq", &opts(false)).await.unwrap().text;
        assert_eq!(text, "olá mundo");
        let reqs = lk(&seen);
        let (path, headers, body) = &reqs[0];
        assert_eq!(path, "/openai/v1/audio/transcriptions");
        assert_eq!(headers["authorization"].to_str().unwrap(), "Bearer chave-secreta");
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-large-v3"), "{body}");
        assert!(body.contains("name=\"language\"\r\n\r\npt"), "{body}");
        assert!(body.contains("name=\"response_format\"\r\n\r\nverbose_json"), "{body}");
    }

    #[tokio::test]
    async fn remote_reports_the_audio_seconds_when_the_engine_sends_them() {
        let (base, _) = mock(StatusCode::OK, json!({"text": "oi", "duration": 2.5})).await;
        let t = transcribe_remote(client().unwrap(), &base, "k", "m", b"a".to_vec(), "audio/webm", "groq", &opts(false)).await.unwrap();
        assert_eq!(t, Transcript { text: "oi".into(), seconds: Some(2.5) });
    }

    #[tokio::test]
    async fn translation_goes_to_the_translations_endpoint() {
        let (base, seen) = mock(StatusCode::OK, json!({"text": "hello"})).await;
        let text = transcribe_remote(client().unwrap(), &base, "k", "m", b"a".to_vec(), "audio/webm", "groq", &opts(true)).await.unwrap().text;
        assert_eq!(text, "hello");
        assert_eq!(lk(&seen)[0].0, "/openai/v1/audio/translations");
        // opts() sets language "pt": the translations endpoint must not get it
        assert!(!lk(&seen)[0].2.contains("name=\"language\""));
    }

    #[tokio::test]
    async fn http_error_is_stt_failed_with_status_and_no_key() {
        let (base, _seen) = mock(StatusCode::TOO_MANY_REQUESTS, json!({"error": "limite"})).await;
        let e = transcribe_remote(client().unwrap(), &base, "chave-secreta", "m", b"a".to_vec(), "audio/webm", "groq", &opts(false)).await.unwrap_err();
        assert_eq!(e.code(), "stt_failed");
        let msg = e.to_string();
        assert!(msg.contains("429"), "{msg}");
        assert!(!msg.contains("chave-secreta"));
    }

    #[test]
    fn idioma_do_whisper_local_nunca_fica_no_padrao_ingles_do_servidor() {
        assert_eq!(idioma_local(Some("auto")), "auto");
        assert_eq!(idioma_local(None), "auto");
        assert_eq!(idioma_local(Some(" ")), "auto");
        assert_eq!(idioma_local(Some("pt")), "pt");
    }

    #[test]
    fn wav_da_ui_vai_direto_sem_ffmpeg() {
        let d = tempfile::tempdir().unwrap();
        let wav = wav_hdr(16000);
        assert!(wav_16k_mono(&wav));
        assert_eq!(preparar_wav(d.path(), wav.clone(), "audio/wav").unwrap(), wav);
    }

    fn wav_hdr(hz: u32) -> Vec<u8> {
        let mut b = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0".to_vec();
        b.extend([1, 0, 1, 0]);
        b.extend(hz.to_le_bytes());
        b.extend((hz * 2).to_le_bytes());
        b.extend([2, 0, 16, 0]);
        b.extend(b"data\0\0\0\0");
        b
    }

    #[test]
    fn wav_de_outra_taxa_nao_e_16k_mono() {
        assert!(!wav_16k_mono(&wav_hdr(44100)));
        assert!(!wav_16k_mono(b"RIFF....WAVE"));
    }

    #[test]
    fn fonte_usa_o_modelo_escolhido_cai_no_outro_e_erra_sem_nenhum() {
        let d = tempfile::tempdir().unwrap();
        let mut c = VoiceConfig { whisper_modelo: "preciso".into(), ..VoiceConfig::default() };
        assert_eq!(fonte(d.path(), &c).unwrap_err().code(), "whisper_missing", "nada baixado");
        // só o leve baixado, escolha preciso: cai no leve
        crate::whisper_local::instalar_falso(d.path(), &["leve"]);
        assert_eq!(fonte(d.path(), &c).unwrap(), Fonte::Embutido { modelo: "leve" });
        // os dois baixados: vale o escolhido
        crate::whisper_local::instalar_falso(d.path(), &["preciso", "leve"]);
        assert_eq!(fonte(d.path(), &c).unwrap(), Fonte::Embutido { modelo: "preciso" });
        c.whisper_modelo = "leve".into();
        assert_eq!(fonte(d.path(), &c).unwrap(), Fonte::Embutido { modelo: "leve" });
        // o caminho manual vence
        let exe = d.path().join("meu-server.exe");
        let bin = d.path().join("meu.bin");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::write(&bin, b"x").unwrap();
        c.whisper_server = Some(exe.to_string_lossy().into_owned());
        c.whisper_model = Some(bin.to_string_lossy().into_owned());
        assert!(matches!(fonte(d.path(), &c).unwrap(), Fonte::Manual { .. }), "o caminho manual vence");
    }

    #[test]
    fn prazo_converte_minutos() {
        assert_eq!(prazo(0), Duration::ZERO);
        assert_eq!(prazo(2), Duration::from_secs(120));
        assert_eq!(prazo(15), Duration::from_secs(900));
    }

    #[test]
    fn servidor_sai_conforme_o_prazo_configurado() {
        let t0 = Instant::now();
        // prazo 0: sai já, mesmo sem tempo decorrido
        assert!(servidor_ocioso(t0, t0, prazo(0)));
        // prazo 2 min
        assert!(!servidor_ocioso(t0, t0 + Duration::from_secs(119), prazo(2)));
        assert!(servidor_ocioso(t0, t0 + Duration::from_secs(120), prazo(2)));
        // prazo 5 min
        assert!(!servidor_ocioso(t0, t0 + Duration::from_secs(299), prazo(5)));
        assert!(servidor_ocioso(t0, t0 + Duration::from_secs(300), prazo(5)));
        assert!(!servidor_ocioso(t0 + Duration::from_secs(5), t0, prazo(5)), "relógio para trás não encerra");
    }

    #[test]
    fn ticker_nao_encerra_servidor_com_pedido_em_curso() {
        let t0 = Instant::now();
        let tarde = t0 + Duration::from_secs(3600);
        assert!(!pode_parar(1, t0, tarde, prazo(2)), "transcrevendo há uma hora ainda não é ocioso");
        assert!(!pode_parar(1, t0, tarde, prazo(0)), "nem com prazo zero");
        assert!(pode_parar(0, t0, tarde, prazo(2)));
        assert!(pode_parar(0, t0, t0, prazo(0)));
        assert!(!pode_parar(0, t0, t0 + Duration::from_secs(60), prazo(2)));
    }

    /// Um processo que só dorme, para ocupar o `Local` nos testes (é morto ao fim).
    fn processo_que_dorme() -> Child {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "ping -n 60 127.0.0.1 >NUL"]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg("60");
            c
        };
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
    }

    #[test]
    fn uso_conta_e_solta_a_guarda() {
        let geracao = GERACAO.fetch_add(1, Ordering::SeqCst);
        *lk(&LOCAL) = Some(Local {
            child: processo_que_dorme(),
            port: 0,
            chave: String::new(),
            token: String::new(),
            ultimo: Instant::now(),
            ocioso: prazo(2),
            em_uso: 0,
            geracao,
            _travas: std::sync::Arc::new(vec![]),
        });
        let contar = || lk(&LOCAL).as_ref().map(|l| l.em_uso);
        let abrir = || Uso::abrir(lk(&LOCAL).as_mut().unwrap());
        let um = abrir();
        assert_eq!(contar(), Some(1), "a guarda conta o pedido");
        let dois = abrir();
        assert_eq!(contar(), Some(2));
        drop(um);
        assert_eq!(contar(), Some(1), "soltar uma guarda desconta só ela");
        drop(dois);
        assert_eq!(contar(), Some(0));
        // guarda de um servidor já trocado não desconta o do servidor atual
        drop(Uso { geracao: geracao + 1000 });
        assert_eq!(contar(), Some(0));
        // prazo de 2 min: o servidor fica (o ticker é quem encerra)
        assert!(lk(&LOCAL).is_some());
        stop_local();
        assert!(lk(&LOCAL).is_none());
    }

    #[tokio::test]
    async fn local_without_whisper_server_is_a_clear_error() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        let e = transcribe(&s, b"x".to_vec(), "audio/webm", "pane").await.unwrap_err();
        // the default engine needs a key; switch to local to reach the whisper check
        assert_eq!(e.code(), "missing_secret");
        let mut c = VoiceConfig::default();
        c.engine = "local".into();
        save(d.path(), &c).unwrap();
        let e = transcribe(&s, b"x".to_vec(), "audio/webm", "pane").await.unwrap_err();
        assert_eq!(e.code(), "whisper_missing");
    }

    #[tokio::test]
    async fn oversized_audio_is_refused_before_anything_else() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        // the purpose is also wrong: the size check comes first
        let e = transcribe(&s, vec![0u8; MAX_AUDIO + 1], "audio/webm", "outro").await.unwrap_err();
        assert_eq!(e.code(), "audio_too_large");
    }

    #[tokio::test]
    async fn empty_audio_and_unknown_purpose_are_rejected_before_any_engine() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        assert_eq!(transcribe(&s, vec![], "audio/webm", "pane").await.unwrap_err().code(), "empty_audio");
        assert_eq!(transcribe(&s, b"x".to_vec(), "audio/webm", "outro").await.unwrap_err().code(), "bad_purpose");
    }

    #[tokio::test]
    async fn only_groq_and_whisper_ask_for_verbose_json() {
        assert!(wants_verbose("groq", "llama"));
        assert!(wants_verbose("openai", "whisper-1"));
        assert!(!wants_verbose("openai", "gpt-4o-mini-transcribe"));
        let (base, seen) = mock(StatusCode::OK, json!({"text": "oi", "duration": 2.5})).await;
        let t = transcribe_remote(client().unwrap(), &base, "k", "gpt-4o-mini-transcribe", b"a".to_vec(), "audio/webm", "openai", &opts(false)).await.unwrap();
        assert_eq!(t.seconds, None, "sem verbose não há segundos");
        let first = lk(&seen)[0].2.clone();
        assert!(first.contains("name=\"response_format\"\r\n\r\njson"), "{first}");
        let t = transcribe_remote(client().unwrap(), &base, "k", "whisper-1", b"a".to_vec(), "audio/webm", "openai", &opts(false)).await.unwrap();
        assert_eq!(t.seconds, Some(2.5));
        assert!(lk(&seen)[1].2.contains("name=\"response_format\"\r\n\r\nverbose_json"));
    }

    /// Real call to Groq with the key from the vault (`voice.groq`). Needs network and a key: not run by default.
    #[tokio::test]
    #[ignore = "rede real: usa a chave voice.groq do cofre"]
    async fn real_groq_accepts_a_wav() {
        let s = AppState::new(crate::util::data_dir(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        // 1 s of silence, 16 kHz mono 16-bit PCM
        let mut wav = b"RIFF\x00\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x80\x3e\x00\x00\x00\x7d\x00\x00\x02\x00\x10\x00data\x00\x00\x00\x00".to_vec();
        wav.extend(std::iter::repeat_n(0u8, 32000));
        let o = SttOpts { mode: "codigo".into(), language: Some("pt".into()), to_english: false, prompt: String::new() };
        let key = s.secret("voice.groq").unwrap().expect("chave voice.groq no cofre");
        let _ = transcribe_remote(client().unwrap(), "https://api.groq.com/openai/v1", &key, "whisper-large-v3-turbo", wav, "audio/wav", "groq", &o).await.unwrap();
    }
}
