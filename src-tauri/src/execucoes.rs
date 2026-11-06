//! Execuções de shell (Bash/PowerShell) de um painel do Claude: registro em memória, sem processo. O hook PreToolUse
//! abre a execução; PostToolUse e PostToolUseFailure fecham. Parte pura: quem chama cuida de painéis e de eventos.
//! Tudo é guardado por `(painel do Claude, id)`: um painel nunca alcança a execução de outro.
use crate::execmatch::is_long_running;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Execuções guardadas por painel do Claude (as mais antigas saem).
pub const MAX_POR_PAINEL: usize = 200;
/// Saída guardada por execução: um anel que guarda os últimos bytes.
pub const TETO_SAIDA: usize = 2 * 1024 * 1024;
/// Saída guardada por painel do Claude, somando as execuções: sai a das concluídas mais antigas primeiro.
pub const TETO_PAI: usize = 32 * 1024 * 1024;
/// Bytes devolvidos por `trecho` em cada chamada.
pub const TRECHO_MAX: usize = 256 * 1024;
/// Tamanho máximo de um id de execução ou de tarefa.
pub const MAX_ID: usize = 64;
/// Marca de painel promovido em reserva (o painel ainda não existe).
const PENDENTE: &str = "pendente";
/// Bytes do fim do texto lido que ficam guardados entre passadas (acha o marcador quando ele cai partido).
const JANELA_MARCA: usize = 64;
/// Aviso de teto de promoções: no máximo um por painel do Claude a cada minuto.
const AVISO_CAP_MS: i64 = 60_000;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Rodando,
    /// Em segundo plano: o Claude devolveu a tarefa; ela segue até o fim (a saída vem do arquivo dela).
    Fundo,
    Ok,
    Erro,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Exec {
    pub id: String,
    pub parent_pane: String,
    pub tool: String,
    pub command: String,
    pub description: Option<String>,
    pub background: bool,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: Status,
    pub exit_code: Option<i32>,
    /// Os últimos bytes da saída (anel de `TETO_SAIDA`).
    pub output: String,
    /// Bytes que a saída já produziu, contando os que saíram do anel.
    pub total: u64,
    /// Painel próprio (kind `exec`) da execução, quando promovida.
    pub promoted_pane: Option<String>,
    /// O painel promovido segue aberto (conta no teto de promoções do pai).
    #[serde(skip)]
    pub pane_open: bool,
    /// Id da tarefa em segundo plano (`backgroundTaskId`): é o que o TaskStop para.
    #[serde(skip)]
    pub tarefa: Option<String>,
}

/// O que o hook informa sobre a execução (o mesmo em PreToolUse e nos Post*).
#[derive(Clone, Debug, PartialEq)]
pub struct Dados {
    pub id: String,
    pub tool: String,
    pub command: String,
    pub description: Option<String>,
    pub background: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fim {
    Ok,
    Erro { exit_code: Option<i32> },
    /// Tarefa devolvida em segundo plano: a execução continua aberta.
    Fundo,
}

/// Regra de promoção a painel próprio (ajustes do usuário).
pub struct Regra {
    pub ativa: bool,
    pub apos_ms: i64,
    pub max: usize,
}

/// Pedaço da saída de uma execução: `exec.output` vai de `de` (posição em bytes) até o fim do trecho.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Trecho {
    pub exec: Exec,
    pub de: u64,
}

/// Tarefa em segundo plano sendo acompanhada: raiz e arquivo já validados, quanto já foi lido e os tempos.
#[derive(Clone, Debug, PartialEq)]
pub struct Cauda {
    pub pai: String,
    pub id: String,
    pub tarefa: String,
    pub raiz: PathBuf,
    pub arquivo: PathBuf,
    pub lido: u64,
    /// Fim do texto já anexado (acha o marcador de saída quando ele cai partido entre duas passadas).
    pub resto: String,
    pub ultimo_avanco: i64,
    pub ultima_leitura: i64,
    /// Viu um servidor de pé na saída (ver `servindo`). Fica marcado: o anel da saída pode deixar a linha para trás.
    pub servidor: bool,
}

/// Sem crescer por este tempo (ms), a cauda passa a ser lida só a cada `QUIETO_LEITURA_MS` (nunca para sozinha).
pub const QUIETO_MS: i64 = 10_000;
pub const QUIETO_LEITURA_MS: i64 = 5_000;
/// Bytes lidos por passada (a cada 500 ms), para não travar o ticker.
pub const PASSADA_BYTES: u64 = 256 * 1024;
/// Execução em segundo plano sem saída nova por este tempo deixa de segurar o painel como trabalhando (evento perdido).
pub const FUNDO_VIVO_MS: i64 = 30 * 60_000;

#[derive(Default)]
pub struct Execs {
    por_painel: HashMap<String, Vec<Exec>>,
    fixados: HashSet<String>,
    caudas: HashMap<(String, String), Cauda>,
    avisos_cap: HashMap<String, i64>,
}

/// Id aceito: até 64 caracteres de `[A-Za-z0-9_-]`. Qualquer outra coisa não entra no registro.
pub fn id_valido(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn nova(pai: &str, d: Dados, agora: i64) -> Exec {
    Exec {
        id: d.id,
        parent_pane: pai.into(),
        tool: d.tool,
        command: d.command,
        description: d.description,
        background: d.background,
        started_at: agora,
        ended_at: None,
        status: Status::Rodando,
        exit_code: None,
        output: String::new(),
        total: 0,
        promoted_pane: None,
        pane_open: false,
        tarefa: None,
    }
}

/// Acrescenta à saída e guarda só os últimos `TETO_SAIDA` bytes (em fronteira de caractere).
fn acrescenta(e: &mut Exec, texto: &str) {
    e.total += texto.len() as u64;
    e.output.push_str(texto);
    if e.output.len() > TETO_SAIDA {
        let mut corte = e.output.len() - TETO_SAIDA;
        while !e.output.is_char_boundary(corte) {
            corte += 1;
        }
        e.output.drain(..corte);
    }
}

/// Os últimos `n` bytes do texto (em fronteira de caractere).
fn ultimos_bytes(s: &str, n: usize) -> String {
    let mut ini = s.len().saturating_sub(n);
    while !s.is_char_boundary(ini) {
        ini += 1;
    }
    s[ini..].to_string()
}

/// Se a saída do painel passa do teto, zera a das execuções concluídas mais antigas (a rodando fica).
fn teto_pai(lista: &mut [Exec]) {
    let mut total: usize = lista.iter().map(|e| e.output.len()).sum();
    for e in lista.iter_mut().filter(|e| matches!(e.status, Status::Ok | Status::Erro)) {
        if total <= TETO_PAI {
            break;
        }
        total -= e.output.len();
        e.output = String::new();
    }
}

/// Cópia da execução sem a saída (a lista é relida a cada segundo).
fn sem_saida(e: &Exec) -> Exec {
    Exec {
        id: e.id.clone(),
        parent_pane: e.parent_pane.clone(),
        tool: e.tool.clone(),
        command: e.command.clone(),
        description: e.description.clone(),
        background: e.background,
        started_at: e.started_at,
        ended_at: e.ended_at,
        status: e.status,
        exit_code: e.exit_code,
        output: String::new(),
        total: e.total,
        promoted_pane: e.promoted_pane.clone(),
        pane_open: e.pane_open,
        tarefa: e.tarefa.clone(),
    }
}

/// Arquivo de saída de uma tarefa em segundo plano: `<raiz>/<projeto>/<sessão>/tasks/<tarefa>.output`, onde `raiz` é a
/// pasta temporária do Claude. Só aceita sessão em forma de UUID e tarefa alfanumérica; o arquivo tem de ser comum e
/// ficar dentro da raiz depois de resolvido (nada de `..`, UNC fora dela ou link para outro lugar). Sem isso, None.
pub fn arquivo_de_tarefa(raiz: &Path, sessao: &str, tarefa: &str) -> Option<PathBuf> {
    let uuid = sessao.len() == 36 && sessao.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    let alfanum = (6..=16).contains(&tarefa.len()) && tarefa.chars().all(|c| c.is_ascii_alphanumeric());
    if !uuid || !alfanum {
        return None;
    }
    let raiz = std::fs::canonicalize(raiz).ok()?;
    for projeto in std::fs::read_dir(&raiz).ok()?.flatten() {
        let p = projeto.path().join(sessao).join("tasks").join(format!("{tarefa}.output"));
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if !meta.file_type().is_file() {
            continue;
        }
        if std::fs::canonicalize(&p).is_ok_and(|real| real.starts_with(&raiz)) {
            return Some(p);
        }
    }
    None
}

/// Código do marcador `[exited with code N]` que o Claude põe no fim da saída de uma tarefa em segundo plano.
/// Vale só quando ele é a última linha não vazia do texto.
pub fn codigo_de_saida(texto: &str) -> Option<i32> {
    let ultima = texto.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    ultima.strip_prefix("[exited with code ")?.strip_suffix(']')?.parse().ok()
}

/// A saída mostra um servidor de pé (dev server, watch): ele não termina, então o Claude não está esperando por ele.
/// ponytail: heurística por texto; uma build que imprime "localhost:" cai aqui e volta a avisar como antes.
/// Teto: `python -m http.server`, watcher silencioso e shell morto fora do TaskStop não são pegos (o servidor ou o
/// shell segura o painel até o teto de `FUNDO_VIVO_MS`). Subir a detecção: ler a porta de fato ou o pid do processo.
pub fn servindo(saida: &str) -> bool {
    let s = saida.to_lowercase();
    ["http://localhost", "https://localhost", "http://127.0.0.1", "https://127.0.0.1", "http://0.0.0.0", "listening on", "ready in", "watching for"]
        .iter()
        .any(|p| s.contains(p))
}

fn recusado() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "caminho de tarefa recusado")
}

