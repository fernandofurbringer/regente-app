//! Verificação da missão (X1b, A1.4). Roda o comando de testes como processo do Regente, na pasta da missão, e mostra
//! a saída no painel "Verificação · <missão>" (exec, sem processo; um por missão, reaproveitado). Estratégia: testes
//! relacionados primeiro (`testes::relacionados`) e a suíte completa no fim ou sob demanda. Comando vindo do
//! `projeto.json` só roda sozinho com a confiança do hash (`projeto::confiado`). A falha volta ao agente como digest
//! compacto (`testes::compacto`). Rodar leva minutos: nada aqui segura a trava do despacho (planexec).
use crate::error::{fail, AppResult};
use crate::execucoes::{Dados, Fim};
use crate::orchestrator::{bracketed, clean_text};
use crate::planexec;
use crate::providers::{PaneKind, PaneSpec};
use crate::runtime::AppState;
use crate::testes::{compacto, relacionados};
use crate::util::{lk, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::State;

type S<'a> = State<'a, Arc<AppState>>;

/// Limite do digest que volta ao agente (caracteres).
pub const DIGEST_MAX: usize = 2500;
/// Saída guardada por rodada: os últimos bytes (o painel mostra o mesmo teto do anel de execuções).
const TETO_SAIDA: usize = 2 * 1024 * 1024;
const CMD_MAX: usize = 300;
const TIMEOUT_MIN: u32 = 20;
pub const MAX_ITER: u32 = 3;

/// Configuração de verificação da missão (`missions.verify_json`). Ausente = padrões (nada roda sozinho).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct VerifyCfg {
    /// Comando próprio da missão; sem ele, vale o `comandos.testar` do projeto.json.
    pub comando: Option<String>,
    /// Roda sozinha quando um agente da missão termina (Trabalhando -> Ocioso) e a árvore mudou.
    pub rodar_ao_terminar: bool,
    /// Falha vira correção automática: a saída vai ao agente, até `max_iter` vezes.
    pub auto_loop: bool,
    pub max_iter: u32,
    pub timeout_min: u32,
}

impl Default for VerifyCfg {
    fn default() -> Self {
        Self { comando: None, rodar_ao_terminar: false, auto_loop: false, max_iter: MAX_ITER, timeout_min: TIMEOUT_MIN }
    }
}

impl VerifyCfg {
    /// Lê o JSON gravado; o que não lê volta aos padrões.
    pub fn parse(json: Option<&str>) -> Self {
        json.and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
    }

    /// Confere o comando (uma linha, sem controle, até 300 caracteres) e prende os limites.
    pub fn sanear(mut self) -> AppResult<Self> {
        self.comando = match self.comando.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => {
                check_cmd(c)?;
                Some(c.to_string())
            }
            None => None,
        };
        self.max_iter = self.max_iter.clamp(1, 5);
        self.timeout_min = self.timeout_min.clamp(1, 120);
        Ok(self)
    }
}

/// Comando que vai para o `cmd.exe`: uma linha, sem caractere de controle, no limite de tamanho.
pub fn check_cmd(c: &str) -> AppResult<()> {
    if c.chars().count() > CMD_MAX || c.chars().any(char::is_control) {
        return fail("comando_invalido", "o comando de verificação tem até 300 caracteres, numa linha só");
    }
    Ok(())
}

/// Comandos de uma rodada e se algum veio do `projeto.json` (exige confiança para rodar sozinho).
#[derive(Debug, Clone, PartialEq)]
pub struct Rodada {
    pub comandos: Vec<String>,
    pub do_projeto: bool,
}

/// Comandos da rodada. `completa`: a suíte inteira (comando da missão, senão `testar` do projeto, senão o padrão
/// do runner). Senão, testes relacionados aos `mudados` (um comando por runner), e a suíte se não houver relacionados.
pub fn comandos(root: &Path, cfg: &VerifyCfg, projeto: Option<&Value>, mudados: &[String], completa: bool, relacionados_primeiro: bool) -> Option<Rodada> {
    let testar_projeto = projeto.and_then(|p| p.pointer("/comandos/testar")).and_then(Value::as_str).filter(|c| !c.trim().is_empty());
    let testar = cfg.comando.clone().or_else(|| testar_projeto.map(String::from));
    let mut usou_projeto = false;
    let mut comandos: Vec<String> = Vec::new();
    if !completa && relacionados_primeiro {
        let modelos = projeto.and_then(|p| p.pointer("/comandos/relacionados")).and_then(Value::as_object);
        for runner in relacionados::detect_runner(root) {
            if modelos.is_some_and(|m| m.contains_key(runner.chave())) {
                usou_projeto = true;
            }
            for c in relacionados::related_tests(&runner, root, mudados, modelos) {
                if !comandos.contains(&c) {
                    comandos.push(c);
                }
            }
        }
    }
    if comandos.is_empty() {
        if let Some(t) = testar {
            usou_projeto |= cfg.comando.is_none() && testar_projeto.is_some();
            comandos.push(t);
        }
    }
    if comandos.is_empty() {
        return None;
    }
    Some(Rodada { comandos, do_projeto: usou_projeto })
}

