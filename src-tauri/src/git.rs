use crate::error::{AppError, AppResult};
use crate::util::cmd;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn git(dir: &Path, args: &[&str]) -> AppResult<String> {
    git_env(dir, args, &[])
}

/// Limite de cada chamada local do git: um hook ou um repositório travado não prende a thread para sempre.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// `git` with extra environment variables (e.g. `GIT_INDEX_FILE`); same limits and errors as `git`, killed after `GIT_TIMEOUT`.
pub fn git_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> AppResult<String> {
    let mut c = cmd("git");
    c.arg("-C").arg(dir).args(args).envs(env.iter().copied());
    run_limited(c, args, GIT_TIMEOUT)
}

/// Like `git`, but the stdout bytes as they are (blobs can be binary; `git` trims and decodes them). Same timeout.
pub fn git_bytes(dir: &Path, args: &[&str]) -> AppResult<Vec<u8>> {
    let mut c = cmd("git");
    c.arg("-C").arg(dir).args(args);
    run_limited_bytes(c, args, GIT_TIMEOUT)
}

pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--is-inside-work-tree"]).map(|s| s == "true").unwrap_or(false)
}

pub fn current_branch(dir: &Path) -> AppResult<String> {
    git(dir, &["branch", "--show-current"])
}

/// Branch atual lida direto do arquivo HEAD, sem processo git (meio milissegundo contra ~50 ms): sobe a partir de `dir`
/// até achar o `.git` (pasta, ou arquivo `gitdir:` de worktree). `Some("")` = HEAD destacado; None = fora de
/// repositório, HEAD ilegível ou formato que só o git entende (reftable) — aí quem chama pergunta ao git.
pub fn head_branch(dir: &Path) -> Option<String> {
    let mut at = Some(dir);
    while let Some(p) = at {
        let g = p.join(".git");
        if g.is_dir() {
            return read_head(&g.join("HEAD"));
        }
        if g.is_file() {
            let s = std::fs::read_to_string(&g).ok()?;
            let gitdir = p.join(s.trim().strip_prefix("gitdir:")?.trim());
            return read_head(&gitdir.join("HEAD"));
        }
        at = p.parent();
    }
    None
}

fn read_head(f: &Path) -> Option<String> {
    let s = std::fs::read_to_string(f).ok()?;
    let s = s.trim();
    match s.strip_prefix("ref: refs/heads/") {
        // reftable guarda o HEAD de verdade fora do arquivo
        Some(".invalid") => None,
        Some(b) => Some(b.to_string()),
        None if s.starts_with("ref:") => None,
        None => Some(String::new()),
    }
}

/// `head_branch`, ou o git quando o arquivo não basta.
pub fn branch_fast(dir: &Path) -> AppResult<String> {
    match head_branch(dir) {
        Some(b) => Ok(b),
        None => current_branch(dir),
    }
}

pub fn branch_exists(dir: &Path, branch: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok()
}

/// Stderr lines that mean git wanted a credential it cannot ask for (no terminal, no dialog).
pub fn is_auth_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    ["could not read username", "could not read password", "authentication failed", "terminal prompts disabled", "permission denied", "logon failed", "403"]
        .iter()
        .any(|k| lower.contains(k))
}