/// Abre o arquivo da tarefa conferindo o caminho no momento da abertura (o arquivo pode ser trocado depois):
/// sem link simbólico ou junção, sem outro nome de link (hard link) e dentro da raiz.
#[cfg(windows)]
fn abrir_seguro(raiz: &Path, arquivo: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT};
    // abre o próprio link (se for um), sem seguir
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let f = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(arquivo)?;
    let h = f.as_raw_handle();
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(h, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || info.nNumberOfLinks > 1 {
        return Err(recusado());
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(recusado());
    }
    let real = String::from_utf16_lossy(&buf[..n]);
    let real = real.strip_prefix(r"\\?\").unwrap_or(&real).to_lowercase();
    let raiz = raiz.to_string_lossy();
    let raiz = raiz.strip_prefix(r"\\?\").unwrap_or(&raiz).to_lowercase();
    let dentro = real.starts_with(&raiz) && real.get(raiz.len()..).is_some_and(|r| r.starts_with('\\'));
    if !dentro {
        return Err(recusado());
    }
    Ok(f)
}

#[cfg(not(windows))]
fn abrir_seguro(raiz: &Path, arquivo: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // O_NOFOLLOW: o último componente nunca é link simbólico (valor por arquitetura no Linux; BSD e macOS 0x100)
    #[cfg(all(any(target_os = "linux", target_os = "android"), any(target_arch = "aarch64", target_arch = "arm")))]
    const O_NOFOLLOW: i32 = 0o100000;
    #[cfg(all(any(target_os = "linux", target_os = "android"), not(any(target_arch = "aarch64", target_arch = "arm"))))]
    const O_NOFOLLOW: i32 = 0o400000;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const O_NOFOLLOW: i32 = 0x0100;
    let f = std::fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW).open(arquivo)?;
    // conferido no arquivo aberto: é arquivo comum, sem outro nome (hard link), e o caminho real fica dentro da raiz
    let meta = f.metadata()?;
    if !meta.is_file() || meta.nlink() > 1 {
        return Err(recusado());
    }
    if !std::fs::canonicalize(arquivo)?.starts_with(std::fs::canonicalize(raiz)?) {
        return Err(recusado());
    }
    Ok(f)
}

/// Lê do arquivo a partir de `de` (até `PASSADA_BYTES`), só leitura, com a conferência de `abrir_seguro`. Devolve o
/// texto, até onde leu (um caractere UTF-8 partido no fim fica para a próxima passada) e se chegou ao fim do arquivo.
/// Arquivo encolhido (recomeçou) volta ao início.
pub fn ler_desde(raiz: &Path, arquivo: &Path, de: u64) -> std::io::Result<(String, u64, bool)> {
    let mut f = abrir_seguro(raiz, arquivo)?;
    let tam = f.metadata()?.len();
    let de = if de > tam { 0 } else { de };
    f.seek(SeekFrom::Start(de))?;
    let mut buf = Vec::new();
    f.take(PASSADA_BYTES).read_to_end(&mut buf)?;
    let corte = match std::str::from_utf8(&buf) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _ => buf.len(),
    };
    let ate = de + corte as u64;
    Ok((String::from_utf8_lossy(&buf[..corte]).into_owned(), ate, ate == tam))
}

/// Inicia uma execução na lista do painel: nada se o id é inválido ou já existe.
fn inicio_em(lista: &mut Vec<Exec>, pai: &str, d: Dados, agora: i64) -> Option<Exec> {
    if !id_valido(&d.id) || lista.iter().any(|e| e.id == d.id) {
        return None;
    }
    let e = nova(pai, d, agora);
    lista.push(e.clone());
    Some(e)
}

/// Corta a lista do painel no máximo. Devolve os ids que saíram: a cauda deles sai junto (ver `tira_caudas`).
fn corta(lista: &mut Vec<Exec>) -> Vec<String> {
    let mut saiu = Vec::new();
    while lista.len() > MAX_POR_PAINEL {
        saiu.push(lista.remove(0).id);
    }
    saiu
}

/// Tira as caudas de execuções que saíram da lista (sem isto, a cauda seguiria lendo uma execução sem registro).
fn tira_caudas(caudas: &mut HashMap<(String, String), Cauda>, pai: &str, ids: &[String]) {
    for id in ids {
        caudas.remove(&(pai.to_string(), id.clone()));
    }
}

impl Execs {
    /// Abre uma execução (PreToolUse). Id já visto: nada, devolve None.
    pub fn inicio(&mut self, pai: &str, d: Dados, agora: i64) -> Option<Exec> {
        let lista = self.por_painel.entry(pai.into()).or_default();
        let e = inicio_em(lista, pai, d, agora)?;
        let saiu = corta(lista);
        tira_caudas(&mut self.caudas, pai, &saiu);
        Some(e)
    }