/// O projeto está confiado (hash dos comandos confirmado pelo usuário).
pub fn confiado(s: &AppState, ws_id: &str, root: &Path) -> bool {
    let prefs = lk(&s.settings).projeto.clone();
    crate::projeto::confiado(&prefs, ws_id, root)
}

/// Resultado de uma rodada (também é o que a interface recebe).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Resultado {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub digest: String,
    pub comando: String,
    /// Parou por cancelamento (usuário ou arquivamento). Quem decide o próximo passo lê isto, não o digest.
    pub cancelado: bool,
}

/// Última rodada da missão, para o chip e para "Enviar falha ao agente".
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ultimo {
    pub resultado: Resultado,
    /// relacionados | completa | automatica | manual | entrega
    pub modo: String,
    pub em: i64,
}

/// Estado em memória por missão: rodada em curso (e como cancelar), última rodada, iterações do ciclo automático.
#[derive(Default)]
pub struct Estado {
    pub rodando: bool,
    pub cancel: Option<Arc<AtomicBool>>,
    pub ultimo: Option<Ultimo>,
    pub iter: u32,
    /// Hash da árvore na última rodada automática (só roda de novo se a árvore mudou).
    pub hash: Option<String>,
    /// Fim da última rodada automática (debounce de 10 s conta desde aqui).
    pub disparo: i64,
    /// Um disparo automático chegou no debounce ou com a missão ocupada: roda de novo uma vez, ao fim da espera.
    pub pendente: bool,
    /// Painel que recebe a falha (o agente que acabou de terminar).
    pub painel: Option<String>,
}

pub static ESTADO: LazyLock<Mutex<HashMap<String, Estado>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Id do pai virtual das execuções da verificação (não é um painel: a leitura usa esta chave).
pub fn pai_de(missao: &str) -> String {
    format!("verif:{missao}")
}

/// Id da execução única da missão (passa em `execucoes::id_valido`: UUID tem 36 caracteres).
fn exec_id(missao: &str) -> String {
    format!("verif-{missao}")
}

/// Id da execução do pré-aquecimento da missão (painel "Pré-aquecimento"). Fora do estado da verificação.
fn aquec_id(missao: &str) -> String {
    format!("aquec-{missao}")
}

