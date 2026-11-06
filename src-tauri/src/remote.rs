//! Workspaces remotos: `ssh://alias/caminho/absoluto`. Usa o OpenSSH do sistema e tmux no servidor.
//! Nunca guarda senha: depende do ssh-agent, de chaves e do `~/.ssh/config`.
use crate::error::{fail, AppError, AppResult};
use crate::files::{FileText, FsEntry, IGNORED, MAX_TEXT};
use crate::pty::SpawnPlan;
use crate::providers::which;
use crate::util::cmd;
use base64::Engine;
use std::io::{Read, Write};
use std::process::{ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const SCHEME: &str = "ssh://";
/// Sessões tmux criadas pelo Regente; só essas são encerradas.
const OWN_PREFIX: &str = "regente-";
/// Intervalo entre leituras remotas do custo.
pub const COST_EVERY: Duration = Duration::from_secs(5);
/// Tamanho máximo do comando remoto (vai inteiro, em base64, na linha do ssh).
pub const MAX_CMD: usize = 12 * 1024;
/// Chamada não interativa que passa disso é morta.
const WATCHDOG: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct Remote {
    pub alias: String,
    /// Caminho absoluto no servidor.
    pub path: String,
}

pub fn is_remote(p: &str) -> bool {
    p.starts_with(SCHEME)
}

/// Alias do `~/.ssh/config` ou `usuario@host`: nada que comece com `-` (evita `-oProxyCommand=`).
pub fn valid_alias(a: &str) -> bool {
    !a.is_empty() && a.len() <= 255 && !a.starts_with('-') && a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'))
}

/// Caminho absoluto, sem controles, aspas nem `..`.
pub fn valid_path(p: &str) -> bool {
    p.starts_with('/') && p.chars().all(|c| !c.is_control() && !matches!(c, '\'' | '"' | '`' | '\\')) && !p.split('/').any(|s| s == "..")
}

/// Nome de sessão tmux ou texto livre sem controles nem aspas (sempre entra quotado no shell remoto).
fn safe_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| !c.is_control() && !matches!(c, '\'' | '"' | '`' | '\\'))
}

/// `ssh://alias/caminho` → Remote. None para caminho local ou inválido.
pub fn parse_ws_path(p: &str) -> Option<Remote> {
    let rest = p.strip_prefix(SCHEME)?;
    let (alias, path) = rest.split_once('/')?;
    let path = format!("/{}", path.trim_end_matches('/'));
    (valid_alias(alias) && valid_path(&path)).then(|| Remote { alias: alias.into(), path })
}

/// Caminho de pasta/arquivo: None = local; Err = `ssh://` malformado (nunca cai no caminho local).
pub fn remote_cwd(p: &str) -> AppResult<Option<Remote>> {
    if !is_remote(p) {
        return Ok(None);
    }
    parse_ws_path(p).map(Some).ok_or_else(|| AppError::new("bad_remote", format!("caminho remoto inválido: {p}")))
}

/// URL de um caminho remoto (usada em FsEntry e no workspace).
pub fn ws_url(alias: &str, path: &str) -> String {
    format!("{SCHEME}{alias}{path}")
}

/// Um caminho remoto está dentro de um workspace remoto aberto (mesmo alias, prefixo de pasta).
/// Devolve o alvo e a raiz do workspace (o servidor confere a raiz de novo, com realpath).
pub fn within(roots: &[String], p: &str) -> AppResult<(Remote, String)> {
    let r = parse_ws_path(p).ok_or_else(|| AppError::new("bad_remote", "caminho remoto inválido"))?;
    let root = roots
        .iter()
        .filter_map(|w| parse_ws_path(w).map(|root| (w, root)))
        .find(|(_, root)| root.alias == r.alias && (r.path == root.path || r.path.starts_with(&format!("{}/", root.path))))
        .map(|(w, root)| (w.clone(), root.path));
    match root {
        Some((_, root_path)) => Ok((r, root_path)),
        None => fail("outside_workspace", "caminho fora dos workspaces abertos"),
    }
}