    /// Fecha uma execução (PostToolUse, PostToolUseFailure). Sem abertura antes (evento fora de ordem), cria já
    /// fechada. Já fechada, ou fim de fundo sobre uma execução que não está rodando: nada, devolve None.
    pub fn fim(&mut self, pai: &str, d: Dados, fim: Fim, saida: &str, agora: i64) -> Option<Exec> {
        if !id_valido(&d.id) {
            return None;
        }
        let (status, exit_code) = match fim {
            Fim::Ok => (Status::Ok, None),
            Fim::Erro { exit_code } => (Status::Erro, exit_code),
            Fim::Fundo => (Status::Fundo, None),
        };
        let lista = self.por_painel.entry(pai.into()).or_default();
        let resultado = if let Some(e) = lista.iter_mut().find(|e| e.id == d.id) {
            if e.ended_at.is_some() || (fim == Fim::Fundo && e.status != Status::Rodando) {
                return None;
            }
            e.output.clear();
            acrescenta(e, saida);
            e.status = status;
            e.exit_code = exit_code;
            if fim != Fim::Fundo {
                e.ended_at = Some(agora);
            }
            Some(e.clone())
        } else {
            let mut e = nova(pai, d, agora);
            acrescenta(&mut e, saida);
            e.status = status;
            e.exit_code = exit_code;
            e.ended_at = (fim != Fim::Fundo).then_some(agora);
            lista.push(e.clone());
            let saiu = corta(lista);
            tira_caudas(&mut self.caudas, pai, &saiu);
            Some(e)
        };
        let lista = self.por_painel.entry(pai.into()).or_default();
        teto_pai(lista);
        resultado
    }

    fn busca(&self, pai: &str, id: &str) -> Option<&Exec> {
        if !id_valido(id) {
            return None;
        }
        self.por_painel.get(pai)?.iter().find(|e| e.id == id)
    }

    fn busca_mut(&mut self, pai: &str, id: &str) -> Option<&mut Exec> {
        if !id_valido(id) {
            return None;
        }
        self.por_painel.get_mut(pai)?.iter_mut().find(|e| e.id == id)
    }

    /// Execuções do painel, da mais antiga para a mais nova.
    pub fn lista(&self, pai: &str) -> Vec<Exec> {
        self.por_painel.get(pai).cloned().unwrap_or_default()
    }

    /// Verificação (X1b): uma execução por missão, reaproveitada. Cada rodada acrescenta um separador e abre a
    /// execução como "rodando"; a saída entra por `verificacao_texto` e a rodada fecha em `verificacao_fim`.
    pub fn verificacao_inicio(&mut self, pai: &str, d: Dados, separador: &str, agora: i64) {
        let id = d.id.clone();
        let lista = self.por_painel.entry(pai.into()).or_default();
        if let Some(e) = lista.iter_mut().find(|e| e.id == id) {
            e.command = d.command;
            e.status = Status::Rodando;
            e.exit_code = None;
            e.ended_at = None;
            e.started_at = agora;
            acrescenta(e, separador);
        } else {
            let mut e = nova(pai, d, agora);
            acrescenta(&mut e, separador);
            lista.push(e);
            let saiu = corta(lista);
            tira_caudas(&mut self.caudas, pai, &saiu);
        }
        if let Some(lista) = self.por_painel.get_mut(pai) {
            teto_pai(lista);
        }
    }

    /// Acrescenta um pedaço da saída da verificação em andamento (chamado a cada leitura do processo).
    pub fn verificacao_texto(&mut self, pai: &str, id: &str, texto: &str) {
        if let Some(e) = self.busca_mut(pai, id) {
            acrescenta(e, texto);
        }
        if let Some(lista) = self.por_painel.get_mut(pai) {
            teto_pai(lista);
        }
    }

    /// Fecha a rodada da verificação: `Ok` com código 0, `Erro` com o código (ou sem código, em caso de tempo esgotado).
    pub fn verificacao_fim(&mut self, pai: &str, id: &str, fim: Fim, agora: i64) {
        if let Some(e) = self.busca_mut(pai, id) {
            let (status, code) = match fim {
                Fim::Ok => (Status::Ok, Some(0)),
                Fim::Erro { exit_code } => (Status::Erro, exit_code),
                Fim::Fundo => (Status::Fundo, None),
            };
            e.status = status;
            e.exit_code = code;
            e.ended_at = Some(agora);
        }
    }

    /// Execuções do painel sem a saída (a lista é relida a cada segundo; a saída vem por `trecho`).
    pub fn resumos(&self, pai: &str) -> Vec<Exec> {
        self.por_painel.get(pai).map_or_else(Vec::new, |l| l.iter().map(sem_saida).collect())
    }

    /// Pedaço da saída de uma execução do painel, a partir de `desde` (bytes), no máximo `TRECHO_MAX`. Só o que
    /// ainda está no anel: o que saiu dele começa de onde o anel começa. Outro painel não alcança a execução.
    pub fn trecho(&self, pai: &str, id: &str, desde: u64) -> Option<Trecho> {
        let e = self.busca(pai, id)?;
        let base = e.total - e.output.len() as u64;
        let ini = desde.max(base).min(e.total);
        let fim = e.total.min(ini + TRECHO_MAX as u64);
        let mut a = (ini - base) as usize;
        let mut b = (fim - base) as usize;
        while a < e.output.len() && !e.output.is_char_boundary(a) {
            a += 1;
        }
        while b > a && !e.output.is_char_boundary(b) {
            b -= 1;
        }
        let mut exec = sem_saida(e);
        exec.output = e.output[a..b].to_string();
        Some(Trecho { exec, de: base + a as u64 })
    }

    /// Execuções ainda em andamento (sem promoção) que a regra pode promover: (pai, execução).
    pub fn candidatas(&self) -> Vec<(String, Exec)> {
        self.por_painel.values().flatten().filter(|e| e.promoted_pane.is_none() && matches!(e.status, Status::Rodando | Status::Fundo)).map(|e| (e.parent_pane.clone(), e.clone())).collect()
    }

    /// Painéis promovidos de um pai que seguem abertos (ou reservados).
    pub fn promovidas_abertas(&self, pai: &str) -> usize {
        self.por_painel.get(pai).map_or(0, |l| l.iter().filter(|e| e.pane_open).count())
    }

    /// Reserva a promoção de uma execução, sob o mesmo lock que confere o teto. Falso: teto cheio, já promovida
    /// ou execução desconhecida. Quem reservou cria o painel e chama `marcar_promovida`, ou `liberar_promocao`.
    pub fn reservar_promocao(&mut self, pai: &str, id: &str, max: usize) -> bool {
        if self.promovidas_abertas(pai) >= max {
            return false;
        }
        let Some(e) = self.busca_mut(pai, id) else { return false };
        if e.promoted_pane.is_some() {
            return false;
        }
        e.promoted_pane = Some(PENDENTE.into());
        e.pane_open = true;
        true
    }

    /// A criação do painel falhou: a reserva volta, e a execução pode tentar de novo.
    pub fn liberar_promocao(&mut self, pai: &str, id: &str) {
        if let Some(e) = self.busca_mut(pai, id).filter(|e| e.promoted_pane.as_deref() == Some(PENDENTE)) {
            e.promoted_pane = None;
            e.pane_open = false;
        }
    }

    pub fn marcar_promovida(&mut self, pai: &str, id: &str, painel: &str) {
        if let Some(e) = self.busca_mut(pai, id) {
            e.promoted_pane = Some(painel.into());
            e.pane_open = true;
        }
    }

    /// O painel promovido foi fechado (pelo usuário ou pelo prazo).
    pub fn painel_fechado(&mut self, painel: &str) {
        for e in self.por_painel.values_mut().flatten().filter(|e| e.promoted_pane.as_deref() == Some(painel)) {
            e.pane_open = false;
        }
        self.fixados.remove(painel);
    }

