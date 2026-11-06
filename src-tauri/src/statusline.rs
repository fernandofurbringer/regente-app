use crate::error::AppResult;
use crate::util::{cmd, home_dir};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

/// Ferramentas de shell que os hooks Pre/PostToolUse acompanham (PowerShell também; o comando nunca é reescrito).
const SHELL_TOOLS: &str = "Bash|PowerShell";

pub fn format_line(input: &Value, effort: &str, branch: Option<&str>) -> String {
    let model = input["model"]["display_name"].as_str().or(input["model"]["id"].as_str()).unwrap_or("?");
    let mut parts = vec![model.to_string()];
    if !effort.is_empty() && effort != "-" {
        parts.push(effort.to_string());
    }
    if let Some(b) = branch.map(str::trim).filter(|b| !b.is_empty()) {
        parts.push(b.to_string());
    }
    parts.join(" · ")
}

/// The user's own statusline command (ignoring Regente's own, to avoid recursion).
pub fn user_command(claude_dir: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(claude_dir.join("settings.json")).ok()?).ok()?;
    let c = v["statusLine"]["command"].as_str()?;
    (!c.contains("--statusline")).then(|| c.to_string())
}

/// Opções do `claude-settings.json` de um painel. `Default` = statusline sem hooks e sem chaves extras.
#[derive(Debug, Clone, Default)]
pub struct PaneSettingsOpts {
    /// Esforço mostrado na statusline (sanitizado na escrita).
    pub effort: String,
    /// Também os eventos que `--hook` encaminha ao app.
    pub hooks: bool,
    /// Ferramentas que o Claude pode rodar sem perguntar (`mcp::pre_allowed`); vazio = sem `allow`.
    pub allow: Vec<String>,
    /// Também os eventos de shell (Pre/PostToolUse), para o painel Execuções (setting `execucoes`).
    pub exec_hooks: bool,
    /// Ferramentas negadas (`permissions.deny`), ex.: `Read(./node_modules/**)`.
    pub deny: Vec<String>,
    /// `outputStyle` do painel (ex.: "Concise" nos papéis curtos do Econômico).
    pub output_style: Option<String>,
    /// `idleCompaction: false`: a compactação ociosa fica com o Regente (perfil `contexto.nativaOciosa`).
    pub idle_compaction_off: bool,
    /// Resumo de saídas do perfil (`saidas.resumirAcima > 0`): PostToolUse de shell para o `--hook` trocar a saída.
    pub trim: bool,
    /// Digest de falha (`saidas.digestFalhas`): PostToolUseFailure de shell.
    pub digest_falhas: bool,
}

/// Writes the pane's `claude-settings.json` and returns its path (see `PaneSettingsOpts`).
pub fn write_settings(pane_dir: &Path, exe: &Path, opts: &PaneSettingsOpts) -> AppResult<PathBuf> {
    std::fs::create_dir_all(pane_dir)?;
    let exe = exe.to_string_lossy().replace('\\', "/");
    let effort = opts.effort.as_str();
    let effort = if !effort.is_empty() && effort.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { effort } else { "-" };
    let mut v = json!({ "statusLine": { "type": "command", "command": format!("\"{exe}\" --statusline --effort {effort}"), "padding": 0 } });
    let mut perms = serde_json::Map::new();
    if !opts.allow.is_empty() {
        perms.insert("allow".into(), json!(opts.allow));
    }
    if !opts.deny.is_empty() {
        perms.insert("deny".into(), json!(opts.deny));
    }
    if !perms.is_empty() {
        v["permissions"] = Value::Object(perms);
    }
    if let Some(style) = &opts.output_style {
        v["outputStyle"] = json!(style);
    }
    if opts.idle_compaction_off {
        v["idleCompaction"] = json!(false);
    }
    let (hooks, exec_hooks) = (opts.hooks, opts.exec_hooks);
    if hooks {
        // (evento, matcher): sem matcher, vale para todas as ferramentas. Novos eventos entram aqui.
        // Um evento pode ter vários grupos (um por matcher); a ordem dos grupos é a da lista.
        let mut events: Vec<(&str, Option<&str>)> = vec![
            ("UserPromptSubmit", None),
            ("Stop", None),
            ("Notification", None),
            ("SessionStart", None),
            // subagentes: um painel por agente (ver runtime::subagent_hook)
            ("SubagentStart", None),
            ("SubagentStop", None),
            // compactação: conta no painel e marca o contexto antes/depois (medidor)
            ("PreCompact", None),
            ("PostCompact", None),
            // a pergunta do AskUserQuestion: o painel fica "pergunta" entre o Pre e o Post
            ("PreToolUse", Some("AskUserQuestion")),
            ("PostToolUse", Some("AskUserQuestion")),
        ];
        if exec_hooks {
            // comandos de shell: o painel Execuções lê estes eventos; o comando nunca é reescrito aqui
            events.push(("PreToolUse", Some(SHELL_TOOLS)));
        }
        if exec_hooks || opts.trim {
            // um grupo só: o mesmo PostToolUse serve ao painel Execuções e ao resumo de saídas
            events.push(("PostToolUse", Some(SHELL_TOOLS)));
        }
        if exec_hooks || opts.digest_falhas {
            // comando que sai com erro (ou é interrompido) não dispara PostToolUse: dispara este
            events.push(("PostToolUseFailure", Some(SHELL_TOOLS)));
        }
        if exec_hooks {
            // TaskStop: a tarefa em segundo plano parou (fecha a execução); o campo `task_id` vem em tool_input
            events.push(("PostToolUse", Some("TaskStop")));
        }
        let mut h = serde_json::Map::new();
        for (ev, matcher) in events {
            let mut group = json!({"hooks": [{"type": "command", "command": format!("\"{exe}\" --hook {ev}")}]});
            if let Some(m) = matcher {
                group["matcher"] = json!(m);
            }
            h.entry(ev).or_insert_with(|| json!([])).as_array_mut().map(|list| list.push(group));
        }
        v["hooks"] = Value::Object(h);
    }
    let p = pane_dir.join("claude-settings.json");
    std::fs::write(&p, serde_json::to_string_pretty(&v)?)?;
    Ok(p)
}