/// Runs `command` with piped output and kills it after `timeout`. Returns stdout; a credential request is `git_auth`.
pub fn run_limited(command: Command, args: &[&str], timeout: Duration) -> AppResult<String> {
    run_limited_bytes(command, args, timeout).map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

/// `run_limited` with the stdout bytes as they are.
pub fn run_limited_bytes(mut command: Command, args: &[&str], timeout: Duration) -> AppResult<Vec<u8>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(r) = out.as_mut() {
            let _ = r.read_to_end(&mut b);
        }
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(r) = err.as_mut() {
            let _ = r.read_to_end(&mut b);
        }
        b
    });
    let t0 = Instant::now();
    // espera curta no começo (a maioria das chamadas leva 20-60 ms) e cresce até 50 ms: o fixo de 50 ms somava até
    // 50 ms em cada git do app, e o rodapé faz uma dúzia em série
    let mut pausa = Duration::from_millis(1);
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if t0.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::new("git_timeout", format!("git {} passou de {} s e foi interrompido", args.join(" "), timeout.as_secs())));
        }
        std::thread::sleep(pausa);
        pausa = (pausa * 2).min(Duration::from_millis(50));
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).trim().to_string();
    if !status.success() {
        if is_auth_error(&stderr) {
            return Err(AppError::new("git_auth", "o git pediu credenciais; faça login no terminal (gh auth login / git credential)"));
        }
        return Err(AppError::new("git_failed", format!("git {}: {stderr}", args.join(" "))));
    }
    Ok(stdout)
}

/// `git -C dir args` for network operations: no prompts (terminal or credential dialog), killed after `timeout`.
pub fn git_remote(dir: &Path, args: &[&str], timeout: Duration) -> AppResult<String> {
    let mut c = cmd("git");
    c.arg("-C").arg(dir).args(args).env("GIT_TERMINAL_PROMPT", "0").env("GCM_INTERACTIVE", "never");
    run_limited(c, args, timeout)
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn head_lido_do_arquivo_no_repo_na_subpasta_no_worktree_e_destacado() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        git(r, &["init", "-b", "main"]).unwrap();
        git(r, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--allow-empty", "-m", "a"]).unwrap();
        assert_eq!(head_branch(r).as_deref(), Some("main"));
        let sub = r.join("pasta").join("funda");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(head_branch(&sub).as_deref(), Some("main"), "workspace numa subpasta do repositório");
        let wt = r.join("wt");
        git(r, &["worktree", "add", "-b", "dev", &wt.to_string_lossy()]).unwrap();
        assert_eq!(head_branch(&wt).as_deref(), Some("dev"));
        git(&wt, &["checkout", "--detach"]).unwrap();
        assert_eq!(head_branch(&wt).as_deref(), Some(""));
        assert_eq!(branch_fast(&wt).unwrap(), current_branch(&wt).unwrap());
        assert_eq!(head_branch(r).as_deref(), Some("main"), "o worktree não muda o HEAD do principal");
        let fora = tempfile::tempdir().unwrap();
        assert_eq!(head_branch(fora.path()), None);
    }

    #[test]
    fn credential_requests_are_recognised() {
        assert!(is_auth_error("fatal: could not read Username for 'https://github.com': terminal prompts disabled"));
        assert!(is_auth_error("remote: Permission denied"));
        assert!(!is_auth_error("! [rejected] main -> main (non-fast-forward)"));
    }

    #[test]
    fn a_quick_command_returns_its_output() {
        let mut c = cmd("cmd");
        c.args(["/c", "echo", "ola"]);
        assert_eq!(run_limited(c, &["echo"], Duration::from_secs(10)).unwrap(), "ola");
    }

    #[test]
    fn git_env_is_killed_at_its_timeout() {
        // alias de shell: o git fica esperando o `sleep`; o limite tem que cortar a espera
        let d = tempfile::tempdir().unwrap();
        git(d.path(), &["init", "-q"]).unwrap();
        let mut c = cmd("git");
        c.arg("-C").arg(d.path()).args(["-c", "alias.lento=!sleep 30", "lento"]);
        let t0 = Instant::now();
        assert_eq!(run_limited(c, &["lento"], Duration::from_millis(300)).unwrap_err().code(), "git_timeout");
        assert!(t0.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_slow_command_is_killed_at_the_timeout() {
        let mut c = cmd("ping");
        c.args(["-n", "20", "127.0.0.1"]);
        let t0 = Instant::now();
        assert_eq!(run_limited(c, &["ping"], Duration::from_millis(300)).unwrap_err().code(), "git_timeout");
        assert!(t0.elapsed() < Duration::from_secs(10));
    }
}