/// Aspas simples POSIX: `it's` → `'it'\''s'`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Opções do ssh do terminal interativo (o usuário pode precisar aceitar host key ou digitar senha de chave).
pub fn ssh_base(alias: &str) -> Vec<String> {
    ["-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2", alias].iter().map(|s| s.to_string()).collect()
}

/// Opções do ssh para chamadas não interativas: nunca pergunta nada.
pub fn ssh_batch(alias: &str) -> Vec<String> {
    let mut v = vec!["-o".to_string(), "BatchMode=yes".to_string()];
    v.extend(ssh_base(alias));
    v
}

/// `regente-<uuid completo>`: o id do painel inteiro, sem colisão.
pub fn tmux_session(pane_id: &str) -> String {
    format!("{OWN_PREFIX}{pane_id}")
}

/// Sessões tmux: `(nome, externa)`. Externa = não criada pelo Regente.
pub fn parse_sessions(out: &str) -> Vec<(String, bool)> {
    out.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| (l.to_string(), !l.starts_with(OWN_PREFIX))).collect()
}

/// Linha ASCII enviada ao ssh: o comando inteiro vai em base64 e é executado por `sh` no servidor.
pub fn remote_line(script: &str) -> AppResult<String> {
    if script.len() > MAX_CMD {
        return fail("bad_arg", "comando remoto grande demais");
    }
    Ok(format!("echo {} | base64 -d | sh", base64::engine::general_purpose::STANDARD.encode(script)))
}

/// Payload do painel: roda sob um shell de login (PATH de ~/.profile). Lê do `/dev/tty` porque, dentro do
/// pipe `| sh`, o stdin do tmux seria o pipe e não um terminal.
fn pane_payload(script: &str) -> String {
    format!("exec sh -lc {} </dev/tty", sh_quote(script))
}

/// Plano do PTY local: `ssh -tt … "tmux new-session -A …"`. `-A` reanexa se a sessão já existe.
pub fn spawn_plan(r: &Remote, pane_id: &str, remote_cmd: &str) -> AppResult<SpawnPlan> {
    let sess = tmux_session(pane_id);
    let tmux = format!("tmux new-session -A -s {} -c {} {}", sh_quote(&sess), sh_quote(&r.path), sh_quote(remote_cmd));
    let mut args = vec!["-tt".to_string()];
    args.extend(ssh_base(&r.alias));
    args.push(remote_line(&pane_payload(&tmux))?);
    Ok(SpawnPlan { program: "ssh".into(), args, session_id: Some(sess), ..Default::default() })
}

/// Anexa a uma sessão externa (leitura e escrita). A sessão guardada no painel é o próprio nome externo.
pub fn attach_plan(r: &Remote, sess: &str) -> AppResult<SpawnPlan> {
    if !safe_text(sess) {
        return fail("bad_session", "nome de sessão inválido");
    }
    let tmux = format!("tmux attach-session -t {}", sh_quote(&format!("={sess}")));
    let mut args = vec!["-tt".to_string()];
    args.extend(ssh_base(&r.alias));
    args.push(remote_line(&pane_payload(&tmux))?);
    Ok(SpawnPlan { program: "ssh".into(), args, session_id: Some(sess.into()), ..Default::default() })
}

/// Comando remoto que encerra uma sessão. Recusa qualquer nome que não seja do Regente.
pub fn kill_cmd(sess: &str) -> AppResult<String> {
    if !sess.starts_with(OWN_PREFIX) || !safe_text(sess) {
        return fail("bad_session", "só sessões regente-* são encerradas pelo Regente");
    }
    Ok(format!("tmux kill-session -t {}", sh_quote(&format!("={sess}"))))
}

pub fn ssh_program() -> AppResult<String> {
    which("ssh").ok_or_else(|| AppError::new("ssh_missing", "OpenSSH não encontrado: instale o cliente OpenSSH do Windows"))
}

fn err_text(out: &Output) -> String {
    let t = String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>();
    if t.is_empty() { "falhou sem mensagem do servidor".into() } else { t }
}