/// Pastas pesadas que a IA não deve ler, por stack (`Read(...)` do `permissions.deny`). Nunca lock files, nunca a raiz.
/// `stacks`: "node" | "rust" | "python" (de `projeto.json.stacks` ou da detecção pelos arquivos da raiz).
pub fn read_denies(stacks: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut add = |rule: &str| {
        if !out.iter().any(|r| r == rule) {
            out.push(rule.to_string());
        }
    };
    for s in stacks {
        match s.as_str() {
            "node" => ["./node_modules/**", "./dist/**", "./build/**", "./.next/**"].iter().for_each(|p| add(&format!("Read({p})"))),
            "rust" => ["./target/**", "./src-tauri/target/**"].iter().for_each(|p| add(&format!("Read({p})"))),
            "python" => ["./.venv/**", "./**/__pycache__/**"].iter().for_each(|p| add(&format!("Read({p})"))),
            _ => {}
        }
    }
    for p in ["./**/*.map", "./**/*.min.js"] {
        add(&format!("Read({p})"));
    }
    out
}

/// Stacks de uma raiz: `stacks` de `.regente/projeto.json` ou, sem ele, o que os arquivos da raiz indicam.
pub fn detect_stacks(root: &Path) -> Vec<String> {
    let from_json = std::fs::read_to_string(root.join(".regente").join("projeto.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| {
        v["stacks"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect::<Vec<_>>())
    });
    if let Some(list) = from_json {
        return list;
    }
    let mut out = vec![];
    if root.join("package.json").is_file() {
        out.push("node".to_string());
    }
    if root.join("Cargo.toml").is_file() || root.join("src-tauri").join("Cargo.toml").is_file() {
        out.push("rust".to_string());
    }
    if root.join("pyproject.toml").is_file() || root.join("requirements.txt").is_file() {
        out.push("python".to_string());
    }
    out
}

/// Código de saída do `error` de um PostToolUseFailure: a primeira linha é `Exit code N`. None quando não há (ex.: o shell nem subiu).
pub fn exit_code_of(error: &str) -> Option<i32> {
    error.lines().next()?.strip_prefix("Exit code ")?.trim().parse().ok()
}

fn arg_after(args: &[String], flag: &str) -> String {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned().unwrap_or_default()
}

/// Entry point for `Regente.exe --statusline --effort X`. Returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    let input: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let cwd = input["workspace"]["current_dir"].as_str().or(input["cwd"].as_str()).unwrap_or(".").to_string();
    let branch = cmd("git").args(["-C", &cwd, "branch", "--show-current"]).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string());
    let mut line = format_line(&input, &arg_after(args, "--effort"), branch.as_deref());
    let claude_dir = std::env::var("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|_| home_dir().join(".claude"));
    if let Some(out) = user_command(&claude_dir).and_then(|u| run_user_command(&u, &raw, Duration::from_millis(1500))) {
        for l in out.lines().filter(|l| !l.trim().is_empty()) {
            line.push_str(&format!("\n{l}"));
        }
    }
    println!("{line}");
    // medidor de contexto: depois da linha impressa, em thread com prazo curto; falha não faz nada
    if let (Ok(url), Ok(token), Ok(pane)) = (std::env::var("REGENTE_HOOK_URL"), std::env::var("REGENTE_MCP_TOKEN"), std::env::var("REGENTE_PANE_ID")) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            manda_contexto(&input, &pane, &url, &token, |u, t, b| crate::hookcli::post(u, t, b, Duration::from_millis(300)));
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_millis(300));
    }
    0
}

