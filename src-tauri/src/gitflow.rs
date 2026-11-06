//! Commit e PR pelo Regente (P2.2). Tudo roda com o clique do usuário; git e gh recebem argumentos separados.
//! A sugestão de mensagem usa o provedor da IA auxiliar (ou o Decisor) quando a chave está no cofre; sem chave, um texto
//! montado com os nomes dos arquivos. O diff vai para a IA capado em `DIFF_MAX`.

use crate::agentic::{self, DECISOR_MODEL};
use crate::error::{fail, AppError, AppResult};
use crate::git::{git, git_bytes};
use crate::jarvis;
use crate::missions;
use crate::review::{checked, split_nul};
use crate::runtime::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest diff text sent to the AI, in bytes.
pub const DIFF_MAX: usize = 30 * 1024;
/// Largest commit message or PR body accepted.
pub const MSG_MAX: usize = 2000;

const COMMIT_SYSTEM: &str = "Você escreve mensagens de commit no padrão Conventional Commits, em português do Brasil. \
Responda somente com a mensagem: uma linha de assunto no formato tipo: descrição (até 72 caracteres), e se precisar \
uma linha em branco e um corpo curto em tópicos. Sem markdown e sem cercas de código.";
const PR_SYSTEM: &str = "Você escreve títulos e descrições de pull request em português do Brasil. \
Responda somente com JSON: {\"title\": \"<título curto>\", \"body\": \"<descrição em markdown>\"}.";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub path: String,
    /// A novo, M alterado, D apagado, ? não rastreado.
    pub code: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommitSuggestion {
    pub message: String,
    /// "ia" ou "modelo" (texto pelos nomes dos arquivos).
    pub source: String,
    pub ai_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrSuggestion {
    pub title: String,
    pub body: String,
    pub source: String,
    pub ai_error: Option<String>,
}

/// Folder the git calls run in: the mission's worktree, or the repository root of the workspace.
pub fn target(s: &AppState, workspace_id: &str, mission_id: Option<&str>) -> AppResult<PathBuf> {
    let ws = s.db.get_workspace(workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return fail("remote_unsupported", "o git do Regente só roda em workspace local");
    }
    match mission_id {
        Some(id) => {
            let m = s.db.get_mission(id)?;
            if m.workspace_id != ws.id {
                return fail("bad_mission", "a missão não é deste workspace");
            }
            Ok(missions::live_worktree(&m)?.path)
        }
        None => {
            // um processo só: fora de repositório o `--show-toplevel` falha (antes era `is_repo` e depois ele)
            match git(Path::new(&ws.path), &["rev-parse", "--show-toplevel"]) {
                Ok(top) => Ok(PathBuf::from(top)),
                Err(_) => fail("not_a_repo", "a pasta do workspace não é um repositório git"),
            }
        }
    }
}

/// The changes to pick from: staged, unstaged and untracked files (renames shown as delete plus add).
pub fn status_entries(root: &Path) -> AppResult<Vec<Entry>> {
    parse_status(&git_bytes(root, &["--no-optional-locks", "status", "--porcelain", "-z", "--untracked-files=all", "--no-renames"])?)
}

/// As marcas da árvore de arquivos, a cada poucos segundos: sem pegar o `index.lock` (`--no-optional-locks`, não briga
/// com o git do agente nem com o Sincronizar) e com pasta nova numa linha só (`dir/`, em vez de cada arquivo dentro dela).
pub fn status_marks(root: &Path) -> AppResult<Vec<Entry>> {
    parse_status(&git_bytes(root, &["--no-optional-locks", "status", "--porcelain", "-z", "--untracked-files=normal", "--no-renames"])?)
}

fn parse_status(raw: &[u8]) -> AppResult<Vec<Entry>> {
    Ok(split_nul(raw)
        .into_iter()
        .filter(|e| e.len() > 3)
        .map(|e| {
            let xy = &e[..2];
            let code = if xy.contains('?') {
                "?"
            } else if xy.contains('D') {
                "D"
            } else if xy.contains('A') {
                "A"
            } else {
                "M"
            };
            Entry { path: e[3..].to_string(), code: code.into() }
        })
        .collect())
}

/// Commit with the chosen files only: they are staged, then `commit -- <files>` takes just them.
pub fn commit(root: &Path, paths: &[String], message: &str) -> AppResult<String> {
    let msg = message.trim();
    if msg.is_empty() {
        return fail("empty_message", "escreva a mensagem do commit");
    }
    if msg.chars().count() > MSG_MAX {
        return fail("message_too_long", format!("a mensagem passa de {MSG_MAX} caracteres"));
    }
    if paths.is_empty() {
        return fail("no_files", "escolha ao menos um arquivo");
    }
    for p in paths {
        checked(root, p)?;
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(paths.iter().map(String::as_str));
    git(root, &add)?;
    let mut args = vec!["commit", "-m", msg, "--"];
    args.extend(paths.iter().map(String::as_str));
    git(root, &args)?;
    git(root, &["rev-parse", "--short", "HEAD"])
}

/// The diff of the chosen files against HEAD (what the AI reads), with a line for each new file.
pub fn commit_diff(root: &Path, paths: &[String]) -> AppResult<String> {
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "HEAD", "--"];
    args.extend(paths.iter().map(String::as_str));
    // a repository without commits has no HEAD: the diff is then empty
    let mut out = git(root, &args).unwrap_or_default();
    for e in status_entries(root)?.iter().filter(|e| e.code == "?" && paths.contains(&e.path)) {
        out += &format!("\narquivo novo (sem diff): {}\n", e.path);
    }
    Ok(out)
}

/// Diff de tudo que mudou (workspace ou missão), capado em `DIFF_MAX`: é o `@diff` do compositor.
/// Com `base` (missão com worktree), vem também o que a branch já tem contra a base (`base...HEAD`).
/// Falha do git vira erro com a causa, nunca "sem alterações".
pub fn working_diff(root: &Path, base: Option<&str>) -> AppResult<String> {
    let build = || -> AppResult<String> {
        let mut out = String::new();
        if let Some(b) = base {
            check_base(b)?;
            out += &git(root, &["diff", "--no-color", "--no-ext-diff", &format!("{b}...HEAD")])?;
        }
        // git tira a quebra final: uma nova entre as duas partes evita colar o "diff --git" na linha de cima
        if !out.is_empty() {
            out.push('\n');
        }
        out += &git(root, &["diff", "--no-color", "--no-ext-diff", "HEAD"])?;
        for e in status_entries(root)?.iter().filter(|e| e.code == "?") {
            out += &format!("\narquivo novo (sem diff): {}\n", e.path);
        }
        Ok(cap_diff(&out))
    };
    build().map_err(|e| AppError::new("diff_failed", format!("não consegui gerar o diff: {e}")))
}

/// Text cut to `DIFF_MAX` bytes at a char boundary, with a marker when it was cut.
pub fn cap_diff(text: &str) -> String {
    const MARK: &str = "\n[diff truncado]";
    if text.len() <= DIFF_MAX {
        return text.to_string();
    }
    let mut end = DIFF_MAX - MARK.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARK}", &text[..end])
}