    /// "Fixar": o painel promovido não fecha sozinho.
    pub fn fixar(&mut self, painel: &str, fixar: bool) {
        if fixar {
            self.fixados.insert(painel.into());
        } else {
            self.fixados.remove(painel);
        }
    }

    pub fn fixado(&self, painel: &str) -> bool {
        self.fixados.contains(painel)
    }

    /// O painel do Claude fechou: some com as execuções, as caudas e os avisos dele.
    pub fn fechar_pai(&mut self, pai: &str) {
        for p in self.por_painel.remove(pai).unwrap_or_default().iter().filter_map(|e| e.promoted_pane.as_deref()) {
            self.fixados.remove(p);
        }
        self.caudas.retain(|(p, _), _| p != pai);
        self.avisos_cap.remove(pai);
    }

    /// Guarda a tarefa em segundo plano da execução (`backgroundTaskId`), para o TaskStop achar a execução.
    pub fn vincular_tarefa(&mut self, pai: &str, id: &str, tarefa: &str) {
        if !id_valido(tarefa) {
            return;
        }
        if let Some(e) = self.busca_mut(pai, id) {
            e.tarefa = Some(tarefa.into());
        }
    }

    /// Começa a acompanhar a saída de uma tarefa em segundo plano. `raiz` e `arquivo` já vieram validados.
    pub fn acompanhar(&mut self, pai: &str, id: &str, tarefa: &str, raiz: PathBuf, arquivo: PathBuf, agora: i64) {
        if !id_valido(id) {
            return;
        }
        // a saída que a execução já tem (um servidor pode ter subido antes da cauda): conta também
        let servidor = self.busca(pai, id).is_some_and(|e| servindo(&e.output));
        let c = Cauda { pai: pai.into(), id: id.into(), tarefa: tarefa.into(), raiz, arquivo, lido: 0, resto: String::new(), ultimo_avanco: agora, ultima_leitura: agora, servidor };
        self.caudas.insert((pai.into(), id.into()), c);
    }

    /// Caudas a ler agora: a cada tick enquanto crescem; quietas por `QUIETO_MS`, a cada `QUIETO_LEITURA_MS`.
    /// Nenhuma sai sozinha: o fim vem do marcador de saída, do TaskStop ou do fechamento do painel.
    pub fn caudas_devidas(&mut self, agora: i64) -> Vec<Cauda> {
        let mut devidas = Vec::new();
        for c in self.caudas.values_mut() {
            let espera = if agora - c.ultimo_avanco < QUIETO_MS { 0 } else { QUIETO_LEITURA_MS };
            if agora - c.ultima_leitura >= espera {
                c.ultima_leitura = agora;
                devidas.push(c.clone());
            }
        }
        devidas
    }

    #[cfg(test)]
    pub fn tem_cauda(&self, pai: &str, id: &str) -> bool {
        self.caudas.contains_key(&(pai.into(), id.into()))
    }

    /// Painéis com execução em segundo plano que o Claude ainda espera: aberta, acompanhada (a cauda sabe quando acaba),
    /// com saída nova nos últimos `FUNDO_VIVO_MS` e que não é servidor.
    pub fn pais_com_fundo(&self, agora: i64) -> HashSet<String> {
        self.por_painel
            .iter()
            .filter(|(pai, v)| {
                v.iter().any(|e| {
                    e.status == Status::Fundo
                        && e.ended_at.is_none()
                        && self.caudas.get(&((*pai).clone(), e.id.clone())).is_some_and(|c| !c.servidor && agora - c.ultimo_avanco.max(e.started_at) < FUNDO_VIVO_MS)
                })
            })
            .map(|(pai, _)| pai.clone())
            .collect()
    }

    /// Para de acompanhar sem fechar a execução (o arquivo deixou de ser válido).
    pub fn parar_cauda(&mut self, pai: &str, id: &str) {
        self.caudas.remove(&(pai.into(), id.into()));
    }

    /// Anexa o texto lido da tarefa à saída da execução. `ate` é até onde leu; `eof`, se chegou ao fim do arquivo.
    /// O marcador `[exited with code N]` fecha a execução, mas só no fim do arquivo e como última linha.
    /// Devolve a execução que o marcador fechou (para o app anunciar o fim no painel promovido).
    pub fn anexar(&mut self, pai: &str, id: &str, texto: &str, ate: u64, eof: bool, agora: i64) -> Option<Exec> {
        let chave = (pai.to_string(), id.to_string());
        // execução que já não existe (saiu da lista): a cauda não tem mais o que preencher
        if self.busca(pai, id).is_none() {
            self.caudas.remove(&chave);
            return None;
        }
        let c = self.caudas.get_mut(&chave)?;
        c.lido = ate;
        if !texto.is_empty() {
            c.ultimo_avanco = agora;
        }
        let janela = format!("{}{texto}", c.resto);
        // uma vez que apareceu, fica: o servidor pode ter saído do anel (2 MB) antes de a próxima leitura chegar
        if servindo(&janela) {
            c.servidor = true;
        }
        c.resto = ultimos_bytes(&janela, JANELA_MARCA);
        let codigo = if eof { codigo_de_saida(&janela) } else { None };
        if !texto.is_empty() {
            if let Some(e) = self.busca_mut(pai, id) {
                acrescenta(e, texto);
            }
            if let Some(lista) = self.por_painel.get_mut(pai) {
                teto_pai(lista);
            }
        }
        let mut fechada = None;
        if let Some(code) = codigo {
            if let Some(e) = self.busca_mut(pai, id).filter(|e| e.ended_at.is_none()) {
                e.status = if code == 0 { Status::Ok } else { Status::Erro };
                e.exit_code = Some(code);
                e.ended_at = Some(agora);
                fechada = Some(e.clone());
            }
            self.caudas.remove(&chave);
        }
        fechada
    }

    /// TaskStop da tarefa: a cauda para e a execução em segundo plano fecha como erro (parada, sem código).
    /// Falso: nenhuma execução do painel tem essa tarefa.
    pub fn tarefa_parada(&mut self, pai: &str, tarefa: &str, agora: i64) -> Option<Exec> {
        if !id_valido(tarefa) {
            return None;
        }
        let e = self.por_painel.get_mut(pai)?.iter_mut().find(|e| e.tarefa.as_deref() == Some(tarefa))?;
        if e.status == Status::Fundo && e.ended_at.is_none() {
            e.status = Status::Erro;
            e.ended_at = Some(agora);
        }
        let id = e.id.clone();
        let atual = e.clone();
        self.caudas.remove(&(pai.to_string(), id));
        Some(atual)
    }

    /// Execução do painel pelo id (cópia, com a saída).
    pub fn achar(&self, pai: &str, id: &str) -> Option<Exec> {
        self.busca(pai, id).cloned()
    }

    /// Promovidas já concluídas com sucesso que seguem abertas e não estão fixadas (o alvo de "Fechar execuções concluídas").
    pub fn concluidas_sem_fixar(&self, pai: &str) -> Vec<String> {
        self.por_painel
            .get(pai)
            .into_iter()
            .flatten()
            .filter(|e| e.status == Status::Ok && e.pane_open)
            .filter_map(|e| e.promoted_pane.clone())
            .filter(|p| p != PENDENTE && !self.fixados.contains(p))
            .collect()
    }