/// Corpo do `/hook` com o uso de contexto da statusline. `current_usage` é null no início e logo após `/compact`.
pub fn corpo_contexto(pane: &str, input: &Value) -> Value {
    let cw = &input["context_window"];
    let u = &input["current_usage"];
    let ctx_tokens = u.is_object().then(|| ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"].iter().map(|k| u[*k].as_i64().unwrap_or(0)).sum::<i64>());
    json!({"event": "statusline", "pane": pane, "payload": {
        "ctxPct": cw["used_percentage"].clone(),
        "ctxTokens": ctx_tokens,
        "windowSize": cw["context_window_size"].clone(),
        "model": input["model"]["display_name"].as_str().or(input["model"]["id"].as_str()),
        "modelId": input["model"]["id"].clone(),
    }})
}

/// Monta o corpo e entrega pelo `envia` (injetado nos testes). Retorna se o envio foi feito.
pub fn manda_contexto(input: &Value, pane: &str, url: &str, token: &str, envia: impl FnOnce(&str, &str, &str) -> Option<()>) -> bool {
    let body = corpo_contexto(pane, input).to_string();
    envia(url, token, &body).is_some()
}

/// `...\Git\cmd\git.exe` -> `...\Git\bin\bash.exe`.
fn bash_from_git(git: &Path) -> Option<PathBuf> {
    Some(git.parent()?.parent()?.join("bin").join("bash.exe"))
}

fn git_bash() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let default = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    if default.is_file() {
        return Some(default);
    }
    crate::providers::which("git").and_then(|g| bash_from_git(Path::new(&g))).filter(|p| p.is_file())
}