/// Fallback commit message from the file names: the type comes from the files (docs, test or chore).
pub fn commit_template(paths: &[String]) -> String {
    let all = |f: fn(&str) -> bool| !paths.is_empty() && paths.iter().all(|p| f(p));
    let kind = if all(|p| p.ends_with(".md") || p.ends_with(".txt") || p.ends_with(".rst")) {
        "docs"
    } else if all(|p| p.contains("test") || p.contains("spec")) {
        "test"
    } else {
        "chore"
    };
    let subject = match paths {
        [] => format!("{kind}: atualiza arquivos"),
        [one] => format!("{kind}: atualiza {one}"),
        many => format!("{kind}: atualiza {} arquivos", many.len()),
    };
    let body: Vec<String> = paths.iter().map(|p| format!("- {p}")).collect();
    format!("{subject}\n\n{}", body.join("\n")).trim_end().to_string()
}

/// Fallback PR title and body: one commit gives its subject as the title; the body lists the files.
pub fn pr_template(branch: &str, commits: &[String], paths: &[String]) -> (String, String) {
    let title = match commits {
        [one] => one.clone(),
        _ => format!("Alterações de {branch}"),
    };
    let files: Vec<String> = paths.iter().map(|p| format!("- {p}")).collect();
    (title, format!("## Alterações\n{}", files.join("\n")))
}

/// The AI's commit message: code fences out, trimmed, capped. None when nothing is left.
pub fn clean_message(text: &str) -> Option<String> {
    let body: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
    let msg: String = body.join("\n").trim().chars().take(MSG_MAX).collect();
    (!msg.is_empty()).then_some(msg)
}

/// The AI's PR answer: the first JSON object of the text with a non-empty title and body.
pub fn parse_pr(text: &str) -> Option<(String, String)> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let v: Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let title = v["title"].as_str()?.trim().to_string();
    let body = v["body"].as_str()?.trim().to_string();
    (!title.is_empty() && !body.is_empty()).then_some((title.chars().take(300).collect(), body.chars().take(MSG_MAX).collect()))
}

/// Pushes the branch to `origin` without prompts and within 120 s.
pub fn push(root: &Path, branch: &str) -> AppResult<()> {
    crate::git::git_remote(root, &["push", "-u", "origin", branch], Duration::from_secs(120)).map(|_| ())
}

/// The URL `gh pr create` prints at the end.
pub fn pr_url(out: &str) -> Option<String> {
    out.split_whitespace().rev().find(|w| w.starts_with("https://") && w.contains("/pull/")).map(str::to_string)
}