/// Erro de um comando remoto pelo código de saída. Códigos 4 e 5 são os do próprio script (ver `resolve`).
pub fn status_error(code: Option<i32>, stderr: &str) -> AppError {
    match code {
        Some(4) => AppError::new("outside_workspace", "caminho fora do workspace remoto"),
        Some(3) => AppError::new("changed_on_disk", "o arquivo mudou no servidor desde que foi aberto"),
        Some(5) => AppError::new("not_file", "não é um arquivo"),
        _ => AppError::new("remote_failed", stderr.trim().chars().take(300).collect::<String>()),
    }
}

fn ok_or(out: &Output) -> AppResult<()> {
    if out.status.success() {
        return Ok(());
    }
    Err(status_error(out.status.code(), &String::from_utf8_lossy(&out.stderr)))
}

/// Roda um processo; passado `WATCHDOG`, é morto e vira erro. stdout/stderr são lidos em threads
/// (sem isso o pipe encheria e o processo travaria). Erros de escrita no stdin são ignorados: o
/// código de saída e o stderr dizem o que aconteceu.
fn run_watched(mut c: std::process::Command, input: Option<Vec<u8>>) -> AppResult<Output> {
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn()?;
    if let Some(mut si) = child.stdin.take() {
        let data = input.unwrap_or_default();
        thread::spawn(move || {
            let _ = si.write_all(&data);
        });
    }
    let (out_pipe, err_pipe) = (child.stdout.take(), child.stderr.take());
    let h_out = thread::spawn(move || {
        let mut v = Vec::new();
        if let Some(mut p) = out_pipe {
            let _ = p.read_to_end(&mut v);
        }
        v
    });
    let h_err = thread::spawn(move || {
        let mut v = Vec::new();
        if let Some(mut p) = err_pipe {
            let _ = p.read_to_end(&mut v);
        }
        v
    });
    let deadline = Instant::now() + WATCHDOG;
    let mut timed_out = false;
    let status: ExitStatus = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(50));
    };
    if timed_out {
        return fail("remote_timeout", "o servidor não respondeu em 30 s");
    }
    Ok(Output { status, stdout: h_out.join().unwrap_or_default(), stderr: h_err.join().unwrap_or_default() })
}

/// Executa um script no servidor por ssh não interativo (sem terminal, sem senha, com tempo-limite).
fn ssh_run(alias: &str, script: &str) -> AppResult<Output> {
    let line = remote_line(script)?;
    let mut c = cmd(&ssh_program()?);
    c.args(ssh_batch(alias)).arg(line);
    run_watched(c, None)
}

/// Como `ssh_run`, mas com `input` no stdin do script.
fn ssh_run_input(alias: &str, script: &str, input: &[u8]) -> AppResult<Output> {
    let line = remote_line(script)?;
    let mut c = cmd(&ssh_program()?);
    c.args(ssh_batch(alias)).arg(line);
    run_watched(c, Some(input.to_vec()))
}

/// Aliases concretos do `~/.ssh/config` (sem curingas: `*`, `?`, `!`), para sugerir no diálogo.
pub fn parse_ssh_config(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if !parts.next().is_some_and(|k| k.eq_ignore_ascii_case("host")) {
            continue;
        }
        for h in parts.filter(|h| valid_alias(h)) {
            if !out.iter().any(|x| x == h) {
                out.push(h.to_string());
            }
        }
    }
    out
}

pub fn ssh_aliases() -> Vec<String> {
    std::fs::read_to_string(crate::util::home_dir().join(".ssh").join("config")).map(|t| parse_ssh_config(&t)).unwrap_or_default()
}

/// Resultado de `check`: o que é aviso (não impede abrir).
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// O CLI pedido não está no PATH de login do servidor.
    pub cli_missing: bool,
}