/// Pré-aquecimentos em curso, por missão, com o cancelamento de cada um (fechar o painel cancela).
static AQUECENDO: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Pré-aquecimento (X1.2): roda os comandos de build da worktree numa execução quieta "Pré-aquecimento" da missão,
/// pelo mesmo runner da verificação (cmd endurecido, tempo limite, cancelamento). Não usa a trava de rodadas: não
/// espera nem bloqueia a verificação. Quem chama já checou que o projeto é confiado.
pub fn aquecer(s: Arc<AppState>, missao: &str, cwd: std::path::PathBuf, comandos: Vec<String>, env: Vec<(String, String)>) {
    let comandos: Vec<String> = comandos.into_iter().filter(|c| check_cmd(c).is_ok()).collect();
    if comandos.is_empty() {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    lk(&AQUECENDO).insert(missao.to_string(), cancel.clone());
    let (chave, missao) = (missao.to_string(), missao.to_string());
    let spawned = std::thread::Builder::new().name("regente-pre-aquecimento".into()).spawn(move || {
        let (pai, id) = (format!("aquec:{missao}"), aquec_id(&missao));
        if let Err(e) = aquecer_em_painel(&s, &missao, &pai, &id, &cwd, &comandos, &env, &cancel) {
            tracing::warn!("pré-aquecimento da missão {missao} não rodou: {e}");
        }
        lk(&AQUECENDO).remove(&missao);
    });
    if let Err(e) = spawned {
        lk(&AQUECENDO).remove(&chave);
        tracing::warn!("thread de pré-aquecimento não criada: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
fn aquecer_em_painel(s: &Arc<AppState>, missao: &str, pai: &str, id: &str, cwd: &Path, comandos: &[String], env: &[(String, String)], cancel: &AtomicBool) -> AppResult<()> {
    let m = s.db.get_mission(missao)?;
    s.spawn_pane(PaneSpec {
        workspace_id: m.workspace_id.clone(),
        mission_id: Some(missao.into()),
        kind: PaneKind::Exec,
        name: Some(format!("Pré-aquecimento · {}", m.name)),
        cwd: cwd.to_string_lossy().into_owned(),
        url_or_path: Some(id.into()),
        parent_id: Some(pai.into()),
        ..Default::default()
    })?;
    lk(&s.execs).verificacao_inicio(pai, Dados { id: id.into(), tool: "Pré-aquecimento".into(), command: comandos.join(" && "), description: None, background: false }, "", now_ms());
    let limite = Duration::from_secs(u64::from(VerifyCfg::default().timeout_min.max(1)) * 60);
    let mut ok = true;
    for c in comandos {
        lk(&s.execs).verificacao_texto(pai, id, &format!("\n── {} {} ──\n", chrono::Local::now().format("%H:%M:%S"), c));
        match executar_env(cwd, c, env, limite, cancel, |t| lk(&s.execs).verificacao_texto(pai, id, t)) {
            Ok(saida) if saida.ok => {}
            Ok(_) => {
                ok = false;
                break;
            }
            Err(e) => {
                lk(&s.execs).verificacao_fim(pai, id, Fim::Erro { exit_code: None }, now_ms());
                return Err(e);
            }
        }
    }
    lk(&s.execs).verificacao_fim(pai, id, if ok { Fim::Ok } else { Fim::Erro { exit_code: None } }, now_ms());
    Ok(())
}

/// Garante o painel "Verificação · <missão>" (exec) da missão; reaproveita o que está aberto.
fn garantir_painel(s: &AppState, missao: &str) -> AppResult<()> {
    let id = exec_id(missao);
    if s.db.mission_panes(missao)?.iter().any(|p| p.kind == "exec" && p.closed_at.is_none() && p.url_or_path.as_deref() == Some(id.as_str())) {
        return Ok(());
    }
    let m = s.db.get_mission(missao)?;
    let cwd = planexec::cwd_da_missao(s, missao)?;
    s.spawn_pane(PaneSpec {
        workspace_id: m.workspace_id.clone(),
        mission_id: Some(missao.into()),
        kind: PaneKind::Exec,
        name: Some(format!("Verificação · {}", m.name)),
        cwd: cwd.to_string_lossy().into_owned(),
        url_or_path: Some(id),
        parent_id: Some(pai_de(missao)),
        ..Default::default()
    })?;
    Ok(())
}

/// Saída de um comando: código, texto (os últimos bytes) e o motivo de parada, se não terminou sozinho.
struct Saida {
    ok: bool,
    exit_code: Option<i32>,
    texto: String,
    motivo: Option<&'static str>,
    cancelado: bool,
}

#[cfg(windows)]
fn shell(comando: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new("cmd.exe");
    // /s tira só as aspas de fora: o comando chega como está (é o valor confiado do projeto ou da missão)
    c.raw_arg(format!("/d /s /c \"{comando}\""));
    // sem isto, o cmd procura o programa na pasta atual (um `cargo.exe` plantado no repositório rodaria)
    c.env("NoDefaultCurrentDirectoryInExePath", "1");
    // pré-aquecimento e verificação compilam junto com as panes: mesmo teto (`compilacao`)
    c.envs(crate::compilacao::ambiente().iter().cloned());
    c
}

#[cfg(not(windows))]
fn shell(comando: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(comando);
    c
}

/// Mata a árvore inteira (cmd -> cargo -> rustc...). No Windows, `taskkill /T` leva os filhos junto.
fn matar_arvore(child: &mut std::process::Child) {
    let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let _ = child.kill();
    let _ = child.wait();
}

fn guarda_cauda(texto: &mut String, novo: &str) {
    texto.push_str(novo);
    if texto.len() > TETO_SAIDA {
        let mut corte = texto.len() - TETO_SAIDA;
        while !texto.is_char_boundary(corte) {
            corte += 1;
        }
        texto.drain(..corte);
    }
}

/// Roda um comando na pasta, entrega cada pedaço da saída a `sink` (stdout e stderr juntos) e para no limite de
/// tempo ou no cancelamento, matando a árvore. Erro só se o processo não puder ser criado.
#[cfg(test)]
fn executar(cwd: &Path, comando: &str, limite: Duration, cancel: &AtomicBool, sink: impl FnMut(&str)) -> AppResult<Saida> {
    executar_env(cwd, comando, &[], limite, cancel, sink)
}

/// Como `executar`, com variáveis extras no ambiente do comando (pré-aquecimento: cache de build da worktree).
fn executar_env(cwd: &Path, comando: &str, env: &[(String, String)], limite: Duration, cancel: &AtomicBool, mut sink: impl FnMut(&str)) -> AppResult<Saida> {
    let mut child = shell(comando).envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str()))).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (tx, rx) = mpsc::channel::<String>();
    for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut r = pipe;
            let mut buf = [0u8; 8192];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 || tx.send(String::from_utf8_lossy(&buf[..n]).into_owned()).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    let inicio = Instant::now();
    let mut texto = String::new();
    let mut status = None;
    let mut motivo: Option<&'static str> = None;
    let mut cancelado = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(t) => {
                sink(&t);
                guarda_cauda(&mut texto, &t);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(100)),
        }
        if let Some(st) = child.try_wait()? {
            status = Some(st);
            break;
        }
        if cancel.load(Ordering::SeqCst) {
            motivo = Some("cancelado pelo usuário; processo encerrado");
            cancelado = true;
            matar_arvore(&mut child);
            break;
        }
        if inicio.elapsed() > limite {
            motivo = Some("tempo esgotado; processo encerrado");
            matar_arvore(&mut child);
            break;
        }
    }
    // o que ainda estiver no cano depois que o processo saiu
    while let Ok(t) = rx.recv_timeout(Duration::from_millis(200)) {
        sink(&t);
        guarda_cauda(&mut texto, &t);
    }
    Ok(Saida { ok: motivo.is_none() && status.as_ref().is_some_and(|s| s.success()), exit_code: status.and_then(|s| s.code()), texto, motivo, cancelado })
}