/// Branch base padrão do PR: a do remoto (`origin/HEAD`), senão `init.defaultBranch` se o repo a tiver, senão a `main` ou `master` que existir.
pub fn default_base(root: &Path) -> String {
    let remote = git(root, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"]).ok().and_then(|r| r.trim().strip_prefix("refs/remotes/origin/").map(str::to_string));
    // init.defaultBranch é a do próximo `git init` da máquina, não deste repo: só vale se a branch existir aqui
    let configured = || git(root, &["config", "init.defaultBranch"]).ok().map(|b| b.trim().to_string()).filter(|b| !b.is_empty() && crate::git::branch_exists(root, b));
    let existing = || ["main", "master"].into_iter().find(|b| crate::git::branch_exists(root, b)).map(str::to_string);
    remote.or_else(configured).or_else(existing).unwrap_or_else(|| "main".into())
}

/// Opens the PR: checks `gh` (missing or not logged in: the error says what to do), pushes the branch, then `gh pr create`.
/// The push runs only after the check; both `gh` and `push` are passed in so tests never touch the network.
pub fn open_pr(base: &str, branch: &str, title: &str, body: &str, gh: &dyn Fn(&[&str]) -> AppResult<String>, push: &dyn Fn(&str) -> AppResult<()>) -> AppResult<String> {
    if title.trim().is_empty() {
        return fail("empty_title", "escreva o título do PR");
    }
    gh(&["auth", "status"])?;
    push(branch)?;
    let out = gh(&["pr", "create", "--base", base, "--head", branch, "--title", title.trim(), "--body", body])?;
    pr_url(&out).ok_or_else(|| AppError::new("gh_failed", format!("o gh não devolveu a URL do PR: {out}")))
}

/// Provider for the suggestion: the IA auxiliar one when its key is in the vault, else the Decisor's. None = no key.
pub(crate) struct Provider {
    /// "a IA auxiliar" or "o Decisor": names the provider in errors.
    who: &'static str,
    kind: String,
    base: String,
    model: String,
    key: String,
}

pub(crate) fn provider(s: &AppState) -> AppResult<Option<Provider>> {
    let j = jarvis::load(&s.data);
    if let Some(name) = j.secret.as_deref() {
        if let Some(key) = s.secret(name)? {
            return Ok(Some(Provider { who: "a IA auxiliar", kind: j.provider, base: j.base_url, model: j.model, key }));
        }
    }
    let d = agentic::load(&s.data).decisor;
    if !matches!(d.kind.as_str(), "anthropic" | "openai") {
        return Ok(None);
    }
    let base = match d.base_url.trim() {
        "" if d.kind == "anthropic" => "https://api.anthropic.com".to_string(),
        "" => return Ok(None),
        b => b.to_string(),
    };
    let Some(name) = d.secret.as_deref() else { return Ok(None) };
    let Some(key) = s.secret(name)? else { return Ok(None) };
    let model = if d.model.trim().is_empty() { DECISOR_MODEL.to_string() } else { d.model.clone() };
    Ok(Some(Provider { who: "o Decisor", kind: d.kind, base, model, key }))
}

pub(crate) async fn complete(p: &Provider, system: &str, user: &str) -> AppResult<String> {
    agentic::check_base_url(p.base.trim())?;
    let base = p.base.trim_end_matches('/');
    let client = agentic::client()?;
    let who = p.who;
    let map = |e: AppError| AppError::new("ai_failed", e.to_string());
    if p.kind == "openai" {
        let body = json!({
            "model": p.model,
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}],
        });
        let v = agentic::send(client.post(format!("{base}/chat/completions")).bearer_auth(&p.key).json(&body), who).await.map_err(map)?;
        Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string())
    } else {
        let body = json!({
            "model": p.model, "max_tokens": 400, "system": system,
            "messages": [{"role": "user", "content": user}], "output_config": {"effort": "low"},
        });
        let req = client.post(format!("{base}/v1/messages")).header("anthropic-version", "2023-06-01").header("x-api-key", &p.key).json(&body);
        let v = agentic::send(req, who).await.map_err(map)?;
        Ok(v["content"].as_array().and_then(|c| c.iter().find_map(|b| b["text"].as_str())).unwrap_or_default().to_string())
    }
}

fn list(paths: &[String]) -> String {
    paths.iter().map(|p| format!("- {p}\n")).collect()
}

/// A commit message for the chosen files. With a key, the AI writes it from the capped diff; any AI failure falls back
/// to the template and says why.
pub async fn suggest_commit(s: &AppState, paths: &[String], diff: &str) -> AppResult<CommitSuggestion> {
    let Some(p) = provider(s)? else {
        return Ok(CommitSuggestion { message: commit_template(paths), source: "modelo".into(), ai_error: None });
    };
    let user = format!("Arquivos:\n{}\nDiff:\n{}", list(paths), cap_diff(diff));
    match complete(&p, COMMIT_SYSTEM, &user).await.map(|t| clean_message(&t)) {
        Ok(Some(message)) => Ok(CommitSuggestion { message, source: "ia".into(), ai_error: None }),
        Ok(None) => Ok(CommitSuggestion { message: commit_template(paths), source: "modelo".into(), ai_error: Some("a IA não devolveu texto".into()) }),
        Err(e) => Ok(CommitSuggestion { message: commit_template(paths), source: "modelo".into(), ai_error: Some(e.to_string()) }),
    }
}