/// Pré-requisitos ao abrir, numa só chamada ssh: tmux, a pasta, GNU coreutils (erros) e o CLI (aviso).
/// O script roda sob `sh -lc`, então o PATH é o de login do usuário.
pub fn check(alias: &str, path: &str, cli: &str) -> AppResult<Check> {
    if !valid_alias(alias) {
        return fail("bad_remote", "alias SSH inválido");
    }
    let script = format!(
        "command -v tmux >/dev/null 2>&1 || exit 10; [ -d {p} ] || exit 11; stat -c %Y / >/dev/null 2>&1 || exit 12; \
         if command -v {c} >/dev/null 2>&1; then echo cli=ok; else echo cli=missing; fi",
        p = sh_quote(path),
        c = sh_quote(cli)
    );
    let out = ssh_run(alias, &format!("sh -lc {}", sh_quote(&script)))?;
    match out.status.code() {
        Some(0) => Ok(Check { cli_missing: String::from_utf8_lossy(&out.stdout).contains("cli=missing") }),
        Some(10) => fail("no_tmux", format!("tmux não está instalado em {alias}")),
        Some(11) => fail("bad_path", format!("a pasta {path} não existe em {alias}")),
        Some(12) => fail("no_gnu", format!("o servidor {alias} não tem GNU coreutils (stat -c)")),
        _ => fail("ssh_unreachable", format!("não consegui conectar a {alias} sem senha (use chave no ssh-agent ou ~/.ssh/config). {}", err_text(&out))),
    }
}

/// Sessões tmux do servidor. Sem servidor tmux rodando = lista vazia.
pub fn list_sessions(alias: &str) -> AppResult<Vec<(String, bool)>> {
    let out = ssh_run(alias, &format!("tmux ls -F {}", sh_quote("#S")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("no server running") || err.contains("error connecting to") {
            return Ok(vec![]);
        }
        return fail("remote_failed", err_text(&out));
    }
    Ok(parse_sessions(&String::from_utf8_lossy(&out.stdout)))
}

pub fn kill_session(alias: &str, sess: &str) -> AppResult<()> {
    let out = ssh_run(alias, &kill_cmd(sess)?)?;
    ok_or(&out)
}

/// Primeira sessão de custo do Claude no servidor (`~/.claude/projects/*/<sid>.jsonl`).
pub fn session_find_cmd(sid: &str) -> Option<String> {
    let ok = !sid.is_empty() && sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| format!("ls -d ~/.claude/projects/*/{sid}.jsonl 2>/dev/null | head -n 1"))
}

pub fn claude_session_file(alias: &str, sid: &str) -> AppResult<Option<String>> {
    let Some(c) = session_find_cmd(sid) else { return Ok(None) };
    let out = ssh_run(alias, &c)?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!line.is_empty()).then_some(line))
}

/// Resolve `q` no servidor (realpath) e exige que fique dentro de `root`; senão sai com 4. Depois, `"$r"` é o caminho real.
pub fn resolve(q: &str, root: &str) -> String {
    let qr = sh_quote(root);
    format!("r=$(realpath -m -- {}) && case \"$r\" in {qr}|{qr}/*) ;; *) exit 4;; esac", sh_quote(q))
}

/// Como `resolve`, mas a raiz é a pasta de projetos do Claude (`$HOME/.claude/projects`).
fn resolve_projects(q: &str) -> String {
    format!("r=$(realpath -m -- {}) && case \"$r\" in \"$HOME\"/.claude/projects/*) ;; *) exit 4;; esac", sh_quote(q))
}

/// Tamanho atual do arquivo (primeira linha) e os bytes a partir de `offset` (no máximo 8 MiB por leitura).
pub fn tail_cmd(file: &str, offset: u64) -> String {
    format!("{} && stat -c %s \"$r\" && tail -c +{} \"$r\" | head -c {}", resolve_projects(file), offset + 1, 8u64 << 20)
}

/// Saída do `tail_cmd` → (tamanho, bytes novos).
pub fn parse_tail(out: &[u8]) -> AppResult<(u64, Vec<u8>)> {
    let nl = out.iter().position(|b| *b == b'\n').ok_or_else(|| AppError::new("remote_failed", "resposta inesperada do servidor"))?;
    let size = String::from_utf8_lossy(&out[..nl]).trim().parse::<u64>().map_err(|_| AppError::new("remote_failed", "resposta inesperada do servidor"))?;
    Ok((size, out[nl + 1..].to_vec()))
}

pub fn remote_tail(alias: &str, file: &str, offset: u64) -> AppResult<(u64, Vec<u8>)> {
    let out = ssh_run(alias, &tail_cmd(file, offset))?;
    ok_or(&out)?;
    parse_tail(&out.stdout)
}