fn cauda(texto: &str, n: usize) -> String {
    let total = texto.chars().count();
    texto.chars().skip(total.saturating_sub(n)).collect()
}

/// Digest para o agente: o resumo do runner reconhecido (testes::compacto), ou o fim da saída.
pub fn digest_de(comando: &str, texto: &str, motivo: Option<&str>) -> String {
    let base = match compacto::detect(comando, texto) {
        Some(r) => compacto::digest(r, texto, DIGEST_MAX).text(),
        None => cauda(texto, DIGEST_MAX),
    };
    match motivo {
        Some(m) => cauda(&format!("{base}\n({m})"), DIGEST_MAX),
        None => base,
    }
}

/// Hash da árvore: `git status --porcelain` e `git diff --stat`, mais os arquivos alterados (para os relacionados).
pub fn mudancas(cwd: &Path) -> (String, Vec<String>) {
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(cwd).args(args).stdin(Stdio::null()).output().map(|o| o.stdout).unwrap_or_default();
    let status = git(&["status", "--porcelain"]);
    // o diff inteiro entra no hash: uma edição que não muda o `--stat` (mesmo tamanho, outra linha) também conta
    let stat = [git(&["diff", "--stat"]), git(&["diff"])].concat();
    (hash_de(&status, &stat), arquivos_de(&status))
}

/// Só os arquivos alterados, por um `git status --porcelain` (sem diff). Para quem precisa da lista e não do hash.
pub fn arquivos_alterados(cwd: &Path) -> Vec<String> {
    let status = Command::new("git").arg("-C").arg(cwd).args(["status", "--porcelain"]).stdin(Stdio::null()).output().map(|o| o.stdout).unwrap_or_default();
    arquivos_de(&status)
}

fn arquivos_de(status: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(status)
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| l[3..].rsplit(" -> ").next().unwrap_or_default().trim().trim_matches('"').to_string())
        .collect()
}