    /// Aviso de teto de promoções: true uma vez por painel do Claude a cada minuto.
    pub fn avisar_cap(&mut self, pai: &str, agora: i64) -> bool {
        match self.avisos_cap.get(pai) {
            Some(t) if agora - t < AVISO_CAP_MS => false,
            _ => {
                self.avisos_cap.insert(pai.into(), agora);
                true
            }
        }
    }

    /// Painéis promovidos que concluíram com sucesso, não estão fixados e passaram do prazo (ms desde o fim).
    /// Com erro, o painel fica aberto.
    pub fn fechar_devidos(&self, agora: i64, apos_ms: i64) -> Vec<String> {
        self.por_painel
            .values()
            .flatten()
            .filter(|e| e.pane_open && e.status == Status::Ok)
            .filter(|e| e.ended_at.is_some_and(|t| agora - t >= apos_ms))
            .filter_map(|e| e.promoted_pane.clone())
            .filter(|p| p != PENDENTE && !self.fixados.contains(p))
            .collect()
    }
}

/// Deve esta execução virar painel próprio agora? Só enquanto ela roda (ou está em segundo plano), com a regra
/// ligada, sem promoção anterior, e com vaga entre as promovidas do pai. Quer: segundo plano, comando longo
/// (servidor, teste, watch) ou execução que passou do prazo.
pub fn deve_promover(e: &Exec, agora: i64, regra: &Regra, promovidas_abertas: usize) -> bool {
    let ativa = matches!(e.status, Status::Rodando | Status::Fundo);
    regra.ativa
        && ativa
        && e.promoted_pane.is_none()
        && promovidas_abertas < regra.max
        && (e.background || is_long_running(&e.command) || (e.status == Status::Rodando && agora - e.started_at >= regra.apos_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dados(id: &str, cmd: &str) -> Dados {
        Dados { id: id.into(), tool: "Bash".into(), command: cmd.into(), description: None, background: false }
    }

    const REGRA: Regra = Regra { ativa: true, apos_ms: 5_000, max: 6 };

    fn um(x: &Execs, pai: &str, id: &str) -> Exec {
        x.lista(pai).into_iter().find(|e| e.id == id).unwrap()
    }

    /// Caminho de arquivo de tarefa num diretório temporário (a raiz já canônica, como o app passa).
    fn tarefa_em(d: &tempfile::TempDir, nome: &str, conteudo: &[u8]) -> (PathBuf, PathBuf) {
        let raiz = std::fs::canonicalize(d.path()).unwrap();
        let f = raiz.join(nome);
        std::fs::write(&f, conteudo).unwrap();
        (raiz, f)
    }

    #[test]
    fn pre_then_post_ok_closes_with_output() {
        let mut x = Execs::default();
        let e = x.inicio("p", dados("u1", "ls"), 1_000).unwrap();
        assert_eq!(e.status, Status::Rodando);
        let f = x.fim("p", dados("u1", "ls"), Fim::Ok, "a\nb", 1_400).unwrap();
        assert_eq!((f.status, f.ended_at, f.exit_code, f.output.as_str()), (Status::Ok, Some(1_400), None, "a\nb"));
        assert_eq!(x.lista("p").len(), 1);
    }

    #[test]
    fn pre_then_failure_closes_as_erro_with_code() {
        let mut x = Execs::default();
        x.inicio("p", dados("u1", "false"), 0).unwrap();
        let f = x.fim("p", dados("u1", "false"), Fim::Erro { exit_code: Some(1) }, "Exit code 1\nboom", 10).unwrap();
        assert_eq!((f.status, f.exit_code), (Status::Erro, Some(1)));
    }

    #[test]
    fn failure_without_a_code_line_keeps_exit_code_empty() {
        let mut x = Execs::default();
        x.inicio("p", dados("u1", "x"), 0).unwrap();
        let f = x.fim("p", dados("u1", "x"), Fim::Erro { exit_code: None }, "o shell não subiu", 5).unwrap();
        assert_eq!((f.status, f.exit_code), (Status::Erro, None));
    }

    #[test]
    fn background_post_stays_open_until_the_task_ends() {
        let mut x = Execs::default();
        x.inicio("p", Dados { background: true, ..dados("b1", "npm run dev") }, 0).unwrap();
        let f = x.fim("p", Dados { background: true, ..dados("b1", "npm run dev") }, Fim::Fundo, "iniciado", 20).unwrap();
        assert_eq!((f.status, f.ended_at), (Status::Fundo, None));
        // um segundo Post de fundo não reabre nem altera
        assert!(x.fim("p", dados("b1", "npm run dev"), Fim::Fundo, "outra", 30).is_none());
    }

    #[test]
    fn servindo_reconhece_servidor_de_desenvolvimento() {
        assert!(servindo("  VITE v5  ready in 300 ms\n  ➜  Local:   http://localhost:5173/\n"));
        assert!(servindo("Now listening on: http://127.0.0.1:5000"));
        assert!(servindo("webpack: watching for file changes"));
        assert!(!servindo("Compiling regente v1.3.4\n   Finished dev profile"));
    }

    #[test]
    fn pais_com_fundo_so_conta_tarefa_acompanhada_viva_e_que_nao_serve() {
        let mut x = Execs::default();
        let d = |id: &str| Dados { id: id.into(), tool: "Bash".into(), command: "cargo build".into(), description: None, background: true };
        x.inicio("pai", d("b1"), 0);
        x.fim("pai", d("b1"), Fim::Fundo, "", 0);
        // sem acompanhar o arquivo: não há como saber o fim, não conta
        assert!(x.pais_com_fundo(1_000).is_empty());
        x.acompanhar("pai", "b1", "t1", PathBuf::from("C:/raiz"), PathBuf::from("C:/raiz/t1.output"), 0);
        assert!(x.pais_com_fundo(1_000).contains("pai"));
        // 30 min sem saída nova: deixa de contar
        assert!(x.pais_com_fundo(FUNDO_VIVO_MS + 1).is_empty());
        // servidor: não conta
        x.anexar("pai", "b1", "Local: http://localhost:5173/\n", 30, false, 2_000);
        assert!(x.pais_com_fundo(3_000).is_empty());
    }

    #[test]
    fn servidor_partido_entre_duas_leituras_tambem_e_pego() {
        let mut x = Execs::default();
        let d = Dados { id: "b1".into(), tool: "Bash".into(), command: "npm run dev".into(), description: None, background: true };
        x.inicio("pai", d.clone(), 0);
        x.fim("pai", d, Fim::Fundo, "", 0);
        x.acompanhar("pai", "b1", "t1", PathBuf::from("C:/raiz"), PathBuf::from("C:/raiz/t1.output"), 0);
        x.anexar("pai", "b1", "Local: http://local", 20, false, 1_000);
        assert!(x.pais_com_fundo(1_000).contains("pai"), "ainda não dá para saber");
        x.anexar("pai", "b1", "host:5173/\n", 40, false, 2_000);
        assert!(x.pais_com_fundo(2_000).is_empty(), "a URL partida foi reconhecida");
    }

    #[test]
    fn servidor_visto_continua_marcado_mesmo_quando_sai_do_anel() {
        let mut x = Execs::default();
        let d = Dados { id: "b1".into(), tool: "Bash".into(), command: "npm run dev".into(), description: None, background: true };
        x.inicio("pai", d.clone(), 0);
        x.fim("pai", d, Fim::Fundo, "", 0);
        x.acompanhar("pai", "b1", "t1", PathBuf::from("C:/raiz"), PathBuf::from("C:/raiz/t1.output"), 0);
        x.anexar("pai", "b1", "Local: http://localhost:5173/\n", 30, false, 1_000);
        // a saída passa do teto do anel: a linha do servidor sai do texto guardado
        let muito = "x".repeat(TETO_SAIDA + 10);
        x.anexar("pai", "b1", &muito, 30 + muito.len() as u64, false, 2_000);
        assert!(!um(&x, "pai", "b1").output.contains("localhost"), "o anel já largou a linha");
        assert!(x.pais_com_fundo(2_500).is_empty(), "mas o servidor segue marcado");
    }

    #[test]
    fn servidor_ja_na_saida_quando_a_cauda_comeca_tambem_conta() {
        let mut x = Execs::default();
        let d = Dados { id: "b1".into(), tool: "Bash".into(), command: "npm run dev".into(), description: None, background: true };
        x.inicio("pai", d.clone(), 0);
        x.fim("pai", d, Fim::Fundo, "Local: http://localhost:5173/", 0);
        x.acompanhar("pai", "b1", "t1", PathBuf::from("C:/raiz"), PathBuf::from("C:/raiz/t1.output"), 0);
        assert!(x.pais_com_fundo(1_000).is_empty());
    }

    #[test]
    fn duplicate_and_out_of_order_events_are_tolerated() {
        let mut x = Execs::default();
        assert!(x.inicio("p", dados("u1", "ls"), 0).is_some());
        assert!(x.inicio("p", dados("u1", "ls"), 1).is_none(), "Pre repetido");
        x.fim("p", dados("u1", "ls"), Fim::Ok, "", 2).unwrap();
        assert!(x.fim("p", dados("u1", "ls"), Fim::Ok, "de novo", 3).is_none(), "Post repetido");
        // Post antes do Pre: a execução nasce já fechada, e o Pre atrasado não a duplica
        let f = x.fim("p", dados("u2", "pwd"), Fim::Ok, "/c", 5).unwrap();
        assert_eq!(f.status, Status::Ok);
        assert!(x.inicio("p", dados("u2", "pwd"), 6).is_none());
        assert_eq!(x.lista("p").len(), 2);
    }

    #[test]
    fn caps_limit_the_list_and_the_output_ring() {
        let mut x = Execs::default();
        for i in 0..(MAX_POR_PAINEL + 5) {
            x.inicio("p", dados(&format!("u{i}"), "ls"), i as i64).unwrap();
        }
        let l = x.lista("p");
        assert_eq!(l.len(), MAX_POR_PAINEL);
        assert_eq!(l[0].id, "u5", "as mais antigas saem");
        let grande = "é".repeat(TETO_SAIDA);
        let f = x.fim("p", dados("u300", "ls"), Fim::Ok, &grande, 1).unwrap();
        assert!(f.output.len() <= TETO_SAIDA && f.output.ends_with('é'), "guarda só o fim, em caractere inteiro");
        assert_eq!(f.total, grande.len() as u64);
    }

    #[test]
    fn output_ring_serves_only_the_increment_from_a_position() {
        let mut x = Execs::default();
        x.inicio("p", dados("r", "cat"), 0).unwrap();
        let grande = format!("{}fim", "a".repeat(TETO_SAIDA));
        x.fim("p", dados("r", "cat"), Fim::Ok, &grande, 1).unwrap();
        let total = grande.len() as u64;
        let primeiro = x.trecho("p", "r", 0).unwrap();
        assert_eq!(primeiro.de, total - TETO_SAIDA as u64, "o anel começa onde o que saiu termina");
        assert_eq!(primeiro.exec.output.len(), TRECHO_MAX, "no máximo 256 KB por chamada");
        let seguinte = x.trecho("p", "r", primeiro.de + TRECHO_MAX as u64).unwrap();
        assert_eq!(seguinte.exec.output.len(), TRECHO_MAX);
        let fim = x.trecho("p", "r", total).unwrap();
        assert!(fim.exec.output.is_empty() && fim.de == total);
        assert_eq!(fim.exec.total, total);
    }

    #[test]
    fn per_parent_cap_clears_the_oldest_finished_output_first() {
        let mut x = Execs::default();
        for i in 0..20 {
            let id = format!("e{i}");
            x.inicio("p", dados(&id, "build"), i).unwrap();
            x.fim("p", dados(&id, "build"), Fim::Ok, &"a".repeat(TETO_SAIDA), i + 1).unwrap();
        }
        let l = x.lista("p");
        let soma: usize = l.iter().map(|e| e.output.len()).sum();
        assert!(soma <= TETO_PAI, "{soma}");
        assert!(l[0].output.is_empty() && l[19].output.len() == TETO_SAIDA, "as mais antigas perdem a saída");
        assert_eq!(l[0].total, TETO_SAIDA as u64, "o total não se perde");
    }

    #[test]
    fn promotion_rules_five_seconds_background_matcher_and_cap() {
        let mut x = Execs::default();
        x.inicio("p", dados("a", "ls"), 0).unwrap();
        let e = |x: &Execs, id: &str| um(x, "p", id);
        assert!(!deve_promover(&e(&x, "a"), 4_999, &REGRA, 0), "abaixo de 5 s");
        assert!(deve_promover(&e(&x, "a"), 5_000, &REGRA, 0), "passou de 5 s");
        x.inicio("p", dados("s", "npm run dev"), 0).unwrap();
        assert!(deve_promover(&e(&x, "s"), 0, &REGRA, 0), "servidor de dev sobe logo");
        x.inicio("p", Dados { background: true, ..dados("b", "sleep 99") }, 0).unwrap();
        assert!(deve_promover(&e(&x, "b"), 0, &REGRA, 0), "segundo plano sobe logo");
        assert!(!deve_promover(&e(&x, "s"), 0, &REGRA, 6), "teto de 6 por pai");
        assert!(!deve_promover(&e(&x, "s"), 0, &Regra { ativa: false, ..REGRA }, 0), "regra desligada");
        x.fim("p", dados("a", "ls"), Fim::Ok, "", 6_000).unwrap();
        assert!(!deve_promover(&e(&x, "a"), 9_000, &REGRA, 0), "já concluída não promove");
    }

    #[test]
    fn promotion_reservation_respects_the_cap_and_rolls_back() {
        let mut x = Execs::default();
        x.inicio("p", dados("a", "npm run dev"), 0).unwrap();
        x.inicio("p", dados("b", "npm run dev"), 0).unwrap();
        assert!(x.reservar_promocao("p", "a", 1));
        assert!(!x.reservar_promocao("p", "a", 2), "já reservada");
        assert!(!x.reservar_promocao("p", "b", 1), "teto cheio");
        x.liberar_promocao("p", "a");
        assert!(x.reservar_promocao("p", "b", 1), "a vaga voltou");
        x.marcar_promovida("p", "b", "painel-b");
        assert_eq!(um(&x, "p", "b").promoted_pane.as_deref(), Some("painel-b"));
        assert!(!x.reservar_promocao("p", "a", 1));
    }

    #[test]
    fn concurrent_promotions_never_pass_the_cap() {
        let x = std::sync::Arc::new(std::sync::Mutex::new(Execs::default()));
        for i in 0..16 {
            x.lock().unwrap().inicio("p", dados(&format!("e{i}"), "npm run dev"), 0).unwrap();
        }
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let x = x.clone();
                std::thread::spawn(move || x.lock().unwrap().reservar_promocao("p", &format!("e{i}"), 6))
            })
            .collect();
        let ok = handles.into_iter().filter_map(|h| h.join().ok()).filter(|&b| b).count();
        assert_eq!(ok, 6);
        assert_eq!(x.lock().unwrap().promovidas_abertas("p"), 6);
    }

    #[test]
    fn a_forged_id_from_another_pane_cannot_touch_the_execution() {
        let mut x = Execs::default();
        x.inicio("a", dados("e1", "npm run dev"), 0).unwrap();
        assert!(x.trecho("b", "e1", 0).is_none(), "outro painel não lê");
        x.marcar_promovida("b", "e1", "painel-falso");
        x.fixar("painel-falso", true);
        assert_eq!(um(&x, "a", "e1").promoted_pane, None, "outro painel não promove");
        assert!(!x.reservar_promocao("b", "e1", 6));
        assert!(x.fim("b", dados("e1", "npm run dev"), Fim::Ok, "", 1).is_some(), "só cria a dele");
        assert_eq!(um(&x, "a", "e1").status, Status::Rodando, "a execução de A segue");
        x.acompanhar("b", "e1", "t", PathBuf::from("."), PathBuf::from("nao.output"), 0);
        x.anexar("b", "e1", "x", 1, true, 1);
        assert_eq!(um(&x, "a", "e1").output, "");
    }

    #[test]
    fn ids_are_capped_and_restricted() {
        assert!(id_valido("toolu_01-ABC"));
        assert!(!id_valido(""), "vazio");
        assert!(!id_valido("../x"), "caractere de caminho");
        assert!(!id_valido("a b"), "espaço");
        assert!(id_valido(&"a".repeat(64)));
        assert!(!id_valido(&"a".repeat(65)));
        let mut x = Execs::default();
        assert!(x.inicio("p", dados("../x", "ls"), 0).is_none());
    }

    #[test]
    fn background_task_file_is_found_only_inside_the_temp_tree() {
        let d = tempfile::tempdir().unwrap();
        let sess = "11111111-2222-3333-4444-555555555555";
        let dir = d.path().join("proj-a").join(sess).join("tasks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b7e8gruuy.output"), "pronto").unwrap();
        assert!(arquivo_de_tarefa(d.path(), sess, "b7e8gruuy").is_some());
        assert!(arquivo_de_tarefa(d.path(), sess, "../../x").is_none(), "tarefa com caractere de caminho");
        assert!(arquivo_de_tarefa(d.path(), "../etc", "b7e8gruuy").is_none(), "sessão fora do formato");
        assert!(arquivo_de_tarefa(d.path(), sess, "naoexiste").is_none());
        assert!(arquivo_de_tarefa(&d.path().join("ausente"), sess, "b7e8gruuy").is_none());
        assert!(arquivo_de_tarefa(Path::new(r"\\servidor\compartilhado"), sess, "b7e8gruuy").is_none(), "UNC");
    }

    #[cfg(windows)]
    #[test]
    fn background_task_file_that_is_a_link_out_of_the_tree_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let fora = tempfile::tempdir().unwrap();
        std::fs::write(fora.path().join("seg.txt"), "fora").unwrap();
        let sess = "11111111-2222-3333-4444-555555555555";
        let dir = d.path().join("proj-a").join(sess).join("tasks");
        std::fs::create_dir_all(&dir).unwrap();
        // link de arquivo pode exigir modo de desenvolvedor no Windows: sem ele, o caso não se monta
        if std::os::windows::fs::symlink_file(fora.path().join("seg.txt"), dir.join("abc123.output")).is_err() {
            return;
        }
        assert!(arquivo_de_tarefa(d.path(), sess, "abc123").is_none());
        let raiz = std::fs::canonicalize(d.path()).unwrap();
        assert!(ler_desde(&raiz, &dir.join("abc123.output"), 0).is_err(), "a abertura também recusa");
    }

    #[test]
    fn reading_refuses_a_hard_link_missing_files_and_keeps_the_root() {
        let d = tempfile::tempdir().unwrap();
        let fora = tempfile::tempdir().unwrap();
        std::fs::write(fora.path().join("seg.txt"), "fora").unwrap();
        let (raiz, f) = tarefa_em(&d, "tarefa.output", b"ok");
        assert_eq!(ler_desde(&raiz, &f, 0).unwrap().0, "ok", "arquivo comum lê");
        let ligado = raiz.join("ligado.output");
        std::fs::hard_link(fora.path().join("seg.txt"), &ligado).unwrap();
        assert!(ler_desde(&raiz, &ligado, 0).is_err(), "outro nome de link: recusa");
        assert!(ler_desde(&raiz, &raiz.join("sumiu.output"), 0).is_err(), "arquivo ausente");
    }

    #[test]
    fn tail_appends_only_new_bytes_and_keeps_polling_slowly_when_quiet() {
        let d = tempfile::tempdir().unwrap();
        let (raiz, f) = tarefa_em(&d, "t.output", b"linha 1\n");
        let mut x = Execs::default();
        x.inicio("p", dados("b", "npm run dev"), 0).unwrap();
        x.fim("p", Dados { background: true, ..dados("b", "npm run dev") }, Fim::Fundo, "", 0).unwrap();
        x.acompanhar("p", "b", "t1", raiz.clone(), f.clone(), 0);
        let c = x.caudas_devidas(1_000).pop().unwrap();
        let (texto, ate, eof) = ler_desde(&c.raiz, &c.arquivo, c.lido).unwrap();
        x.anexar("p", &c.id, &texto, ate, eof, 1_000);
        assert_eq!(um(&x, "p", "b").output, "linha 1\n");
        std::fs::write(&f, "linha 1\nlinha 2\n").unwrap();
        let c = x.caudas_devidas(2_000).pop().unwrap();
        let (texto, ate, eof) = ler_desde(&c.raiz, &c.arquivo, c.lido).unwrap();
        assert_eq!(texto, "linha 2\n", "só o que é novo");
        x.anexar("p", &c.id, &texto, ate, eof, 2_000);
        assert_eq!(um(&x, "p", "b").output, "linha 1\nlinha 2\n");
        // quieta por 10 s: segue na lista, lida a cada 5 s (não some)
        assert_eq!(x.caudas_devidas(2_000 + QUIETO_MS).len(), 1, "passou a 5 s de intervalo");
        assert!(x.caudas_devidas(2_000 + QUIETO_MS + 1_000).is_empty(), "ainda não é hora");
        assert_eq!(x.caudas_devidas(2_000 + QUIETO_MS + QUIETO_LEITURA_MS).len(), 1, "continua acompanhando");
    }

    #[test]
    fn an_evicted_execution_takes_its_tail_with_it() {
        let mut x = Execs::default();
        x.inicio("p", dados("velho", "longo"), 1).unwrap();
        x.acompanhar("p", "velho", "t1", PathBuf::from("."), PathBuf::from("nao-importa.output"), 1);
        for i in 0..MAX_POR_PAINEL {
            x.inicio("p", dados(&format!("n{i:03}"), "ls"), 2).unwrap();
        }
        assert!(x.achar("p", "velho").is_none(), "saiu da lista");
        assert!(!x.tem_cauda("p", "velho"), "a cauda de uma execução que saiu também sai");
    }

    #[test]
    fn a_tail_without_its_execution_is_dropped_on_the_next_pass() {
        let mut x = Execs::default();
        x.acompanhar("p", "fantasma", "t1", PathBuf::from("."), PathBuf::from("nao-importa.output"), 1);
        assert!(x.anexar("p", "fantasma", "saida", 5, false, 2).is_none());
        assert!(!x.tem_cauda("p", "fantasma"));
    }

    #[test]
    fn the_marker_hands_back_the_execution_it_closed() {
        let mut x = Execs::default();
        x.inicio("p", dados("b", "make"), 0).unwrap();
        x.fim("p", Dados { background: true, ..dados("b", "make") }, Fim::Fundo, "", 0).unwrap();
        x.acompanhar("p", "b", "t1", PathBuf::from("."), PathBuf::from("nao-importa.output"), 0);
        assert!(x.anexar("p", "b", "pronto
[exited with code 0]", 28, true, 900).is_some_and(|e| e.status == Status::Ok && e.id == "b"));
        assert!(x.anexar("p", "b", "", 28, true, 901).is_none(), "já fechada: nada a anunciar de novo");
    }

    #[test]
    fn the_task_trailer_closes_the_execution_with_its_exit_code() {
        assert_eq!(codigo_de_saida("probe-done\n\n[exited with code 0]\n"), Some(0));
        assert_eq!(codigo_de_saida("x\n[exited with code 2]"), Some(2));
        assert_eq!(codigo_de_saida("sem marcador"), None);
        assert_eq!(codigo_de_saida("[exited with code 2]\nmais texto"), None, "não é a última linha");
        let mut x = Execs::default();
        x.inicio("p", dados("b", "make"), 0).unwrap();
        x.fim("p", Dados { background: true, ..dados("b", "make") }, Fim::Fundo, "", 0).unwrap();
        x.acompanhar("p", "b", "t1", PathBuf::from("."), PathBuf::from("nao-importa.output"), 0);
        x.anexar("p", "b", "falhou\n[exited with code 2]", 25, false, 500);
        assert_eq!(um(&x, "p", "b").status, Status::Fundo, "sem o fim do arquivo, não fecha");
        x.anexar("p", "b", "", 25, true, 600);
        let e = um(&x, "p", "b");
        assert_eq!((e.status, e.exit_code, e.ended_at), (Status::Erro, Some(2), Some(600)));
        assert!(x.caudas_devidas(700).is_empty(), "terminou: não acompanha mais");
    }

    #[test]
    fn a_marker_split_between_two_passes_is_still_found() {
        let mut x = Execs::default();
        x.inicio("p", dados("b", "make"), 0).unwrap();
        x.fim("p", Dados { background: true, ..dados("b", "make") }, Fim::Fundo, "", 0).unwrap();
        x.acompanhar("p", "b", "t1", PathBuf::from("."), PathBuf::from("nao.output"), 0);
        x.anexar("p", "b", "pronto\n[exited with co", 22, true, 1);
        x.anexar("p", "b", "de 0]\n", 28, true, 2);
        let e = um(&x, "p", "b");
        assert_eq!((e.status, e.exit_code), (Status::Ok, Some(0)));
        assert_eq!(e.output, "pronto\n[exited with code 0]\n", "a saída junta as duas passadas sem repetir");
    }

    #[test]
    fn utf8_split_across_passes_waits_for_the_rest_of_the_character() {
        let d = tempfile::tempdir().unwrap();
        let (raiz, f) = tarefa_em(&d, "u.output", &[0xC3]);
        let (texto, ate, eof) = ler_desde(&raiz, &f, 0).unwrap();
        assert_eq!((texto.as_str(), ate, eof), ("", 0, false), "o byte partido fica para depois");
        std::fs::write(&f, [0xC3, 0xA9]).unwrap();
        let (texto, ate, eof) = ler_desde(&raiz, &f, 0).unwrap();
        assert_eq!((texto.as_str(), ate, eof), ("é", 2, true));
    }

    #[test]
    fn a_file_that_shrinks_is_read_again_from_the_start() {
        let d = tempfile::tempdir().unwrap();
        let (raiz, f) = tarefa_em(&d, "t.output", b"abc");
        assert_eq!(ler_desde(&raiz, &f, 99).unwrap(), ("abc".to_string(), 3, true));
    }

    #[test]
    fn a_task_stop_ends_the_tail_and_the_background_execution() {
        let mut x = Execs::default();
        x.inicio("p", dados("b", "npm run dev"), 0).unwrap();
        x.fim("p", Dados { background: true, ..dados("b", "npm run dev") }, Fim::Fundo, "", 0).unwrap();
        x.vincular_tarefa("p", "b", "bgtask01");
        x.acompanhar("p", "b", "bgtask01", PathBuf::from("."), PathBuf::from("nao.output"), 0);
        assert!(x.tarefa_parada("p", "outra01", 5).is_none(), "tarefa de outro id não para nada");
        assert!(x.tarefa_parada("outro", "bgtask01", 5).is_none(), "tarefa de outro painel não para nada");
        assert!(x.tarefa_parada("p", "bgtask01", 5).is_some());
        let e = um(&x, "p", "b");
        assert_eq!((e.status, e.ended_at), (Status::Erro, Some(5)));
        assert!(x.caudas_devidas(6).is_empty());
    }

    #[test]
    fn closing_the_parent_drops_its_executions_tails_and_pins() {
        let mut x = Execs::default();
        x.inicio("p", dados("r", "npm run dev"), 0).unwrap();
        x.marcar_promovida("p", "r", "painel-r");
        x.fixar("painel-r", true);
        x.acompanhar("p", "r", "t", PathBuf::from("."), PathBuf::from("nao.output"), 0);
        x.inicio("outro", dados("o", "ls"), 0).unwrap();
        x.fechar_pai("p");
        assert!(x.lista("p").is_empty() && x.trecho("p", "r", 0).is_none());
        assert!(!x.fixado("painel-r"));
        assert!(x.caudas_devidas(1).is_empty());
        assert_eq!(x.lista("outro").len(), 1, "os outros painéis seguem");
    }

    #[test]
    fn cap_warning_shows_once_per_parent_per_minute() {
        let mut x = Execs::default();
        assert!(x.avisar_cap("p", 0));
        assert!(!x.avisar_cap("p", 59_999));
        assert!(x.avisar_cap("p", 60_000));
        assert!(x.avisar_cap("q", 1), "outro pai tem o seu");
    }

    #[test]
    fn promoted_panes_close_on_success_after_the_delay_unless_pinned_or_failed() {
        let mut x = Execs::default();
        for (id, cmd) in [("ok", "npm test"), ("erro", "npm test"), ("fixo", "npm test")] {
            x.inicio("p", dados(id, cmd), 0).unwrap();
            x.marcar_promovida("p", id, &format!("painel-{id}"));
        }
        x.fim("p", dados("ok", "npm test"), Fim::Ok, "", 1_000).unwrap();
        x.fim("p", dados("erro", "npm test"), Fim::Erro { exit_code: Some(1) }, "", 1_000).unwrap();
        x.fim("p", dados("fixo", "npm test"), Fim::Ok, "", 1_000).unwrap();
        x.fixar("painel-fixo", true);
        assert_eq!(x.fechar_devidos(20_999, 20_000), Vec::<String>::new());
        assert_eq!(x.fechar_devidos(21_000, 20_000), vec!["painel-ok".to_string()]);
        assert_eq!(x.promovidas_abertas("p"), 3, "nada fechou ainda");
        x.painel_fechado("painel-ok");
        assert_eq!(x.promovidas_abertas("p"), 2);
    }
}