/// Espera entre leituras de custo que falharam: 5 s, dobrando, até 60 s.
pub fn cost_backoff(fails: u32) -> Duration {
    let secs = 5u64.saturating_mul(1u64 << fails.saturating_sub(1).min(8));
    Duration::from_secs(secs.min(60))
}

/// `find` do diretório, com o alvo resolvido dentro da raiz: tipo (dereferenciado) e caminho, um por linha (GNU find).
pub fn find_cmd(root: &str, dir: &str) -> String {
    format!("{} && find \"$r\" -mindepth 1 -maxdepth 1 -printf '%Y\\t%p\\n'", resolve(dir, root))
}

/// Saída do `find_cmd` → entradas do explorador (sem IGNORED, pastas primeiro).
pub fn parse_find(alias: &str, out: &str) -> Vec<FsEntry> {
    let mut v: Vec<FsEntry> = out
        .lines()
        .filter_map(|l| {
            let (y, p) = l.split_once('\t')?;
            let name = p.rsplit('/').next()?.to_string();
            if name.is_empty() || IGNORED.contains(&name.as_str()) {
                return None;
            }
            Some(FsEntry { name, path: ws_url(alias, p), is_dir: y == "d" })
        })
        .collect();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    v
}

/// Lista a pasta `r.path` (dentro da raiz `root`).
pub fn remote_list(r: &Remote, root: &str) -> AppResult<Vec<FsEntry>> {
    let out = ssh_run(&r.alias, &find_cmd(root, &r.path))?;
    ok_or(&out)?;
    Ok(parse_find(&r.alias, &String::from_utf8_lossy(&out.stdout)))
}

/// `stat` (mtime e tamanho) seguido do começo do arquivo, limitado a MAX_TEXT + 1 bytes. Exige arquivo regular.
pub fn read_cmd(root: &str, file: &str) -> String {
    format!("{} && {{ [ -f \"$r\" ] || exit 5; }} && stat -c '%Y %s' \"$r\" && head -c {n} \"$r\"", resolve(file, root), n = MAX_TEXT + 1)
}

/// Saída do `read_cmd` → texto, com os mesmos erros de `files::read_text`.
pub fn parse_read(out: &[u8]) -> AppResult<FileText> {
    let nl = out.iter().position(|b| *b == b'\n').ok_or_else(|| AppError::new("remote_failed", "resposta inesperada do servidor"))?;
    let head = String::from_utf8_lossy(&out[..nl]).to_string();
    let mut it = head.split_whitespace();
    let mtime: i64 = it.next().and_then(|v| v.parse().ok()).ok_or_else(|| AppError::new("remote_failed", "resposta inesperada do servidor"))?;
    let size: u64 = it.next().and_then(|v| v.parse().ok()).ok_or_else(|| AppError::new("remote_failed", "resposta inesperada do servidor"))?;
    if size > MAX_TEXT {
        return fail("too_big", "arquivo maior que 5 MB");
    }
    let body = &out[nl + 1..];
    if body.contains(&0) {
        return fail("binary", "arquivo binário");
    }
    let text = String::from_utf8(body.to_vec()).map_err(|_| AppError::new("encoding", "arquivo não está em UTF-8"))?;
    Ok(FileText { text, mtime_ms: mtime * 1000 })
}

/// Lê o arquivo `r.path` (dentro da raiz `root`).
pub fn remote_read(r: &Remote, root: &str) -> AppResult<FileText> {
    let out = ssh_run(&r.alias, &read_cmd(root, &r.path))?;
    ok_or(&out)?;
    parse_read(&out.stdout)
}

/// Saída de `stat -c %Y` (segundos) → ms, como `files` faz.
pub fn parse_mtime(out: &str) -> AppResult<i64> {
    out.trim().parse::<i64>().map(|s| s * 1000).map_err(|_| AppError::new("remote_failed", "resposta inesperada do servidor"))
}

