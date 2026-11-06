use crate::error::{fail, AppResult};
use crate::orchestrator::bracketed;
use crate::pty::SpawnPlan;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionArgs {
    pub danger: Vec<String>,
    pub auto: Vec<String>,
    pub pend: Vec<String>,
    pub plan: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CliDef {
    pub id: String,
    pub label: String,
    pub command: String,
    pub install: Option<String>,
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    pub permission: PermissionArgs,
    pub model_args: Vec<String>,
    pub effort_args: Vec<String>,
    pub session_new: Vec<String>,
    pub session_resume: Vec<String>,
    pub resume_first: bool,
    pub account_env: Option<String>,
    pub cost: Option<String>,
    pub statusline: bool,
    pub routable: bool,
    /// Initial prompt as a positional/flag argument (`{prompt}`); empty = paste it instead.
    pub prompt_args: Vec<String>,
    /// `--append-system-prompt {system}`; empty = the system text is prepended to the prompt.
    pub system_args: Vec<String>,
    /// `--disallowedTools {tools}`.
    pub disallow_args: Vec<String>,
    /// Which MCP config format the CLI reads: "claude" | "codex" | "gemini".
    pub mcp: Option<String>,
    /// `--plugin-dir {dir}`: the pane's skills as a plugin; empty = the skills go into the system prompt.
    pub plugin_args: Vec<String>,
    /// Env vars that switch off the CLI's own self-update in panes (`DISABLE_AUTOUPDATER=1` for Claude Code).
    pub no_update_env: Vec<(String, String)>,
    /// Pacote npm público da IA (`npm view <pkg> version`); None = sem fonte de versão.
    pub npm_package: Option<String>,
    /// Comando que atualiza a IA; None = usa `install`.
    pub update_cmd: Option<String>,
    /// Chave lógica do perfil de trabalho → variável de ambiente da CLI (ver `perfil::aplicado`).
    pub perfil_env: BTreeMap<String, String>,
    /// Chave lógica do perfil → argumentos da CLI; `{n}` vira o valor do perfil.
    pub perfil_args: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct RouteDef {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub secret: Option<String>,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ShellDef {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProvidersConfig {
    pub clis: Vec<CliDef>,
    pub routes: Vec<RouteDef>,
    pub shells: Vec<ShellDef>,
    /// Padrões já mesclados (`route:<id>`, `install:<cli>`); só o servidor escreve (ver `settings::merge_new_fields`).
    pub seen_defaults: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Danger,
    Auto,
    #[default]
    Pend,
    Plan,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Danger => "danger",
            Self::Auto => "auto",
            Self::Pend => "pend",
            Self::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PaneKind {
    #[default]
    Agent,
    Terminal,
    Browser,
    Image,
    /// Local video file or https URL (the pane plays it; no process).
    Video,
    /// Subagente do Claude: conversa somente leitura, tocada pelo transcrito do subagente (sem processo).
    Subagent,
    /// Comando de shell promovido a painel próprio (saída ao vivo, só leitura; sem processo). Ver `execucoes.rs`.
    Exec,
    /// Lista de execuções de shell de um painel do Claude (sem processo).
    Execucoes,
}

impl PaneKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Terminal => "terminal",
            Self::Browser => "browser",
            Self::Image => "image",
            Self::Video => "video",
            Self::Subagent => "subagent",
            Self::Exec => "exec",
            Self::Execucoes => "execucoes",
        }
    }
}

/// What the UI (or the mission) asks for when opening a pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PaneSpec {
    pub workspace_id: String,
    pub mission_id: Option<String>,
    pub kind: PaneKind,
    pub name: Option<String>,
    pub cli: Option<String>,
    pub shell: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub permission: Permission,
    pub effort: Option<String>,
    pub route: Option<String>,
    pub cwd: String,
    pub url_or_path: Option<String>,
    /// Terminal: command to run when the shell opens (e.g. a CLI installer).
    pub run: Option<String>,
    /// Initial prompt, delivered once the CLI is up (by file reference or bracketed paste).
    pub prompt: Option<String>,
    pub parent_id: Option<String>,
    /// Subagente do Claude (hook SubagentStart); só o painel `subagent` usa.
    pub agent_id: Option<String>,
    pub role: Option<String>,
    pub agent: Option<String>,
    pub task_type: Option<String>,
    /// "full" | "minimal"; None = full.
    pub mcp_profile: Option<String>,
    /// None = unrestricted, Some([]) = no skills.
    pub allowed_skills: Option<Vec<String>>,
    /// Minimal profile: extra MCP servers to keep (reported in the receipt only for now).
    pub mcp_servers: Option<Vec<String>>,
    /// Appended to the system prompt (or prefixed to the prompt when the CLI has no such flag).
    pub append_system: Option<String>,
    pub disallowed_tools: Option<Vec<String>>,
    /// Memória do painel: "auto" (padrão), "standalone" (só a do workspace) ou "off". Ver `memory::resolve_mode`.
    pub memory_mode: Option<String>,
    /// Terminal em workspace remoto: nome de uma sessão tmux externa a anexar (nunca encerrada pelo Regente).
    pub attach_session: Option<String>,
    /// Perfil de trabalho do painel (id de `perfis`); None = o padrão. Gravado na abertura e reusado no respawn.
    pub profile: Option<String>,
    /// Session of a helper to resume (`delegate`). Internal only: the UI and the MCP never set it.
    #[serde(skip)]
    pub resume_session: Option<String>,
}

pub struct BuildCtx<'a> {
    /// Resolved executable (where.exe); None = use `command` as is.
    pub program_path: Option<&'a str>,
    pub accounts_dir: &'a Path,
    pub route_secret: Option<String>,
    /// New session id (used by CLIs with `sessionNew`).
    pub new_session: Option<String>,
    /// Session to resume (hibernation/restore).
    pub resume: Option<String>,
    /// `--settings` file with the injected statusline.
    pub settings_file: Option<PathBuf>,
    /// Where the full prompt was written (`panes/<id>/prompt.md`).
    pub prompt_file: Option<PathBuf>,
    /// Extra flags (MCP config), inserted before the positional prompt.
    pub extra_args: Vec<String>,
    /// Extra environment variables (MCP token, hook url, ...).
    pub extra_env: Vec<(String, String)>,
    /// Plugin folder of the pane's skills (`{dir}` in `pluginArgs`).
    pub plugin_dir: Option<&'a Path>,
    /// Variáveis e argumentos do perfil de trabalho para esta CLI (ver `perfil::aplicado`).
    pub perfil_env: Vec<(String, String)>,
    pub perfil_args: Vec<String>,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn subst(t: &[String], model: &str, effort: &str, session: &str) -> Vec<String> {
    t.iter().map(|a| a.replace("{model}", model).replace("{effort}", effort).replace("{session}", session)).collect()
}

/// Replaces the `{name}` placeholders of a CLI argument template.
fn fill(t: &[String], pairs: &[(&str, &str)]) -> Vec<String> {
    t.iter().map(|a| pairs.iter().fold(a.clone(), |s, &(k, v)| s.replace(k, v))).collect()
}

/// `.cmd`/`.bat` npm shims: the command line goes through cmd.exe, so the prompt cannot be an argument.
pub fn is_batch(program: &str) -> bool {
    let low = program.to_lowercase();
    low.ends_with(".cmd") || low.ends_with(".bat")
}

/// System text prepended to a prompt, for CLIs without a system-prompt flag.
pub fn with_system(sys: &str, prompt: &str) -> String {
    format!("{sys}

---

{prompt}")
}

fn needs_quotes(t: &str) -> bool {
    t.is_empty() || t.chars().any(|c| matches!(c, ' ' | '\t' | '&' | '|' | '<' | '>' | '^' | '(' | ')' | '%' | '!'))
}

/// MSVC-style quoting of one token (it never contains `"`, rejected earlier).
fn quote(t: &str) -> String {
    if !needs_quotes(t) {
        return t.to_string();
    }
    let trailing = t.len() - t.trim_end_matches('\\').len();
    format!("\"{t}{}\"", "\\".repeat(trailing))
}

/// `.cmd`/`.bat` (npm shims) go through cmd.exe, with the command line in an env var so that no
/// argument is ever parsed by cmd (BatBadBut): `cmd /d /v:off /c %REGENTE_CMDLINE%`.
pub fn wrap_batch(program: String, args: Vec<String>, env: &mut Vec<(String, String)>) -> AppResult<(String, Vec<String>)> {
    if !is_batch(&program) {
        return Ok((program, args));
    }
    if std::iter::once(&program).chain(args.iter()).any(|a| a.contains(['"', '\r', '\n'])) {
        return fail("bad_arg", "argumento inválido para script .cmd/.bat");
    }
    let line = std::iter::once(&program).chain(args.iter()).map(|a| quote(a)).collect::<Vec<_>>().join(" ");
    env.push(("REGENTE_CMDLINE".into(), line));
    // ponytail: an npm shim's bare `node` lookup can hit a `node` planted in the cwd; NoDefaultCurrentDirectoryInExePath would be inherited by the whole agent tree and break gradlew/mvnw/make.bat.
    Ok(("cmd.exe".into(), vec!["/d".into(), "/v:off".into(), "/c".into(), "%REGENTE_CMDLINE%".into()]))
}

/// Extra account name (it becomes a folder): letters, digits, `-`, `_`; not "principal" nor a Windows device name.
pub fn valid_account(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    let device = matches!(low.as_str(), "con" | "prn" | "aux" | "nul")
        || ((low.starts_with("com") || low.starts_with("lpt")) && low.len() == 4 && low.as_bytes()[3].is_ascii_digit());
    !name.is_empty() && low != "principal" && !device && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn build_spawn(cfg: &ProvidersConfig, spec: &PaneSpec, ctx: &BuildCtx) -> AppResult<SpawnPlan> {
    match spec.kind {
        PaneKind::Browser | PaneKind::Image | PaneKind::Video | PaneKind::Subagent | PaneKind::Exec | PaneKind::Execucoes => fail("not_a_process", "painel sem processo"),
        PaneKind::Terminal => {
            let sid = nonempty(&spec.shell).unwrap_or("powershell");
            let Some(sh) = cfg.shells.iter().find(|s| s.id == sid) else { return fail("unknown_shell", format!("shell {sid} desconhecido")) };
            let mut args = sh.args.clone();
            if let Some(run) = nonempty(&spec.run) {
                match sh.id.as_str() {
                    "powershell" | "pwsh" => args.extend(["-NoExit".into(), "-Command".into(), run.into()]),
                    "cmd" => args.extend(["/k".into(), run.into()]),
                    "gitbash" => args.extend(["-c".into(), format!("{run}; exec bash -i")]),
                    other => return fail("run_unsupported", format!("shell {other} não suporta executar comando ao abrir")),
                }
            }
            let mut env = Vec::new();
            let (program, args) = wrap_batch(ctx.program_path.unwrap_or(&sh.command).to_string(), args, &mut env)?;
            Ok(SpawnPlan { program, args, env, cwd: spec.cwd.clone(), ..Default::default() })
        }
        PaneKind::Agent => {
            let Some(cid) = nonempty(&spec.cli) else { return fail("missing_cli", "escolha uma IA") };
            let Some(cli) = cfg.clis.iter().find(|c| c.id == cid) else { return fail("unknown_cli", format!("IA {cid} desconhecida")) };
            let model = nonempty(&spec.model).unwrap_or("");
            let effort = nonempty(&spec.effort).unwrap_or("");
            let mut args = Vec::new();
            let mut session_id = None;
            if let (Some(r), true) = (&ctx.resume, cli.resume_first && !cli.session_resume.is_empty()) {
                args.extend(subst(&cli.session_resume, model, effort, r));
                session_id = Some(r.clone());
            }
            let perm = match spec.permission {
                Permission::Danger => &cli.permission.danger,
                Permission::Auto => &cli.permission.auto,
                Permission::Pend => &cli.permission.pend,
                Permission::Plan => &cli.permission.plan,
            };
            args.extend(perm.iter().cloned());
            if !model.is_empty() {
                args.extend(subst(&cli.model_args, model, effort, ""));
            }
            if !effort.is_empty() {
                args.extend(subst(&cli.effort_args, model, effort, ""));
            }
            match (&ctx.resume, cli.resume_first) {
                (Some(r), false) if !cli.session_resume.is_empty() => {
                    args.extend(subst(&cli.session_resume, model, effort, r));
                    session_id = Some(r.clone());
                }
                (None, _) if !cli.session_new.is_empty() => {
                    if let Some(s) = &ctx.new_session {
                        args.extend(subst(&cli.session_new, model, effort, s));
                        session_id = Some(s.clone());
                    }
                }
                _ => {}
            }
            if cli.statusline {
                if let Some(f) = &ctx.settings_file {
                    args.extend(["--settings".into(), f.to_string_lossy().to_string()]);
                }
            }
            let mut env = ctx.extra_env.clone();
            env.extend(cli.no_update_env.iter().cloned());
            env.extend(ctx.perfil_env.iter().cloned());
            if let (Some(acc), Some(var)) = (nonempty(&spec.account), &cli.account_env) {
                if acc != "principal" {
                    if !valid_account(acc) {
                        return fail("bad_account", format!("conta inválida: {acc}"));
                    }
                    env.push((var.clone(), ctx.accounts_dir.join(&cli.id).join(acc).to_string_lossy().to_string()));
                }
            }
            let mut route_label = None;
            if let Some(rid) = nonempty(&spec.route) {
                if !cli.routable {
                    return fail("route_unsupported", format!("{} não aceita rota", cli.label));
                }
                let Some(r) = cfg.routes.iter().find(|r| r.id == rid) else { return fail("unknown_route", format!("rota {rid} desconhecida")) };
                env.push(("ANTHROPIC_BASE_URL".into(), r.base_url.clone()));
                env.push(("ANTHROPIC_AUTH_TOKEN".into(), ctx.route_secret.clone().unwrap_or_else(|| "regente-local".into())));
                env.push(("ANTHROPIC_API_KEY".into(), String::new()));
                if !model.is_empty() {
                    for k in ["ANTHROPIC_MODEL", "ANTHROPIC_SMALL_FAST_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL"] {
                        env.push((k.into(), model.into()));
                    }
                }
                route_label = Some(format!("{} ← {}", cli.id.to_uppercase(), r.label.to_uppercase()));
            }
            let program = ctx.program_path.unwrap_or(&cli.command);
            let batch = is_batch(program);
            if let Some(tools) = spec.disallowed_tools.as_ref().filter(|t| !t.is_empty()) {
                args.extend(fill(&cli.disallow_args, &[("{tools}", &tools.join(","))]));
            }
            let sys = nonempty(&spec.append_system);
            let sys_in_args = sys.filter(|_| !cli.system_args.is_empty() && !batch);
            if let Some(s) = sys_in_args {
                args.extend(fill(&cli.system_args, &[("{system}", s)]));
            }
            args.extend(ctx.extra_args.iter().cloned());
            args.extend(ctx.perfil_args.iter().cloned());
            if let Some(dir) = ctx.plugin_dir {
                args.extend(fill(&cli.plugin_args, &[("{dir}", &dir.to_string_lossy())]));
            }
            // positional prompt goes last, after every flag
            let prompt = spec.prompt.as_deref().filter(|p| !p.trim().is_empty());
            let mut initial_input = None;
            match (prompt, ctx.prompt_file.as_ref()) {
                (Some(_), Some(file)) if !cli.prompt_args.is_empty() && !batch => {
                    let msg = format!("Siga as instruções do arquivo \"{}\"", file.to_string_lossy());
                    args.extend(fill(&cli.prompt_args, &[("{prompt}", &msg)]));
                }
                _ => {
                    let sys_paste = if sys_in_args.is_some() { None } else { sys };
                    let text = match (prompt, sys_paste) {
                        (Some(p), Some(s)) => with_system(s, p),
                        (Some(p), None) => p.to_string(),
                        (None, Some(s)) => s.to_string(),
                        (None, None) => String::new(),
                    };
                    if !text.is_empty() {
                        initial_input = Some(bracketed(&text));
                    }
                }
            }
            let (program, args) = wrap_batch(program.to_string(), args, &mut env)?;
            Ok(SpawnPlan { program, args, env, cwd: spec.cwd.clone(), session_id, route_label, initial_input })
        }
    }
}

/// Resolves an executable on PATH (never the cwd): an `.exe` in any dir beats a `.cmd`/`.bat` shim.
pub fn which_in(command: &str, path_var: &OsStr) -> Option<String> {
    let p = Path::new(command);
    if p.is_absolute() {
        return p.is_file().then(|| command.to_string());
    }
    if command.contains(['/', '\\']) {
        return None;
    }
    let exts: &[&str] = if p.extension().is_some() { &[""] } else { &[".exe", ".cmd", ".bat", ".com"] };
    for ext in exts {
        for dir in std::env::split_paths(path_var).filter(|d| d.is_absolute()) {
            let c = dir.join(format!("{command}{ext}"));
            if c.is_file() {
                return Some(c.to_string_lossy().to_string());
            }
        }
    }
    None
}

pub fn which(command: &str) -> Option<String> {
    which_in(command, &std::env::var_os("PATH")?)
}

/// Binário de um CLI pelo nome do comando. Sem `find` achando `command`, o Cursor é o único alias: o binário
/// foi publicado como `cursor-agent` e como `agent`, e `agent` é nome genérico, então só vale se `is_cursor`
/// confirmar que é o do Cursor. Recebe as duas operações para o teste usar um runner falso.
pub fn resolve_cli_with(command: &str, find: impl Fn(&str) -> Option<String>, is_cursor: impl Fn(&str) -> bool) -> Option<String> {
    if let Some(p) = find(command) {
        return Some(p);
    }
    // ponytail: o único alias conhecido é o do Cursor; tabela de aliases quando outro CLI precisar
    if command != "cursor-agent" {
        return None;
    }
    find("agent").filter(|p| is_cursor(p.as_str()))
}

/// Sonda: o binário diz "cursor" no `--version` ou no `--help`? (roda o programa; 10 s por chamada)
pub fn is_cursor_binary(path: &str) -> bool {
    ["--version", "--help"]
        .iter()
        .any(|a| crate::cli_version::capture(path, &[a], std::time::Duration::from_secs(10)).is_some_and(|o| o.to_lowercase().contains("cursor")))
}

/// Caminho real do CLI, sem cache (ver `resolve_cli_with`). O runtime usa o próprio cache por cima.
pub fn resolve_cli(command: &str) -> Option<String> {
    resolve_cli_with(command, which, is_cursor_binary)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub id: String,
    pub kind: String, // cli | shell
    pub path: Option<String>,
}

pub fn detect(cfg: &ProvidersConfig) -> Vec<Detected> {
    let clis = cfg.clis.iter().map(|c| Detected { id: c.id.clone(), kind: "cli".into(), path: resolve_cli(&c.command) });
    let shells = cfg.shells.iter().map(|s| Detected { id: s.id.clone(), kind: "shell".into(), path: which(&s.command) });
    clis.chain(shells).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PROVIDERS_DEFAULT;

    fn cfg() -> ProvidersConfig {
        serde_json::from_str(PROVIDERS_DEFAULT).unwrap()
    }
    fn ctx(accounts: &Path) -> BuildCtx<'_> {
        BuildCtx { program_path: None, accounts_dir: accounts, route_secret: None, new_session: Some("S1".into()), resume: None, settings_file: Some(PathBuf::from("C:/r/s.json")), prompt_file: None, extra_args: vec![], extra_env: vec![], plugin_dir: None, perfil_env: vec![], perfil_args: vec![] }
    }
    fn agent(cli: &str) -> PaneSpec {
        PaneSpec { workspace_id: "w".into(), cli: Some(cli.into()), cwd: "D:/p".into(), ..Default::default() }
    }

    #[test]
    fn claude_new_session_full() {
        let mut s = agent("claude");
        s.model = Some("claude-opus-5-5".into());
        s.effort = Some("high".into());
        s.permission = Permission::Danger;
        let p = build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap();
        assert_eq!(p.program, "claude");
        assert_eq!(p.args, ["--dangerously-skip-permissions", "--model", "claude-opus-5-5", "--effort", "high", "--session-id", "S1", "--settings", "C:/r/s.json"]);
        assert_eq!(p.session_id.as_deref(), Some("S1"));
        assert_eq!(p.env, vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())]);
    }

    #[test]
    fn claude_resume_replaces_new_session() {
        let mut c = ctx(Path::new("A"));
        c.resume = Some("OLD".into());
        let p = build_spawn(&cfg(), &agent("claude"), &c).unwrap();
        assert_eq!(p.args, ["--resume", "OLD", "--settings", "C:/r/s.json"]);
        assert_eq!(p.session_id.as_deref(), Some("OLD"));
    }

    #[test]
    fn codex_resume_goes_first_and_plan_mode() {
        let mut s = agent("codex");
        s.permission = Permission::Plan;
        s.effort = Some("high".into());
        let mut c = ctx(Path::new("A"));
        c.resume = Some("R9".into());
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        assert_eq!(p.args, ["resume", "R9", "-s", "read-only", "-c", "model_reasoning_effort=high"]);
    }

    #[test]
    fn account_and_route_env() {
        let mut s = agent("claude");
        s.account = Some("trabalho".into());
        s.route = Some("openrouter".into());
        s.model = Some("deepseek/deepseek-chat".into());
        let mut c = ctx(Path::new("C:/acc"));
        c.route_secret = Some("sk-x".into());
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        assert!(p.env.contains(&("CLAUDE_CONFIG_DIR".into(), Path::new("C:/acc").join("claude").join("trabalho").to_string_lossy().to_string())));
        assert!(p.env.contains(&("ANTHROPIC_BASE_URL".into(), "https://openrouter.ai/api".into())));
        assert!(p.env.contains(&("ANTHROPIC_AUTH_TOKEN".into(), "sk-x".into())));
        assert!(p.env.contains(&("ANTHROPIC_MODEL".into(), "deepseek/deepseek-chat".into())));
        assert!(p.env.contains(&("ANTHROPIC_SMALL_FAST_MODEL".into(), "deepseek/deepseek-chat".into())));
        assert!(p.env.contains(&("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), "deepseek/deepseek-chat".into())));
        assert!(p.env.contains(&("ANTHROPIC_API_KEY".into(), String::new())));
        assert_eq!(p.route_label.as_deref(), Some("CLAUDE ← OPENROUTER"));
    }

    #[test]
    fn account_names_are_validated() {
        for ok in ["trabalho", "conta-2", "a_b", "com", "console"] {
            assert!(valid_account(ok), "{ok}");
        }
        for bad in ["", "principal", "..", "a/b", "a b", "CON", "nul", "com1", "LPT9", "ação"] {
            assert!(!valid_account(bad), "{bad}");
        }
        let mut s = agent("claude");
        s.account = Some("..\\x".into());
        assert_eq!(build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap_err().code(), "bad_account");
    }

    #[test]
    fn principal_account_adds_only_the_update_guard() {
        let mut s = agent("claude");
        s.account = Some("principal".into());
        // a única variável é a que desliga o autoatualizador do Claude Code
        assert_eq!(build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap().env, vec![("DISABLE_AUTOUPDATER".to_string(), "1".to_string())]);
    }

    #[test]
    fn claude_pane_turns_off_self_update() {
        let p = build_spawn(&cfg(), &agent("claude"), &ctx(Path::new("A"))).unwrap();
        assert!(p.env.contains(&("DISABLE_AUTOUPDATER".into(), "1".into())));
    }

    #[test]
    fn route_on_non_routable_cli_fails() {
        let mut s = agent("codex");
        s.route = Some("openrouter".into());
        assert_eq!(build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap_err().code(), "route_unsupported");
    }

    fn env_of(p: &SpawnPlan, k: &str) -> Option<String> {
        p.env.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone())
    }

    fn gemini_with(model: &str, program: &'static str) -> AppResult<SpawnPlan> {
        let mut c = ctx(Path::new("A"));
        c.program_path = Some(program);
        let mut s = agent("gemini");
        s.model = Some(model.into());
        build_spawn(&cfg(), &s, &c)
    }

    #[test]
    fn npm_shim_goes_through_cmd() {
        let p = gemini_with("gemini-3-pro", "C:\\Users\\Ana Silva\\npm\\gemini.cmd").unwrap();
        assert_eq!(p.program, "cmd.exe");
        assert_eq!(p.args, ["/d", "/v:off", "/c", "%REGENTE_CMDLINE%"]);
        assert_eq!(env_of(&p, "REGENTE_CMDLINE").unwrap(), "\"C:\\Users\\Ana Silva\\npm\\gemini.cmd\" -m gemini-3-pro");
    }

    #[test]
    fn cmd_injection_rejected_or_neutralized() {
        let p = gemini_with("a&echo INJECTED", "C:\\npm\\gemini.cmd").unwrap();
        assert_eq!(env_of(&p, "REGENTE_CMDLINE").unwrap(), "C:\\npm\\gemini.cmd -m \"a&echo INJECTED\"");
        assert_eq!(quote("C:\\dir with\\"), "\"C:\\dir with\\\\\"");
    }

    #[test]
    fn quote_in_arg_rejected() {
        assert_eq!(gemini_with("x\" & calc", "C:\\npm\\gemini.cmd").unwrap_err().code(), "bad_arg");
    }

    #[test]
    fn cmd_wrapper_really_passes_args_intact() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("pasta com espaco");
        std::fs::create_dir_all(&dir).unwrap();
        let bat = dir.join("eco.cmd");
        std::fs::write(&bat, "@echo off\r\necho [%1] [%2]\r\n").unwrap();
        let mut env = Vec::new();
        let (prog, args) = wrap_batch(bat.to_string_lossy().to_string(), vec!["a&echo INJECTED".into(), "b c".into()], &mut env).unwrap();
        let out = crate::util::cmd(&prog).args(&args).envs(env).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("[\"a&echo INJECTED\"] [\"b c\"]"), "{text}");
        assert!(!text.lines().any(|l| l.trim() == "INJECTED"), "{text}");
    }

    #[test]
    fn run_on_unknown_shell_unsupported() {
        let mut c = cfg();
        c.shells.push(ShellDef { id: "zsh".into(), command: "zsh".into(), ..Default::default() });
        let s = PaneSpec { kind: PaneKind::Terminal, shell: Some("zsh".into()), run: Some("x".into()), ..Default::default() };
        assert_eq!(build_spawn(&c, &s, &ctx(Path::new("A"))).unwrap_err().code(), "run_unsupported");
    }

    #[test]
    fn which_prefers_exe_anywhere_and_ignores_cwd() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x"), "").unwrap();
        std::fs::write(a.join("x.cmd"), "").unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        assert_eq!(which_in("x", &path), Some(a.join("x.cmd").to_string_lossy().to_string()));
        std::fs::write(b.join("x.exe"), "").unwrap();
        assert_eq!(which_in("x", &path), Some(b.join("x.exe").to_string_lossy().to_string()));
        assert_eq!(which_in("x.cmd", &path), Some(a.join("x.cmd").to_string_lossy().to_string()));
        // a file only in the cwd (not on PATH) is never returned
        // (the probe lives in a temp dir that is the process cwd's stand-in: not on `path`)
        let other = d.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("regente_cwd_only_probe.exe"), "").unwrap();
        assert_eq!(which_in("regente_cwd_only_probe", &path), None);
    }

    #[test]
    fn terminal_with_run_in_powershell_and_cmd() {
        let mut s = PaneSpec { kind: PaneKind::Terminal, cwd: "C:/".into(), run: Some("npm i -g x".into()), ..Default::default() };
        let p = build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap();
        assert_eq!(p.program, "powershell.exe");
        assert_eq!(p.args, ["-NoLogo", "-NoExit", "-Command", "npm i -g x"]);
        s.shell = Some("cmd".into());
        assert_eq!(build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap().args, ["/k", "npm i -g x"]);
    }

    #[test]
    fn browser_is_not_a_process() {
        let s = PaneSpec { kind: PaneKind::Browser, ..Default::default() };
        assert_eq!(build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap_err().code(), "not_a_process");
    }

    #[test]
    fn cursor_is_cursor_agent_or_agent_only_when_agent_says_cursor() {
        use std::cell::Cell;
        // PATH falso: só os nomes listados existem
        let on_path = |names: &'static [&'static str]| move |c: &str| names.contains(&c).then(|| format!("C:/bin/{c}.exe"));
        // cursor-agent existe: vence, e o agent nem é consultado
        let asked = Cell::new(false);
        let r = resolve_cli_with("cursor-agent", on_path(&["cursor-agent", "agent"]), |_| {
            asked.set(true);
            true
        });
        assert_eq!(r.as_deref(), Some("C:/bin/cursor-agent.exe"));
        assert!(!asked.get(), "com cursor-agent no PATH o agent não é conferido");
        // só o agent, e ele se diz Cursor: usa o agent
        assert_eq!(resolve_cli_with("cursor-agent", on_path(&["agent"]), |p| p.ends_with("agent.exe")).as_deref(), Some("C:/bin/agent.exe"));
        // só o agent, mas é outro programa (nome genérico): não é o Cursor
        assert_eq!(resolve_cli_with("cursor-agent", on_path(&["agent"]), |_| false), None);
        // o alias é só do Cursor: outros CLIs não caem em `agent`
        assert_eq!(resolve_cli_with("claude", on_path(&["agent"]), |_| true), None);
        assert_eq!(resolve_cli_with("cursor-agent", on_path(&[]), |_| true), None);
    }

    #[test]
    fn which_finds_cmd_exe() {
        assert!(which("cmd").unwrap().to_lowercase().ends_with("cmd.exe"));
        assert!(which("definitivamente-nao-existe-xyz").is_none());
    }

    #[test]
    fn prompt_file_reference_is_the_last_arg() {
        let mut s = agent("claude");
        s.prompt = Some("Faça X".into());
        let mut c = ctx(Path::new("A"));
        c.prompt_file = Some(PathBuf::from("C:/p/prompt.md"));
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        assert_eq!(p.args.last().map(String::as_str), Some("Siga as instruções do arquivo \"C:/p/prompt.md\""));
        assert!(p.initial_input.is_none());
    }

    #[test]
    fn plugin_dir_flag_before_prompt() {
        let mut s = agent("claude");
        s.prompt = Some("Faça X".into());
        let mut c = ctx(Path::new("A"));
        c.prompt_file = Some(PathBuf::from("C:/p/prompt.md"));
        c.plugin_dir = Some(Path::new("C:/p/plugin"));
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        let flag = p.args.iter().position(|a| a == "--plugin-dir").expect("--plugin-dir");
        assert_eq!(p.args[flag + 1], "C:/p/plugin");
        assert!(flag < p.args.len() - 1, "{:?}", p.args);
        assert_eq!(p.args.last().map(String::as_str), Some("Siga as instruções do arquivo \"C:/p/prompt.md\""));
    }

    #[test]
    fn extra_args_before_prompt() {
        let mut s = agent("claude");
        s.prompt = Some("Faça X".into());
        let mut c = ctx(Path::new("A"));
        c.prompt_file = Some(PathBuf::from("C:/p/prompt.md"));
        c.extra_args = vec!["--mcp-config".into(), "f".into()];
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        let flag = p.args.iter().position(|a| a == "--mcp-config").unwrap();
        assert_eq!(p.args[flag + 1], "f");
        assert_eq!(p.args.last().map(String::as_str), Some("Siga as instruções do arquivo \"C:/p/prompt.md\""));
        assert!(flag < p.args.len() - 1);
    }

    #[test]
    fn cmd_shim_gets_prompt_by_paste() {
        let mut s = agent("gemini");
        s.prompt = Some("Olá\r\nmundo".into());
        let mut c = ctx(Path::new("A"));
        c.program_path = Some("C:\\npm\\gemini.cmd");
        c.prompt_file = Some(PathBuf::from("C:/p/prompt.md"));
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        assert!(!p.args.iter().any(|a| a.contains("Siga")));
        assert!(p.initial_input.unwrap().starts_with(b"\x1b[200~"));
    }

    #[test]
    fn system_only_is_pasted_on_cmd_shim() {
        let mut s = agent("gemini");
        s.append_system = Some("SYS".into());
        let mut c = ctx(Path::new("A"));
        c.program_path = Some("C:\\npm\\gemini.cmd");
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        assert_eq!(p.initial_input, Some(bracketed("SYS")));
    }

    #[test]
    fn disallowed_tools_and_append_system_flags() {
        let mut s = agent("claude");
        s.disallowed_tools = Some(vec!["Edit".into(), "Bash".into()]);
        s.append_system = Some("X".into());
        let p = build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap();
        assert!(p.args.windows(2).any(|w| w == ["--disallowedTools", "Edit,Bash"]));
        assert!(p.args.windows(2).any(|w| w == ["--append-system-prompt", "X"]));
    }

    #[test]
    fn batch_cli_with_system_flag_still_gets_the_system_text() {
        // a .cmd shim can't take the system text as an argument, so it goes into the paste
        let mut s = agent("claude");
        s.prompt = Some("tarefa".into());
        s.append_system = Some("MEMORIA".into());
        let mut c = ctx(Path::new("A"));
        c.program_path = Some("C:\\npm\\claude.cmd");
        c.prompt_file = Some(PathBuf::from("C:/p/prompt.md"));
        let p = build_spawn(&cfg(), &s, &c).unwrap();
        assert!(!p.args.iter().any(|a| a.contains("MEMORIA")), "{:?}", p.args);
        assert_eq!(p.initial_input.unwrap(), bracketed(&with_system("MEMORIA", "tarefa")));
    }

    #[test]
    fn system_is_prefixed_to_paste_when_cli_has_no_system_flag() {
        let mut s = agent("codex");
        s.prompt = Some("tarefa".into());
        s.append_system = Some("regras".into());
        let p = build_spawn(&cfg(), &s, &ctx(Path::new("A"))).unwrap();
        assert_eq!(p.initial_input.unwrap(), bracketed(&with_system("regras", "tarefa")));
    }
}