/// Title and body of the PR from the branch's commits and diff against `base` (capped like the commit one).
/// Commits (assuntos) e arquivos da branch contra `base`: o que o modelo local usa.
/// Base vinda da UI: um `-` inicial virava opção do git, e espaço ou `..` não são nome de branch.
pub fn check_base(base: &str) -> AppResult<()> {
    if base.starts_with('-') || base.chars().any(char::is_whitespace) || base.contains("..") {
        return fail("bad_base", format!("branch base inválida: {base}"));
    }
    Ok(())
}

fn pr_facts(root: &Path, base: &str) -> AppResult<(Vec<String>, Vec<String>)> {
    check_base(base)?;
    let range = format!("{base}...HEAD");
    let commits: Vec<String> = git(root, &["log", "--format=%s", &format!("{base}..HEAD")])?.lines().map(str::to_string).collect();
    let paths: Vec<String> = git(root, &["diff", "--name-only", &range])?.lines().map(str::to_string).collect();
    Ok((commits, paths))
}

/// Mensagem do modelo para já abrir o commit preenchida. Com IA configurada não preenche: a IA roda só no clique.
pub fn commit_prefill(s: &AppState, paths: &[String]) -> AppResult<Option<String>> {
    Ok(provider(s)?.is_none().then(|| commit_template(paths)))
}

/// Título e descrição do modelo para já abrir o PR preenchidos. Com IA configurada não preenche.
pub fn pr_prefill(s: &AppState, root: &Path, base: &str, branch: &str) -> AppResult<Option<PrSuggestion>> {
    if provider(s)?.is_some() {
        return Ok(None);
    }
    let (commits, paths) = pr_facts(root, base)?;
    let (title, body) = pr_template(branch, &commits, &paths);
    Ok(Some(PrSuggestion { title, body, source: "modelo".into(), ai_error: None }))
}

pub async fn suggest_pr(s: &AppState, root: &Path, base: &str, branch: &str) -> AppResult<PrSuggestion> {
    let range = format!("{base}...HEAD");
    let (commits, paths) = pr_facts(root, base)?;
    let (t_title, t_body) = pr_template(branch, &commits, &paths);
    let Some(p) = provider(s)? else {
        return Ok(PrSuggestion { title: t_title, body: t_body, source: "modelo".into(), ai_error: None });
    };
    let diff = git(root, &["diff", "--no-color", "--no-ext-diff", &range]).unwrap_or_default();
    let user = format!("Branch: {branch}\nCommits:\n{}\nArquivos:\n{}\nDiff:\n{}", commits.join("\n"), list(&paths), cap_diff(&diff));
    match complete(&p, PR_SYSTEM, &user).await {
        Ok(text) => match parse_pr(&text) {
            Some((title, body)) => Ok(PrSuggestion { title, body, source: "ia".into(), ai_error: None }),
            None => Ok(PrSuggestion { title: t_title, body: t_body, source: "modelo".into(), ai_error: Some("a IA não devolveu título e descrição".into()) }),
        },
        Err(e) => Ok(PrSuggestion { title: t_title, body: t_body, source: "modelo".into(), ai_error: Some(e.to_string()) }),
    }
}

/// Prazo de fetch e push do rodapé (sem prompt de credencial: ver `git_remote`).
const NET: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Branches {
    /// Vazio com HEAD destacado.
    pub current: String,
    /// Locais, a mais recente primeiro.
    pub local: Vec<String>,
    /// `origin/x` sem branch local `x` (trocar para ela cria a local rastreando o remoto).
    pub remote: Vec<String>,
    pub upstream: Option<String>,
    /// Commits locais que o remoto não tem e do remoto que a local não tem (pela última busca).
    pub ahead: u32,
    pub behind: u32,
    /// A busca no remoto falhou (sem rede, sem credencial): a lista é a da última busca.
    pub fetch_error: Option<String>,
}

fn upstream(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).ok().filter(|u| !u.is_empty())
}

/// (atrás, à frente) da branch atual contra a do remoto.
fn counts(root: &Path) -> AppResult<(u32, u32)> {
    let out = git(root, &["rev-list", "--left-right", "--count", "@{u}...HEAD"])?;
    let mut n = out.split_whitespace().map(|x| x.parse().unwrap_or(0));
    Ok((n.next().unwrap_or(0), n.next().unwrap_or(0)))
}

fn fetch(root: &Path) -> AppResult<()> {
    crate::git::git_remote(root, &["fetch", "--prune", "--quiet"], NET).map(|_| ())
}