/// Script de gravação: confere o mtime (exit 3 se mudou, ou se não der para ler e havia um esperado),
/// grava o stdin num temporário ao lado e troca com `mv`, para o arquivo nunca ficar pela metade.
pub fn write_cmd(root: &str, file: &str, expected_secs: Option<i64>) -> String {
    let check = match expected_secs {
        Some(e) => format!("[ \"$(stat -c %Y \"$f\" 2>/dev/null)\" = {e} ] || exit 3; "),
        None => String::new(),
    };
    format!(
        "{} && f=\"$r\"; {check}t=$(mktemp \"$f.XXXXXX\") && cat >\"$t\" && {{ chmod --reference=\"$f\" \"$t\" 2>/dev/null || true; }} && mv -f \"$t\" \"$f\" && stat -c %Y \"$f\"",
        resolve(file, root)
    )
}

/// Grava o texto (stdin) dentro da raiz. `expected_ms`: o mtime que o editor leu (ms, como `read`).
pub fn remote_write(r: &Remote, root: &str, text: &str, expected_ms: Option<i64>) -> AppResult<i64> {
    let out = ssh_run_input(&r.alias, &write_cmd(root, &r.path, expected_ms.map(|ms| ms / 1000)), text.as_bytes())?;
    ok_or(&out)?;
    parse_mtime(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(path: &str) -> Remote {
        Remote { alias: "srv".into(), path: path.into() }
    }

    /// Extrai o script de uma linha `echo <b64> | base64 -d | sh`.
    fn decode(line: &str) -> String {
        let b = line.strip_prefix("echo ").unwrap().split(" | base64 -d | sh").next().unwrap();
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b).unwrap()).unwrap()
    }

    #[test]
    fn parses_remote_workspace_paths() {
        assert_eq!(parse_ws_path("ssh://srv/home/u/proj"), Some(r("/home/u/proj")));
        assert_eq!(parse_ws_path("ssh://user@srv/home/u/proj/"), Some(Remote { alias: "user@srv".into(), path: "/home/u/proj".into() }));
        assert_eq!(parse_ws_path("D:\\proj"), None, "local não é remoto");
        assert_eq!(parse_ws_path("ssh://srv"), None, "sem caminho");
        assert_eq!(parse_ws_path("ssh://-oProxyCommand=x/home"), None, "alias com traço inicial");
        assert_eq!(parse_ws_path("ssh://srv/home/u/it's"), None, "aspas recusadas");
        assert_eq!(parse_ws_path("ssh://srv/home/../etc"), None, "`..` recusado");
        assert_eq!(parse_ws_path("ssh://srv/home/a\nb"), None, "controle recusado");
        assert!(remote_cwd("D:\\proj").unwrap().is_none());
        assert_eq!(remote_cwd("ssh://-x/home").unwrap_err().code(), "bad_remote");
    }

    #[test]
    fn quotes_for_posix_sh() {
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote("a b $x"), "'a b $x'");
    }

    #[test]
    fn ssh_options_split_interactive_and_batch() {
        let base = ["-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2", "srv"];
        assert_eq!(ssh_base("srv"), base);
        assert_eq!(ssh_batch("srv")[..2], ["-o", "BatchMode=yes"]);
        assert_eq!(ssh_batch("srv")[2..], base);
    }

    #[test]
    fn remote_line_round_trips_hostile_scripts() {
        for s in ["echo 'a'\"b\"", "x\\y", "a\nb", "echo $(whoami) `id`", "echo ação ✓ 😀", "stat -c '%Y' \"$r\""] {
            let line = remote_line(s).unwrap();
            assert!(line.is_ascii(), "linha só ASCII: {line}");
            assert_eq!(decode(&line), s);
        }
    }

    #[test]
    fn remote_line_caps_the_payload() {
        assert!(remote_line(&"a".repeat(MAX_CMD)).is_ok());
        assert_eq!(remote_line(&"a".repeat(MAX_CMD + 1)).unwrap_err().code(), "bad_arg");
    }

    #[test]
    fn tmux_session_uses_the_full_uuid() {
        assert_eq!(tmux_session("3f2a9c1e-8b7d-4e6f-9a0b-1c2d3e4f5a6b"), "regente-3f2a9c1e-8b7d-4e6f-9a0b-1c2d3e4f5a6b");
    }

    #[test]
    fn spawn_plan_reattaches_with_quoted_command_in_login_shell() {
        let p = spawn_plan(&r("/home/u/p"), "abcdef12-0000", "claude -p 'oi'").unwrap();
        assert_eq!(p.program, "ssh");
        assert_eq!(p.args[0], "-tt");
        assert_eq!(&p.args[1..8], ["-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2", "srv"]);
        let tmux = format!("tmux new-session -A -s {} -c {} {}", sh_quote("regente-abcdef12-0000"), sh_quote("/home/u/p"), sh_quote("claude -p 'oi'"));
        assert_eq!(decode(&p.args[8]), format!("exec sh -lc {} </dev/tty", sh_quote(&tmux)));
        assert_eq!(p.session_id.as_deref(), Some("regente-abcdef12-0000"));
    }

    #[test]
    fn attach_targets_exact_external_session() {
        let p = attach_plan(&r("/x"), "work").unwrap();
        let script = decode(p.args.last().unwrap());
        assert!(script.starts_with("exec sh -lc ") && script.ends_with(" </dev/tty"), "{script}");
        assert!(script.contains("attach-session -t") && script.contains("=work"), "{script}");
        assert_eq!(p.session_id.as_deref(), Some("work"));
        assert_eq!(attach_plan(&r("/x"), "a'b").unwrap_err().code(), "bad_session");
    }

    #[test]
    fn kill_only_touches_own_sessions() {
        assert_eq!(kill_cmd("regente-abcdef12").unwrap(), "tmux kill-session -t '=regente-abcdef12'");
        assert_eq!(kill_cmd("work").unwrap_err().code(), "bad_session");
        assert_eq!(kill_cmd("regente-x';rm").unwrap_err().code(), "bad_session");
    }

    #[test]
    fn ssh_config_hosts_skip_wildcards_and_comments() {
        let cfg = "Host srv\n  HostName 10.0.0.1\nHost *\n  User x\nhost a b\n# Host c\nHOST dev-1 !nope\n";
        assert_eq!(parse_ssh_config(cfg), ["srv", "a", "b", "dev-1"]);
    }

    #[test]
    fn parses_session_list_and_marks_external() {
        assert_eq!(parse_sessions("work\nregente-abcdef12\n\n"), vec![("work".to_string(), true), ("regente-abcdef12".to_string(), false)]);
        assert!(parse_sessions("").is_empty());
    }

    #[test]
    fn find_is_guarded_and_uses_dereferenced_type() {
        let c = find_cmd("/home/u/p", "/home/u/p/src");
        assert!(c.starts_with("r=$(realpath -m -- '/home/u/p/src') && case \"$r\" in '/home/u/p'|'/home/u/p'/*) ;; *) exit 4;; esac"), "{c}");
        assert!(c.ends_with("find \"$r\" -mindepth 1 -maxdepth 1 -printf '%Y\\t%p\\n'"), "{c}");
        let out = "f\t/home/u/p/README.md\nd\t/home/u/p/src\nd\t/home/u/p/node_modules\nf\t/home/u/p/a.ts\n";
        let v = parse_find("srv", out);
        let names: Vec<_> = v.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, [("src", true), ("a.ts", false), ("README.md", false)]);
        assert_eq!(v[0].path, "ssh://srv/home/u/p/src");
    }

    #[test]
    fn read_requires_a_regular_file_inside_the_root() {
        let c = read_cmd("/home/u/p", "/home/u/p/a.ts");
        assert!(c.contains("[ -f \"$r\" ] || exit 5"), "{c}");
        assert!(c.contains("realpath -m --"));
    }

    #[test]
    fn exit_codes_map_to_clear_errors() {
        assert_eq!(status_error(Some(4), "").code(), "outside_workspace");
        assert_eq!(status_error(Some(5), "").code(), "not_file");
        assert_eq!(status_error(Some(1), "boom").code(), "remote_failed");
    }

    #[test]
    fn read_parses_header_and_keeps_local_errors() {
        let ok = parse_read(b"1700000000 5\nhello").unwrap();
        assert_eq!(ok, FileText { text: "hello".into(), mtime_ms: 1_700_000_000_000 });
        assert_eq!(parse_read(b"1 3\n\x00ab").unwrap_err().code(), "binary");
        assert_eq!(parse_read(b"1 3\n\xff\xfe\xfd").unwrap_err().code(), "encoding");
        let big = format!("1 {}\n", MAX_TEXT + 1);
        assert_eq!(parse_read(big.as_bytes()).unwrap_err().code(), "too_big");
        assert_eq!(parse_read(b"sem cabecalho").unwrap_err().code(), "remote_failed");
    }

    #[test]
    fn write_and_stat_mtime_in_ms() {
        assert_eq!(parse_mtime("1700000000\n").unwrap(), 1_700_000_000_000);
        assert!(parse_mtime("nope").is_err());
    }

    #[test]
    fn within_checks_alias_and_folder_boundary() {
        let roots = vec!["ssh://srv/home/u/proj".to_string()];
        let (ok, root) = within(&roots, "ssh://srv/home/u/proj/src/a.ts").unwrap();
        assert_eq!((ok.path.as_str(), root.as_str()), ("/home/u/proj/src/a.ts", "/home/u/proj"));
        assert!(within(&roots, "ssh://srv/home/u/proj").is_ok());
        assert_eq!(within(&roots, "ssh://srv/home/u/proj2/a").unwrap_err().code(), "outside_workspace");
        assert_eq!(within(&roots, "ssh://outro/home/u/proj/a").unwrap_err().code(), "outside_workspace");
        assert_eq!(within(&roots, "ssh://srv/home/u/proj/../x").unwrap_err().code(), "bad_remote");
    }

    #[test]
    fn session_lookup_and_tail_commands() {
        assert_eq!(session_find_cmd("abc-123").unwrap(), "ls -d ~/.claude/projects/*/abc-123.jsonl 2>/dev/null | head -n 1");
        assert!(session_find_cmd("a;rm").is_none());
        let t = tail_cmd("/x/a.jsonl", 10);
        assert!(t.contains("case \"$r\" in \"$HOME\"/.claude/projects/*) ;;"), "{t}");
        assert!(t.ends_with(&format!("tail -c +11 \"$r\" | head -c {}", 8u64 << 20)), "{t}");
    }

    #[test]
    fn tail_output_carries_the_size_first() {
        assert_eq!(parse_tail(b"120\nabc").unwrap(), (120, b"abc".to_vec()));
        assert!(parse_tail(b"sem nova linha").is_err());
    }

    #[test]
    fn cost_backoff_doubles_to_one_minute() {
        let secs: Vec<u64> = (1..=6).map(|f| cost_backoff(f).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 60, 60]);
    }

    #[test]
    fn write_script_is_atomic_and_checks_the_expected_mtime() {
        let w = write_cmd("/home/u/p", "/home/u/p/a.ts", Some(1_700_000_000));
        assert!(w.contains("[ \"$(stat -c %Y \"$f\" 2>/dev/null)\" = 1700000000 ] || exit 3;"), "{w}");
        assert!(w.contains("mktemp \"$f.XXXXXX\"") && w.contains("mv -f \"$t\" \"$f\""), "{w}");
        assert!(w.starts_with("r=$(realpath -m -- '/home/u/p/a.ts')"), "{w}");
        let free = write_cmd("/home/u/p", "/home/u/p/novo.ts", None);
        assert!(!free.contains("exit 3"), "sem mtime esperado não confere nada");
        assert_eq!(status_error(Some(3), "").code(), "changed_on_disk");
    }

    #[test]
    fn expected_mtime_is_sent_in_seconds() {
        // o editor lê o mtime em ms (segundos * 1000); o script confere em segundos
        let w = write_cmd("/p", "/p/a", Some(1_700_000_000_000 / 1000));
        assert!(w.contains("= 1700000000 ]"));
    }

    #[test]
    #[ignore = "exige ssh configurado para localhost; não roda aqui (não há host de teste)"]
    fn ssh_localhost() {
        match check("localhost", "/tmp", "claude") {
            Ok(_) => {}
            Err(e) => eprintln!("não disponível: {e:?}"),
        }
    }
}