pub fn hash_de(status: &[u8], stat: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(status);
    h.update(stat);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Reserva a rodada da missão (uma por vez). Quem chama segura a rodada até `rodar` soltar.
fn reservar(missao: &str, cancel: &Arc<AtomicBool>) -> AppResult<()> {
    let mut e = lk(&ESTADO);
    let st = e.entry(missao.to_string()).or_default();
    if st.rodando {
        return fail("verificacao_em_curso", "já há uma verificação rodando nesta missão");
    }
    st.rodando = true;
    st.cancel = Some(cancel.clone());
    Ok(())
}

/// Roda os comandos em ordem, na pasta da missão, e grava a saída no painel de verificação. Para no primeiro que
/// falha. Ocupada a missão, recusa (`rodar_esperando` espera).
pub fn rodar(s: &AppState, missao: &str, comandos: &[String], timeout_min: u32, modo: &str) -> AppResult<Resultado> {
    if comandos.is_empty() {
        return fail("sem_comando", "não há comando de verificação para esta missão");
    }
    for c in comandos {
        check_cmd(c)?;
    }
    let cwd = planexec::cwd_da_missao(s, missao)?;
    let cancel = Arc::new(AtomicBool::new(false));
    reservar(missao, &cancel)?;
    s.emit("verificacao-estado", json!({"missionId": missao, "rodando": true}));
    crate::fases::abrir(s, missao, None, None, crate::fases::VERIFICACAO);
    let env = ambiente(s, missao);
    let r = executar_rodada(s, missao, &cwd, comandos, timeout_min, &cancel, &env);
    crate::fases::fechar(s, missao, None, crate::fases::VERIFICACAO);
    let ultimo = r.as_ref().ok().map(|res| Ultimo { resultado: res.clone(), modo: modo.to_string(), em: now_ms() });
    {
        let mut e = lk(&ESTADO);
        let st = e.entry(missao.to_string()).or_default();
        st.rodando = false;
        st.cancel = None;
        if let Some(u) = &ultimo {
            st.ultimo = Some(u.clone());
        }
    }
    s.emit("verificacao-estado", json!({"missionId": missao, "rodando": false, "ultimo": ultimo}));
    r
}

/// Como `rodar`, mas espera a vez quando outra rodada da missão está em curso (verificação de entrega de plano).
pub fn rodar_esperando(s: &AppState, missao: &str, comandos: &[String], timeout_min: u32, modo: &str) -> AppResult<Resultado> {
    let inicio = Instant::now();
    loop {
        // missão arquivando: nenhuma rodada nova (a espera por uma vez também para)
        if crate::planexec::arquivando(missao) {
            return fail("arquivando", "a missão está sendo arquivada; a verificação não roda");
        }
        match rodar(s, missao, comandos, timeout_min, modo) {
            Err(e) if e.code() == "verificacao_em_curso" => {
                if inicio.elapsed() > ESPERA_MAX {
                    return fail("verificacao_espera", "a verificação anterior da missão não terminou a tempo");
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            outro => return outro,
        }
    }
}

/// Espera máxima por uma vez na fila de rodadas da missão (`rodar_esperando`).
// ponytail: espera em laço de 500 ms; uma fila de verdade se o número de entregas simultâneas crescer
const ESPERA_MAX: Duration = Duration::from_secs(30 * 60);

/// Ambiente de cache da missão (o mesmo do pré-aquecimento): a verificação compartilha o target do build da worktree.
fn ambiente(s: &AppState, missao: &str) -> Vec<(String, String)> {
    let Ok(m) = s.db.get_mission(missao) else { return vec![] };
    let Ok(ws) = s.db.get_workspace(&m.workspace_id) else { return vec![] };
    crate::worktree_cache::ambiente_da_missao(&m, &ws)
}

#[allow(clippy::too_many_arguments)]
fn executar_rodada(s: &AppState, missao: &str, cwd: &Path, comandos: &[String], timeout_min: u32, cancel: &AtomicBool, env: &[(String, String)]) -> AppResult<Resultado> {
    garantir_painel(s, missao)?;
    let (pai, id) = (pai_de(missao), exec_id(missao));
    let inicio = Instant::now();
    lk(&s.execs).verificacao_inicio(&pai, Dados { id: id.clone(), tool: "Verificação".into(), command: comandos.join(" && "), description: None, background: false }, "", now_ms());
    let limite = Duration::from_secs(u64::from(timeout_min.max(1)) * 60);
    let (mut ok, mut exit_code, mut comando, mut digest, mut cancelado) = (true, None, String::new(), String::new(), false);
    for c in comandos {
        let cabecalho = format!("\n── {} {} ──\n", chrono::Local::now().format("%H:%M:%S"), c);
        lk(&s.execs).verificacao_texto(&pai, &id, &cabecalho);
        let saida = match executar_env(cwd, c, env, limite, cancel, |t| lk(&s.execs).verificacao_texto(&pai, &id, t)) {
            Ok(x) => x,
            Err(e) => {
                lk(&s.execs).verificacao_fim(&pai, &id, Fim::Erro { exit_code: None }, now_ms());
                return Err(e);
            }
        };
        digest = digest_de(c, &saida.texto, saida.motivo);
        comando = c.clone();
        exit_code = saida.exit_code;
        if !saida.ok {
            ok = false;
            cancelado = saida.cancelado;
            break;
        }
    }
    lk(&s.execs).verificacao_fim(&pai, &id, if ok { Fim::Ok } else { Fim::Erro { exit_code } }, now_ms());
    Ok(Resultado { ok, exit_code, duration_ms: inicio.elapsed().as_millis() as u64, digest, comando, cancelado })
}

/// Cancela a rodada em curso da missão (mata a árvore de processos). Falso: nada rodando.
pub fn cancelar(missao: &str) -> bool {
    match lk(&ESTADO).get(missao).and_then(|e| e.cancel.clone()) {
        Some(c) => {
            c.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

/// Cancela todas as rodadas em curso (saída do app), inclusive os pré-aquecimentos.
pub fn cancelar_todas() -> usize {
    let aquecendo = lk(&AQUECENDO).values().map(|c| c.store(true, Ordering::SeqCst)).count();
    lk(&ESTADO).values().filter_map(|e| e.cancel.clone()).map(|c| c.store(true, Ordering::SeqCst)).count() + aquecendo
}

/// Fechar o painel "Verificação" da missão cancela a rodada dela; fechar o "Pré-aquecimento" cancela o build.
pub fn ao_fechar_painel(row: &crate::db::PaneRow) {
    if let (Some(m), Some(id)) = (row.mission_id.as_deref(), row.url_or_path.as_deref()) {
        if row.kind == "exec" && id == aquec_id(m) {
            if let Some(c) = lk(&AQUECENDO).get(m) {
                c.store(true, Ordering::SeqCst);
            }
        }
        if row.kind == "exec" && id == exec_id(m) {
            cancelar(m);
        }
    }
}

/// Visão da missão para a interface: a configuração, o comando do projeto, se o projeto é confiado, e a última rodada.
pub fn visao(s: &AppState, missao: &str) -> AppResult<Value> {
    let m = s.db.get_mission(missao)?;
    let cfg = VerifyCfg::parse(m.verify_json.as_deref());
    let ws = s.db.get_workspace(&m.workspace_id)?;
    let root = Path::new(&ws.path);
    let projeto = crate::projeto::load(root).ok().flatten();
    let comando_projeto = projeto.as_ref().and_then(|p| p.pointer("/comandos/testar")).and_then(Value::as_str).map(String::from);
    let id = exec_id(missao);
    let painel = s.db.mission_panes(missao)?.into_iter().find(|p| p.kind == "exec" && p.closed_at.is_none() && p.url_or_path.as_deref() == Some(id.as_str())).map(|p| p.id);
    let e = lk(&ESTADO);
    let st = e.get(missao);
    Ok(json!({
        "workspaceId": m.workspace_id,
        "painel": painel,
        "cfg": cfg,
        "comandoProjeto": comando_projeto,
        "confiado": confiado(s, &m.workspace_id, root),
        "rodando": st.is_some_and(|x| x.rodando),
        "ultimo": st.and_then(|x| x.ultimo.clone()),
    }))
}

#[tauri::command]
pub fn mission_verify_get(s: S<'_>, id: String) -> AppResult<Value> {
    visao(&s, &id)
}

#[tauri::command]
pub fn mission_verify_set(s: S<'_>, id: String, value: VerifyCfg) -> AppResult<Value> {
    let cfg = value.sanear()?;
    s.db.get_mission(&id)?;
    s.db.set_mission_verify(&id, &serde_json::to_string(&cfg)?)?;
    visao(&s, &id)
}

/// "Rodar verificação" (manual): a suíte completa. Comando do projeto sem confiança: recusa, para a tela pedir.
#[tauri::command(async)]
pub fn mission_verify_run(s: S<'_>, id: String) -> AppResult<Resultado> {
    let m = s.db.get_mission(&id)?;
    let cfg = VerifyCfg::parse(m.verify_json.as_deref());
    let ws = s.db.get_workspace(&m.workspace_id)?;
    let root = Path::new(&ws.path);
    let projeto = crate::projeto::load(root).ok().flatten();
    let cwd = planexec::cwd_da_missao(&s, &id)?;
    let Some(rodada) = comandos(&cwd, &cfg, projeto.as_ref(), &[], true, true) else {
        return fail("sem_comando", "defina o comando de verificação da missão ou testar em .regente/projeto.json");
    };
    if rodada.do_projeto && !confiado(&s, &m.workspace_id, root) {
        return fail("confianca", "os comandos do projeto.json precisam da sua confiança antes de rodar");
    }
    rodar(&s, &id, &rodada.comandos, cfg.timeout_min, "manual")
}

#[tauri::command]
pub fn mission_verify_cancel(id: String) -> bool {
    cancelar(&id)
}

/// Um agente foi de Trabalhando a Ocioso: se a missão pede verificação ao terminar, roda numa thread própria
/// (o git e os testes levam segundos ou minutos). Só agentes de missão disparam.
pub fn ao_ociosar(s: &Arc<AppState>, painel: &str) {
    let Ok(row) = s.db.get_pane(painel) else { return };
    let Some(missao) = row.mission_id.clone() else { return };
    if row.kind != "agent" {
        return;
    }
    let (alvo, painel) = (s.clone(), painel.to_string());
    let spawned = std::thread::Builder::new().name("regente-verificacao-auto".into()).spawn(move || {
        if let Err(e) = disparar_auto(&alvo, &missao, &painel) {
            tracing::warn!("verificação automática da missão {missao} não rodou: {e}");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("thread da verificação automática não criada: {e}");
    }
}

/// Debounce de disparos automáticos (ms): um Trabalhando->Ocioso logo após o anterior não roda de novo.
const DEBOUNCE_MS: i64 = 10_000;

fn disparar_auto(s: &Arc<AppState>, missao: &str, painel: &str) -> AppResult<()> {
    let rodou = disparar_uma_vez(s, missao, painel);
    // o debounce conta do fim da última rodada; um disparo que chegou no meio roda de novo, uma vez, depois da espera
    let pendente = {
        let mut e = lk(&ESTADO);
        let st = e.entry(missao.to_string()).or_default();
        if matches!(rodou, Ok(true)) {
            st.disparo = now_ms();
        }
        std::mem::take(&mut st.pendente)
    };
    if pendente {
        let (alvo, painel) = (s.clone(), painel.to_string());
        let _ = std::thread::Builder::new().name("regente-verificacao-pendente".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(DEBOUNCE_MS as u64));
            ao_ociosar(&alvo, &painel);
        });
    }
    rodou.map(|_| ())
}

/// Uma tentativa de disparo automático. True = a rodada rodou (e o fim dela conta para o debounce).
fn disparar_uma_vez(s: &AppState, missao: &str, painel: &str) -> AppResult<bool> {
    let m = s.db.get_mission(missao)?;
    let cfg = VerifyCfg::parse(m.verify_json.as_deref());
    if !cfg.rodar_ao_terminar {
        return Ok(false);
    }
    {
        let mut e = lk(&ESTADO);
        let st = e.entry(missao.to_string()).or_default();
        if st.rodando || now_ms() - st.disparo < DEBOUNCE_MS {
            st.pendente = true;
            return Ok(false);
        }
    }
    let cwd = planexec::cwd_da_missao(s, missao)?;
    let (hash, mudados) = mudancas(&cwd);
    let ultimo_hash = lk(&ESTADO).get(missao).and_then(|x| x.hash.clone()).unwrap_or_else(|| hash_de(b"", b""));
    if hash == ultimo_hash {
        return Ok(false);
    }
    let ws = s.db.get_workspace(&m.workspace_id)?;
    let root = Path::new(&ws.path);
    let projeto = crate::projeto::load(root).ok().flatten();
    let relacionados_primeiro = crate::perfil::efetivo_de(s, &m.workspace_id, m.profile.as_deref()).0.testes.relacionados_primeiro;
    let Some(rodada) = comandos(&cwd, &cfg, projeto.as_ref(), &mudados, false, relacionados_primeiro) else { return Ok(false) };
    if rodada.do_projeto && !confiado(s, &ws.id, root) {
        // comando do projeto sem confiança: não roda sozinho; a tela pede a confiança
        s.emit("verificacao-confiar", json!({"workspaceId": ws.id, "missionId": missao}));
        return Ok(false);
    }
    let res = match rodar(s, missao, &rodada.comandos, cfg.timeout_min, "automatica") {
        Ok(r) => r,
        Err(e) if e.code() == "verificacao_em_curso" => {
            lk(&ESTADO).entry(missao.to_string()).or_default().pendente = true;
            return Ok(false);
        }
        Err(e) => return Err(e),
    };
    let tentativa = {
        let mut e = lk(&ESTADO);
        let st = e.entry(missao.to_string()).or_default();
        st.hash = Some(hash);
        st.painel = Some(painel.to_string());
        if res.ok {
            st.iter = 0;
            None
        } else if cfg.auto_loop && st.iter < cfg.max_iter {
            st.iter += 1;
            Some(st.iter)
        } else {
            Some(0)
        }
    };
    if res.ok {
        s.emit("verificacao-ok", json!({"missionId": missao, "paneId": painel, "duracaoMs": res.duration_ms}));
        return Ok(true);
    }
    let limite = cfg.auto_loop && tentativa == Some(0);
    let texto = format!("A verificação falhou: {}. Corrija só isso e entregue de novo.", res.digest);
    match tentativa {
        Some(n) if n > 0 => {
            // ciclo automático: a falha vai ao agente que acabou de terminar
            if let Err(e) = s.write_input(painel, &bracketed(&clean_text(&texto))) {
                tracing::warn!("falha da verificação não entregue a {painel}: {e}");
            }
        }
        _ => {}
    }
    s.emit("verificacao-falhou", json!({"missionId": missao, "paneId": painel, "digest": res.digest, "automatica": cfg.auto_loop, "tentativa": tentativa, "limite": limite}));
    Ok(true)
}

/// "Enviar falha ao agente": a última falha da missão vai ao painel que a recebeu.
#[tauri::command]
pub fn mission_verify_enviar(s: S<'_>, id: String) -> AppResult<()> {
    let (painel, digest) = {
        let e = lk(&ESTADO);
        let st = e.get(&id);
        let painel = st.and_then(|x| x.painel.clone());
        let digest = st.and_then(|x| x.ultimo.as_ref().filter(|u| !u.resultado.ok).map(|u| u.resultado.digest.clone()));
        (painel, digest)
    };
    let (Some(painel), Some(digest)) = (painel, digest) else {
        return fail("sem_falha", "não há falha recente para enviar");
    };
    let texto = format!("A verificação falhou: {digest}. Corrija só isso e entregue de novo.");
    s.write_input(&painel, &bracketed(&clean_text(&texto)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comando_refuses_newlines_controls_and_long_text() {
        assert!(check_cmd("cargo test").is_ok());
        assert!(check_cmd("cargo\ntest").is_err());
        assert!(check_cmd("cargo\x07 test").is_err());
        assert!(check_cmd(&"x".repeat(301)).is_err());
        let mut cfg = VerifyCfg { comando: Some("npm\rtest".into()), ..Default::default() };
        assert!(cfg.clone().sanear().is_err());
        cfg.comando = Some("   ".into());
        assert_eq!(cfg.sanear().unwrap().comando, None);
    }

    #[test]
    fn limits_are_clamped_and_bad_json_falls_back_to_defaults() {
        let cfg = VerifyCfg { max_iter: 99, timeout_min: 0, ..Default::default() }.sanear().unwrap();
        assert_eq!((cfg.max_iter, cfg.timeout_min), (5, 1));
        assert_eq!(VerifyCfg::parse(Some("não é json")), VerifyCfg::default());
        assert!(!VerifyCfg::parse(None).rodar_ao_terminar);
    }

    #[test]
    fn command_precedence_mission_then_project_and_trust_flag() {
        let d = tempfile::tempdir().unwrap();
        let projeto = json!({"comandos": {"testar": "npm test"}});
        // sem comando da missão: o do projeto, e ele exige confiança
        let r = comandos(d.path(), &VerifyCfg::default(), Some(&projeto), &[], true, true).unwrap();
        assert_eq!(r, Rodada { comandos: vec!["npm test".into()], do_projeto: true });
        // comando da missão ganha, e não vem do projeto
        let cfg = VerifyCfg { comando: Some("cargo test".into()), ..Default::default() };
        let r = comandos(d.path(), &cfg, Some(&projeto), &[], true, true).unwrap();
        assert_eq!(r, Rodada { comandos: vec!["cargo test".into()], do_projeto: false });
        // nada configurado: nenhuma rodada
        assert!(comandos(d.path(), &VerifyCfg::default(), None, &[], true, true).is_none());
    }

    #[test]
    fn runner_propagates_exit_code_and_output() {
        let d = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let mut vistos = String::new();
        let s = executar(d.path(), "echo verificacao-ok", Duration::from_secs(60), &cancel, |t| vistos.push_str(t)).unwrap();
        assert!(s.ok && s.exit_code == Some(0));
        assert!(s.texto.contains("verificacao-ok") && vistos.contains("verificacao-ok"));
        let s = executar(d.path(), "cmd /c exit 3", Duration::from_secs(60), &cancel, |_| {}).unwrap();
        assert!(!s.ok);
        assert_eq!(s.exit_code, Some(3));
    }

    #[test]
    fn timeout_kills_the_process_tree() {
        let d = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let inicio = Instant::now();
        let s = executar(d.path(), "ping -n 30 127.0.0.1", Duration::from_secs(1), &cancel, |_| {}).unwrap();
        assert!(!s.ok);
        assert!(s.motivo.is_some_and(|m| m.contains("tempo esgotado")));
        assert!(inicio.elapsed() < Duration::from_secs(20), "o ping de 30 s foi morto antes do fim");
    }

    #[test]
    fn automatic_run_needs_a_changed_tree_and_respects_the_debounce() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        s.db.insert_mission(&crate::db::Mission { id: "mv".into(), workspace_id: ws.id.clone(), name: "m".into(), mode: "livre".into(), status: "ativa".into(), created_at: 1, verify_json: Some(r#"{"comando":"cmd /c exit 1","rodarAoTerminar":true}"#.into()), ..Default::default() }).unwrap();
        // pasta sem git: a árvore não muda (hash vazio), então nada roda
        disparar_auto(&s, "mv", "p").unwrap();
        assert!(lk(&ESTADO).get("mv").and_then(|e| e.ultimo.clone()).is_none(), "árvore sem mudança não roda");
        // debounce: um disparo de agora há pouco não roda de novo
        lk(&ESTADO).entry("mv".into()).or_default().disparo = now_ms();
        disparar_auto(&s, "mv", "p").unwrap();
        assert!(lk(&ESTADO).get("mv").and_then(|e| e.ultimo.clone()).is_none(), "dentro do debounce não roda");
    }

    #[test]
    fn cancel_stops_a_running_command() {
        let d = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            c2.store(true, Ordering::SeqCst);
        });
        let s = executar(d.path(), "ping -n 30 127.0.0.1", Duration::from_secs(60), &cancel, |_| {}).unwrap();
        assert!(!s.ok && s.cancelado && s.motivo.is_some_and(|m| m.contains("cancelado")));
    }

    #[test]
    fn digest_is_compact_for_a_failing_cargo_run() {
        let texto = crate::testes::fixture("cargo_fail");
        let d = digest_de("cargo test", &texto, None);
        assert!(!d.is_empty() && d.chars().count() <= DIGEST_MAX);
    }

    #[test]
    fn tree_hash_changes_with_content() {
        assert_eq!(hash_de(b"", b""), hash_de(b"", b""));
        assert_ne!(hash_de(b" M a.rs\n", b""), hash_de(b"", b""));
        assert_ne!(hash_de(b" M a.rs\n", b" 1 file changed"), hash_de(b" M a.rs\n", b" 2 files changed"));
    }
}