/// Branches do repositório. `fetch`: busca o remoto antes (o erro vai em `fetch_error`, a lista sai mesmo assim).
pub fn branches(root: &Path, with_fetch: bool) -> AppResult<Branches> {
    let fetch_error = if with_fetch { fetch(root).err().map(|e| e.to_string()) } else { None };
    let refs = |ns: &str| -> AppResult<Vec<String>> {
        Ok(git(root, &["for-each-ref", "--sort=-committerdate", "--format=%(refname:short)", ns])?.lines().map(str::to_string).collect())
    };
    let local = refs("refs/heads")?;
    // `origin/HEAD` sai como "origin" (sem barra) ou "origin/HEAD"
    let remote = refs("refs/remotes")?
        .into_iter()
        .filter(|r| r.split_once('/').is_some_and(|(_, name)| name != "HEAD" && !local.iter().any(|l| l == name)))
        .collect();
    let upstream = upstream(root);
    let (behind, ahead) = if upstream.is_some() { counts(root).unwrap_or((0, 0)) } else { (0, 0) };
    Ok(Branches { current: crate::git::current_branch(root)?, local, remote, upstream, ahead, behind, fetch_error })
}

/// O que o rodapé mostra (branch, upstream, ↑↓) num processo só: `status --porcelain=v2 --branch`, sem as listas de
/// branches (essas são do seletor, `branches`).
pub fn sync_state(root: &Path) -> AppResult<Branches> {
    let raw = crate::git::git_bytes(root, &["--no-optional-locks", "status", "--porcelain=v2", "--branch", "-z", "--untracked-files=no"])?;
    Ok(parse_sync_state(&crate::review::split_nul(&raw)))
}

fn parse_sync_state(linhas: &[String]) -> Branches {
    let mut b = Branches { current: String::new(), local: vec![], remote: vec![], upstream: None, ahead: 0, behind: 0, fetch_error: None };
    for l in linhas {
        if let Some(h) = l.strip_prefix("# branch.head ") {
            b.current = if h == "(detached)" { String::new() } else { h.to_string() };
        } else if let Some(u) = l.strip_prefix("# branch.upstream ") {
            b.upstream = Some(u.to_string());
        } else if let Some(ab) = l.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            b.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok()).unwrap_or(0);
            b.behind = it.next().and_then(|x| x.trim_start_matches('-').parse().ok()).unwrap_or(0);
        }
    }
    b
}

/// Troca de branch: local (`switch b`), do remoto (`switch --track origin/b`, cria a local) ou nova (`switch -c b`).
/// Devolve a branch atual depois da troca.
pub fn switch(root: &Path, name: &str, create: bool) -> AppResult<String> {
    let name = name.trim();
    // um `-` inicial viraria opção do git
    if name.is_empty() || name.starts_with('-') || git(root, &["check-ref-format", "--branch", name]).is_err() {
        return fail("bad_branch", format!("nome de branch inválido: {name}"));
    }
    let args: Vec<&str> = if create {
        vec!["switch", "-c", name]
    } else if crate::git::branch_exists(root, name) {
        vec!["switch", name]
    } else if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/{name}")]).is_ok() {
        vec!["switch", "--track", name]
    } else {
        return fail("bad_branch", format!("a branch {name} não existe"));
    };
    git(root, &args).map_err(|e| {
        if e.to_string().contains("would be overwritten") {
            AppError::new("dirty", format!("há alterações não commitadas que a branch {name} sobrescreveria: commite antes de trocar"))
        } else {
            e
        }
    })?;
    crate::git::current_branch(root)
}

/// Branch atual com remoto, para puxar ou sincronizar. Sem remoto: None (sincronizar publica).
fn tracked(root: &Path) -> AppResult<(String, Option<String>)> {
    let branch = crate::git::current_branch(root)?;
    if branch.is_empty() {
        return fail("detached", "HEAD destacado: troque para uma branch antes");
    }
    Ok((branch, upstream(root)))
}

/// Puxa só em avanço rápido (nunca cria merge); alterações não commitadas são guardadas e voltam (`--autostash`).
pub fn pull(root: &Path) -> AppResult<String> {
    let (branch, up) = tracked(root)?;
    if up.is_none() {
        return fail("no_upstream", format!("a branch {branch} não está no remoto: use Sincronizar para publicá-la"));
    }
    fetch(root)?;
    let (behind, ahead) = counts(root)?;
    if behind == 0 {
        return Ok(format!("{branch} já está atualizada"));
    }
    if ahead > 0 {
        return fail("diverged", format!("{branch} tem {ahead} commit(s) que o remoto não tem: use Sincronizar"));
    }
    git(root, &["merge", "--ff-only", "--autostash", "@{u}"])?;
    Ok(format!("{branch}: {behind} commit(s) recebido(s)"))
}