/// Runs the user's statusline command (bash when available, else cmd) with `input` on stdin; stdout, or None on failure/timeout.
pub fn run_user_command(user: &str, input: &str, timeout: Duration) -> Option<String> {
    let mut c = if let Some(b) = git_bash() {
        let mut c = cmd(&b.to_string_lossy());
        c.args(["-c", user]);
        c
    } else {
        use std::os::windows::process::CommandExt;
        let mut c = cmd("cmd.exe");
        c.args(["/d", "/s", "/c"]).raw_arg(format!("\"{user}\""));
        c
    };
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut si = child.stdin.take()?;
    let mut so = child.stdout.take()?;
    let data = input.as_bytes().to_vec();
    std::thread::spawn(move || {
        let _ = si.write_all(&data);
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = so.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let res = rx.recv_timeout(timeout);
    // ponytail: grandchildren of a hung script may survive the kill; use a job object if that shows up
    let _ = child.kill();
    let _ = child.wait();
    res.ok().map(|b| String::from_utf8_lossy(&b).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bash_derived_from_git_path() {
        assert_eq!(bash_from_git(Path::new(r"D:\Tools\Git\cmd\git.exe")), Some(PathBuf::from(r"D:\Tools\Git\bin\bash.exe")));
        assert_eq!(bash_from_git(Path::new("git.exe")), None);
    }
    #[test]
    fn formats_parts() {
        let v = json!({"model": {"display_name": "Opus 5.5"}});
        assert_eq!(format_line(&v, "high", Some("main\n")), "Opus 5.5 · high · main");
        assert_eq!(format_line(&v, "-", None), "Opus 5.5");
        assert_eq!(format_line(&Value::Null, "", Some("")), "?");
    }
    #[test]
    fn read_denies_never_block_the_root_src_or_locks() {
        let all: Vec<String> = ["node", "rust", "python"].iter().map(|s| s.to_string()).collect();
        let rules = read_denies(&all);
        for bad in ["Read(./**)", "Read(./src/**)", "Read(.)", "Read(./)"] {
            assert!(!rules.iter().any(|r| r == bad), "{bad} na lista");
        }
        assert!(!rules.iter().any(|r| r.contains("lock")), "lock não pode ser negado: {rules:?}");
        assert!(rules.contains(&"Read(./node_modules/**)".to_string()) && rules.contains(&"Read(./target/**)".to_string()));
        assert_eq!(read_denies(&[]), ["Read(./**/*.map)", "Read(./**/*.min.js)"]);
    }

    #[test]
    fn stacks_come_from_the_files_at_the_root() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_stacks(d.path()), ["node"]);
        std::fs::create_dir_all(d.path().join("src-tauri")).unwrap();
        std::fs::write(d.path().join("src-tauri").join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_stacks(d.path()), ["node", "rust"]);
        std::fs::create_dir_all(d.path().join(".regente")).unwrap();
        std::fs::write(d.path().join(".regente").join("projeto.json"), r#"{"stacks": ["python"]}"#).unwrap();
        assert_eq!(detect_stacks(d.path()), ["python"]);
    }

    #[test]
    fn settings_carry_only_the_asked_keys() {
        let d = tempfile::tempdir().unwrap();
        let read = |p: PathBuf| -> Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
        // sem opção pedida, nenhuma chave do perfil aparece
        let off = read(write_settings(d.path(), Path::new("C:\\a.exe"), &PaneSettingsOpts::default()).unwrap());
        assert!(off.get("outputStyle").is_none() && off.get("idleCompaction").is_none() && off.get("permissions").is_none());
        let on = read(write_settings(d.path(), Path::new("C:\\a.exe"), &PaneSettingsOpts { output_style: Some("Concise".into()), idle_compaction_off: true, deny: vec!["Read(./dist/**)".into()], ..Default::default() }).unwrap());
        assert_eq!((on["outputStyle"].as_str(), on["idleCompaction"].clone()), (Some("Concise"), json!(false)));
        assert_eq!(on["permissions"], json!({"deny": ["Read(./dist/**)"]}));
    }
    #[test]
    fn settings_file_points_to_exe() {
        let d = tempfile::tempdir().unwrap();
        let p = write_settings(d.path(), Path::new("C:\\Apps\\Regente.exe"), &PaneSettingsOpts { effort: "max".into(), ..Default::default() }).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "\"C:/Apps/Regente.exe\" --statusline --effort max");
        assert!(v.get("hooks").is_none());
    }
    #[test]
    fn settings_allow_regente_tools_only_when_on() {
        let d = tempfile::tempdir().unwrap();
        let read = |p: PathBuf| -> Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
        let on = read(write_settings(d.path(), Path::new("C:\\a.exe"), &PaneSettingsOpts { allow: vec!["mcp__regente__pane_read".to_string()], ..Default::default() }).unwrap());
        assert_eq!(on["permissions"]["allow"], json!(["mcp__regente__pane_read"]));
        let off = read(write_settings(d.path(), Path::new("C:\\a.exe"), &PaneSettingsOpts::default()).unwrap());
        assert!(off.get("permissions").is_none());
    }
    #[test]
    fn settings_with_hooks() {
        let d = tempfile::tempdir().unwrap();
        let p = write_settings(d.path(), Path::new("C:\\Apps\\Regente.exe"), &PaneSettingsOpts { effort: "max".into(), hooks: true, ..Default::default() }).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        for ev in ["UserPromptSubmit", "Stop", "Notification", "SessionStart", "SubagentStart", "SubagentStop", "PreToolUse", "PostToolUse"] {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(cmd, format!("\"C:/Apps/Regente.exe\" --hook {ev}"));
        }
        // a pergunta do AskUserQuestion é só dessas ferramentas; os outros eventos valem para todas
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "AskUserQuestion");
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "AskUserQuestion");
        assert!(v["hooks"]["Stop"][0].get("matcher").is_none());
        assert!(v["hooks"]["SubagentStart"][0].get("matcher").is_none());
        assert!(v["hooks"]["SubagentStop"][0].get("matcher").is_none());
        // sem execucoes: nenhum grupo de shell
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert!(v["hooks"].get("PostToolUseFailure").is_none());
    }

    #[test]
    fn exec_hooks_add_shell_groups_after_the_question_group() {
        let d = tempfile::tempdir().unwrap();
        let p = write_settings(d.path(), Path::new("C:\\Apps\\Regente.exe"), &PaneSettingsOpts { hooks: true, exec_hooks: true, ..Default::default() }).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        for ev in ["PreToolUse", "PostToolUse"] {
            let groups = v["hooks"][ev].as_array().unwrap();
            assert_eq!(groups[0]["matcher"], "AskUserQuestion");
            // o matcher pega Bash e PowerShell; o mesmo comando --hook, sem permissionDecision nem reescrita
            assert_eq!(groups[1]["matcher"], "Bash|PowerShell");
            assert_eq!(groups[1]["hooks"][0]["command"], format!("\"C:/Apps/Regente.exe\" --hook {ev}"));
            if ev == "PostToolUse" {
                // TaskStop: só o PostToolUse (a parada da tarefa fecha a execução); o PreToolUse não é usado
                assert_eq!(groups.len(), 3);
                assert_eq!(groups[2]["matcher"], "TaskStop");
                assert_eq!(groups[2]["hooks"][0]["command"], format!("\"C:/Apps/Regente.exe\" --hook {ev}"));
            } else {
                assert_eq!(groups.len(), 2);
            }
        }
        // a falha de shell não tem grupo de pergunta: só o grupo de shell
        let failure = v["hooks"]["PostToolUseFailure"].as_array().unwrap();
        assert_eq!(failure.len(), 1);
        assert_eq!(failure[0]["matcher"], "Bash|PowerShell");
        assert_eq!(failure[0]["hooks"][0]["command"], "\"C:/Apps/Regente.exe\" --hook PostToolUseFailure");
    }
    #[test]
    fn exit_code_is_read_from_the_first_line() {
        assert_eq!(exit_code_of("Exit code 1\nboom"), Some(1));
        assert_eq!(exit_code_of("Exit code -1073741510"), Some(-1073741510));
        assert_eq!(exit_code_of("Exit code 2\n... [900 characters truncated] ..."), Some(2));
        assert_eq!(exit_code_of("erro sem código"), None);
        assert_eq!(exit_code_of("saída\nExit code 3"), None);
        assert_eq!(exit_code_of(""), None);
    }
    #[test]
    fn effort_is_sanitized() {
        let d = tempfile::tempdir().unwrap();
        let p = write_settings(d.path(), Path::new("C:\\a.exe"), &PaneSettingsOpts { effort: "x\" & calc".into(), ..Default::default() }).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "\"C:/a.exe\" --statusline --effort -");
    }
    #[test]
    fn context_body_has_the_meter_fields_and_nulls_when_no_usage() {
        let input = json!({"model": {"display_name": "Opus 5.5", "id": "claude-opus-5-5"}, "context_window": {"used_percentage": 62.5, "context_window_size": 200000},
            "current_usage": {"input_tokens": 100, "cache_creation_input_tokens": 20, "cache_read_input_tokens": 3}});
        let b = corpo_contexto("P1", &input);
        assert_eq!((b["event"].as_str(), b["pane"].as_str()), (Some("statusline"), Some("P1")));
        assert_eq!(b["payload"], json!({"ctxPct": 62.5, "ctxTokens": 123, "windowSize": 200000, "model": "Opus 5.5", "modelId": "claude-opus-5-5"}));
        // início da sessão e depois do /compact: current_usage é null
        let vazio = corpo_contexto("P1", &json!({"context_window": {"used_percentage": null}, "current_usage": null}));
        assert_eq!(vazio["payload"]["ctxTokens"], Value::Null);
        assert_eq!(vazio["payload"]["ctxPct"], Value::Null);
    }

    #[test]
    fn context_sender_gets_url_token_and_body() {
        let mut got = None;
        let ok = manda_contexto(&json!({"context_window": {"used_percentage": 10}}), "P1", "http://127.0.0.1:1/hook", "tk", |u, t, b| {
            got = Some((u.to_string(), t.to_string(), b.to_string()));
            Some(())
        });
        let (u, t, b) = got.unwrap();
        assert!(ok);
        assert_eq!((u.as_str(), t.as_str()), ("http://127.0.0.1:1/hook", "tk"));
        assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["payload"]["ctxPct"], 10);
    }

    #[test]
    fn hung_user_command_times_out() {
        let t = std::time::Instant::now();
        let _ = run_user_command("ping -n 5 127.0.0.1", "{}", Duration::from_millis(1500));
        assert!(t.elapsed() < Duration::from_millis(2500), "{:?}", t.elapsed());
    }
    #[test]
    fn user_command_output() {
        assert_eq!(run_user_command("echo hello", "{}", Duration::from_secs(5)).unwrap().trim(), "hello");
    }
    #[test]
    fn user_command_skips_recursion() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("settings.json"), r#"{"statusLine":{"command":"x --statusline"}}"#).unwrap();
        assert_eq!(user_command(d.path()), None);
        std::fs::write(d.path().join("settings.json"), r#"{"statusLine":{"command":"node sl.js"}}"#).unwrap();
        assert_eq!(user_command(d.path()).as_deref(), Some("node sl.js"));
    }
}