/// Recebe o remoto (rebase dos commits locais por cima, `--autostash`) e envia os locais. Branch sem remoto é publicada
/// em `origin`. Conflito no rebase: desfaz o rebase (nada muda) e diz para resolver no terminal.
pub fn sync(root: &Path) -> AppResult<String> {
    let (branch, up) = tracked(root)?;
    if up.is_none() {
        if !git(root, &["remote"])?.lines().any(|r| r == "origin") {
            return fail("no_remote", "o repositório não tem remoto (origin)");
        }
        push(root, &branch)?;
        return Ok(format!("{branch} publicada em origin"));
    }
    fetch(root)?;
    let (behind, ahead) = counts(root)?;
    if behind > 0 {
        if let Err(e) = git(root, &["rebase", "--autostash", "@{u}"]) {
            let _ = git(root, &["rebase", "--abort"]);
            return Err(AppError::new("conflict", format!("conflito ao juntar com o remoto; nada mudou. Resolva no terminal com git pull --rebase ({e})")));
        }
    }
    if ahead > 0 {
        crate::git::git_remote(root, &["push"], NET)?;
    }
    Ok(match (behind, ahead) {
        (0, 0) => format!("{branch} já estava sincronizada"),
        (b, a) => format!("{branch}: {b} recebido(s), {a} enviado(s)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github;
    use crate::vault::Vault;
    use std::sync::{Arc, Mutex};

    #[test]
    fn estado_do_rodape_num_status_so() {
        let l = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let b = parse_sync_state(&l(&["# branch.oid abc", "# branch.head dev", "# branch.upstream origin/dev", "# branch.ab +2 -3"]));
        assert_eq!((b.current.as_str(), b.upstream.as_deref(), b.ahead, b.behind), ("dev", Some("origin/dev"), 2, 3));
        let d = parse_sync_state(&l(&["# branch.oid abc", "# branch.head (detached)"]));
        assert_eq!((d.current.as_str(), d.upstream), ("", None));
        let r = tempfile::tempdir().unwrap();
        git(r.path(), &["init", "-q", "-b", "main"]).unwrap();
        git(r.path(), &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a"]).unwrap();
        assert_eq!(sync_state(r.path()).unwrap().current, "main");
    }

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn diff_is_capped_at_30_kb_on_a_char_boundary() {
        let short = "+linha\n".repeat(10);
        assert_eq!(cap_diff(&short), short);
        let long = "é".repeat(DIFF_MAX);
        let capped = cap_diff(&long);
        assert!(capped.len() <= DIFF_MAX, "{}", capped.len());
        assert!(capped.ends_with("[diff truncado]"));
    }

    #[test]
    fn commit_template_names_the_files() {
        assert_eq!(commit_template(&paths(&["src/a.ts"])), "chore: atualiza src/a.ts\n\n- src/a.ts");
        assert_eq!(commit_template(&paths(&["README.md", "docs/x.md"])), "docs: atualiza 2 arquivos\n\n- README.md\n- docs/x.md");
        assert!(commit_template(&paths(&["src/app.test.ts"])).starts_with("test: "));
        assert!(commit_template(&[]).starts_with("chore: atualiza arquivos"));
    }

    #[test]
    fn bad_base_is_refused_before_git_runs() {
        for bad in ["-x", "--output=/tmp/x", "main dev", "main..dev", "a\tb"] {
            assert_eq!(check_base(bad).unwrap_err().code(), "bad_base", "{bad:?}");
        }
        assert!(check_base("origin/main").is_ok());
        assert!(check_base("feat/x-y_z").is_ok());
    }

    #[test]
    fn pr_template_uses_the_only_commit_as_title() {
        let (t, b) = pr_template("regente/x", &paths(&["feat: login"]), &paths(&["a.ts"]));
        assert_eq!((t.as_str(), b.as_str()), ("feat: login", "## Alterações\n- a.ts"));
        let (t, _) = pr_template("regente/x", &paths(&["um", "dois"]), &[]);
        assert_eq!(t, "Alterações de regente/x");
    }

    #[test]
    fn ai_replies_are_cleaned_and_checked() {
        assert_eq!(clean_message("```\nfix: botão\n```").as_deref(), Some("fix: botão"));
        assert_eq!(clean_message("  \n ").as_deref(), None);
        assert_eq!(parse_pr("claro: {\"title\": \"T\", \"body\": \"B\"}"), Some(("T".into(), "B".into())));
        assert_eq!(parse_pr("{\"title\": \"\", \"body\": \"B\"}"), None);
        assert_eq!(parse_pr("sem json"), None);
        assert_eq!(pr_url("criando\nhttps://github.com/a/b/pull/12\n"), Some("https://github.com/a/b/pull/12".into()));
        assert_eq!(pr_url("erro"), None);
    }

    #[test]
    fn branches_switch_pull_and_sync_against_a_local_remote() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let run = |dir: &Path, args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(dir).status().unwrap().success(), "{args:?}");
        };
        let commit = |dir: &Path, file: &str, text: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            run(dir, &["add", file]);
            run(dir, &["commit", "-q", "-m", file]);
        };
        run(d.path(), &["init", "-q", "--bare", "-b", "main", "r.git"]);
        std::fs::create_dir(&a).unwrap();
        run(&a, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "t"), ("user.email", "t@t")] {
            run(&a, &["config", k, v]);
        }
        commit(&a, "x.txt", "base\n");
        run(&a, &["remote", "add", "origin", "../r.git"]);
        run(&a, &["push", "-q", "-u", "origin", "main"]);
        run(&a, &["push", "-q", "origin", "main:feat"]);
        run(d.path(), &["clone", "-q", "r.git", "b"]);
        for (k, v) in [("user.name", "t"), ("user.email", "t@t")] {
            run(&b, &["config", k, v]);
        }

        let br = branches(&a, true).unwrap();
        assert_eq!((br.current.as_str(), br.local.clone(), br.upstream.as_deref()), ("main", vec!["main".to_string()], Some("origin/main")));
        assert_eq!(br.remote, vec!["origin/feat".to_string()], "sem origin/main (já é local) e sem origin/HEAD");
        assert!(br.fetch_error.is_none());

        // remota vira local rastreando; nova; e de volta
        assert_eq!(switch(&a, "origin/feat", false).unwrap(), "feat");
        assert_eq!(upstream(&a).as_deref(), Some("origin/feat"));
        assert_eq!(switch(&a, "nova", true).unwrap(), "nova");
        assert_eq!(switch(&a, "main", false).unwrap(), "main");
        for bad in ["-x", "nada", "a b", ""] {
            assert_eq!(switch(&a, bad, false).unwrap_err().code(), "bad_branch", "{bad:?}");
        }

        // puxar: avanço rápido, com alteração local guardada e devolvida
        commit(&b, "b.txt", "1\n");
        run(&b, &["push", "-q"]);
        std::fs::write(a.join("x.txt"), "base\nlocal\n").unwrap();
        assert_eq!(pull(&a).unwrap(), "main: 1 commit(s) recebido(s)");
        // a alteração voltou do autostash (com o fim de linha do core.autocrlf da máquina)
        assert_eq!(std::fs::read_to_string(a.join("x.txt")).unwrap().replace("\r\n", "\n"), "base\nlocal\n");
        assert_eq!(pull(&a).unwrap(), "main já está atualizada");
        run(&a, &["checkout", "-q", "x.txt"]);

        // divergiu: puxar recusa, sincronizar faz o rebase e envia
        commit(&a, "a.txt", "a\n");
        commit(&b, "b2.txt", "2\n");
        run(&b, &["push", "-q"]);
        assert_eq!(pull(&a).unwrap_err().code(), "diverged");
        assert_eq!(sync(&a).unwrap(), "main: 1 recebido(s), 1 enviado(s)");
        assert_eq!(sync(&a).unwrap(), "main já estava sincronizada");
        assert_eq!(pull(&b).unwrap(), "main: 1 commit(s) recebido(s)");

        // conflito: o rebase é desfeito e nada muda
        commit(&b, "x.txt", "do b\n");
        run(&b, &["push", "-q"]);
        commit(&a, "x.txt", "do a\n");
        let head = git(&a, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(sync(&a).unwrap_err().code(), "conflict");
        assert_eq!(git(&a, &["rev-parse", "HEAD"]).unwrap(), head);
        assert!(!a.join(".git/rebase-merge").exists() && !a.join(".git/rebase-apply").exists());

        // branch sem remoto: sincronizar publica
        switch(&a, "publicar", true).unwrap();
        assert_eq!(pull(&a).unwrap_err().code(), "no_upstream");
        assert_eq!(sync(&a).unwrap(), "publicar publicada em origin");
        assert_eq!(upstream(&a).as_deref(), Some("origin/publicar"));
    }

    #[test]
    fn working_diff_is_capped_and_empty_when_clean() {
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(d.path()).status().unwrap().success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "trunk"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "i"]);
        assert_eq!(working_diff(d.path(), None).unwrap(), "");
        // um arquivo rastreado que muda muito: o diff sai capado, com a marca
        std::fs::write(d.path().join("grande.txt"), "linha\n").unwrap();
        run(&["add", "grande.txt"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "arquivo"]);
        std::fs::write(d.path().join("grande.txt"), "linha de texto\n".repeat(DIFF_MAX / 4)).unwrap();
        let out = working_diff(d.path(), None).unwrap();
        assert!(out.len() <= DIFF_MAX && out.ends_with("[diff truncado]"), "len {}", out.len());
    }

    #[test]
    fn default_base_follows_the_remote_head() {
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(d.path()).status().unwrap().success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "trunk"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "i"]);
        // origin/HEAD vem antes de init.defaultBranch (que depende da config da máquina, por isso não é fixado aqui)
        run(&["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/develop"]);
        assert_eq!(default_base(d.path()), "develop");
    }

    #[test]
    fn default_base_ignores_a_configured_branch_the_repo_lacks() {
        // o Git for Windows grava init.defaultBranch=master no sistema; num repo só com main isso não pode virar a base
        let d = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(d.path()).status().unwrap().success(), "{args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "i"]);
        run(&["config", "init.defaultBranch", "master"]);
        assert_eq!(default_base(d.path()), "main");
    }

    #[test]
    fn open_pr_checks_gh_before_pushing() {
        let calls = Mutex::new(vec![]);
        let pushed = Mutex::new(vec![]);
        let gh_down = |a: &[&str]| -> AppResult<String> {
            calls.lock().unwrap().push(a.join(" "));
            fail("gh_missing", github::GH_MISSING)
        };
        let push = |b: &str| -> AppResult<()> {
            pushed.lock().unwrap().push(b.to_string());
            Ok(())
        };
        assert_eq!(open_pr("main", "regente/x", "t", "b", &gh_down, &push).unwrap_err().code(), "gh_missing");
        assert!(pushed.lock().unwrap().is_empty(), "no push without gh");
        assert_eq!(calls.lock().unwrap().as_slice(), ["auth status"]);

        let ok = |a: &[&str]| -> AppResult<String> {
            calls.lock().unwrap().push(a.join(" "));
            Ok("https://github.com/a/b/pull/7\n".into())
        };
        let url = open_pr("main", "regente/x", "Título", "Corpo", &ok, &push).unwrap();
        assert_eq!(url, "https://github.com/a/b/pull/7");
        assert_eq!(pushed.lock().unwrap().as_slice(), ["regente/x"]);
        assert!(calls.lock().unwrap().last().unwrap().starts_with("pr create --base main --head regente/x --title Título"));
        assert_eq!(open_pr("main", "b", "  ", "", &ok, &push).unwrap_err().code(), "empty_title");
    }

    #[test]
    fn commit_refuses_empty_message_and_bad_paths() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(commit(d.path(), &paths(&["a.txt"]), "  ").unwrap_err().code(), "empty_message");
        assert_eq!(commit(d.path(), &[], "feat: x").unwrap_err().code(), "no_files");
        assert_eq!(commit(d.path(), &paths(&["../fora.txt"]), "feat: x").unwrap_err().code(), "bad_path");
    }

    fn state(dir: &Path) -> Arc<AppState> {
        AppState::new(dir.into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap()
    }

    /// Mock provider on 127.0.0.1: records each request's x-api-key and body, answers with `reply` (status 200 when Some).
    async fn mock(reply: Option<Value>) -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
        use axum::http::{HeaderMap, StatusCode};
        let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(vec![]));
        let rec = seen.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let rec = rec.clone();
                let reply = reply.clone();
                async move {
                    let key = headers.get("x-api-key").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                    rec.lock().unwrap().push((key, body));
                    match reply {
                        Some(r) => (StatusCode::OK, axum::Json(r)),
                        None => (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({}))),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn with_jarvis_key(s: &AppState, d: &Path, base: &str) {
        let mut cfg = jarvis::JarvisConfig::default();
        cfg.base_url = base.into();
        jarvis::save(d, &cfg).unwrap();
        let v = Vault::with_key(&d.join("vault.json"), [7u8; 32]);
        v.set("jarvis.anthropic", "sk-teste").unwrap();
        s.set_vault_for_tests(v);
    }

    #[tokio::test]
    async fn without_a_key_the_template_is_used_and_no_request_is_made() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let out = suggest_commit(&s, &paths(&["src/a.ts"]), "+x").await.unwrap();
        assert_eq!((out.source.as_str(), out.ai_error), ("modelo", None));
        assert_eq!(out.message, commit_template(&paths(&["src/a.ts"])));
    }

    #[tokio::test]
    async fn with_a_key_the_ai_writes_from_the_capped_diff() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let (base, seen) = mock(Some(json!({"content": [{"type": "text", "text": "```\nfeat: login por e-mail\n```"}]}))).await;
        with_jarvis_key(&s, d.path(), &base);
        let big = "+linha de diff\n".repeat(10_000);
        let out = suggest_commit(&s, &paths(&["src/login.ts"]), &big).await.unwrap();
        assert_eq!((out.source.as_str(), out.message.as_str()), ("ia", "feat: login por e-mail"));
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].0, "sk-teste", "the key goes in the header, from the vault");
        let user = reqs[0].1["messages"][0]["content"].as_str().unwrap();
        assert!(user.len() <= DIFF_MAX + 200, "diff sent capped: {}", user.len());
        assert!(user.contains("[diff truncado]"));
    }

    #[tokio::test]
    async fn ai_failure_falls_back_to_the_template_and_says_why() {
        let d = tempfile::tempdir().unwrap();
        let s = state(d.path());
        let (base, _seen) = mock(None).await;
        with_jarvis_key(&s, d.path(), &base);
        let out = suggest_commit(&s, &paths(&["README.md"]), "+x").await.unwrap();
        assert_eq!(out.source, "modelo");
        assert_eq!(out.message, commit_template(&paths(&["README.md"])));
        // the error names the provider that failed (IA auxiliar here, not the Decisor)
        assert!(out.ai_error.unwrap().contains("a IA auxiliar respondeu HTTP 500"));
    }
}
