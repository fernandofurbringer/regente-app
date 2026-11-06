//! Marketplace: lê o catálogo de fontes GitHub (skills e agentes), guarda em cache por 24 h e instala no
//! Claude, no Codex ou no próprio Regente. Só grava arquivos dentro dessas pastas; nunca executa nada baixado.

use crate::catalog::{parse_frontmatter, reserved_stem, valid_name, FmValue};
use crate::error::{fail, AppError, AppResult};
use crate::settings::{load_or_seed, save_json};
use crate::util::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const API_BASE: &str = "https://api.github.com";
const RAW_BASE: &str = "https://raw.githubusercontent.com";
const CACHE_MS: i64 = 24 * 3600 * 1000;
const TREE_MAX: usize = 5 << 20;
const FILE_MAX: usize = 1 << 20;
/// Total bytes of one install.
const INSTALL_MAX: usize = 10 << 20;
const FILES_MAX: usize = 200;
const DESCRIPTION_MAX: usize = 300;
const DESCRIBE_CONCURRENCY: usize = 8;
/// Whole refresh or install, end to end.
const OP_TIMEOUT: Duration = Duration::from_secs(120);
const MARKETPLACE_DEFAULT: &str = r#"{"sources":[{"owner":"anthropics","repo":"skills","ref":"main"}]}"#;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub owner: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// owner/repo
    pub source: String,
    pub name: String,
    /// skill | agent
    pub kind: String,
    /// Folder in the repo: `skills/<name>` for skills, `agents` for agents.
    pub dir: String,
    /// Skills: paths relative to `dir`. Agents: `["<name>.md"]`.
    pub files: Vec<String>,
    pub description: String,
}

#[derive(Deserialize)]
struct Config {
    sources: Vec<Source>,
}

/// `sources` is the fingerprint: a cache of other sources is never used.
#[derive(Serialize, Deserialize)]
struct Cache {
    ts: i64,
    sources: Vec<Source>,
    items: Vec<Item>,
}

/// A GitHub owner, repo or ref: it becomes part of a URL.
fn valid_part(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A relative path inside a repo or a folder: no `..`, absolute or drive paths, backslashes, `~`, Windows
/// device names, or components ending in `.` or a space.
fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && !p.contains(['\\', ':', '~'])
        && Path::new(p).components().all(|c| match c {
            Component::Normal(s) => {
                let s = s.to_string_lossy();
                !s.ends_with(['.', ' ']) && !reserved_stem(s.split('.').next().unwrap_or(""))
            }
            _ => false,
        })
}

fn sources(data: &Path) -> AppResult<Vec<Source>> {
    let cfg: Config = load_or_seed(&data.join("marketplace.json"), MARKETPLACE_DEFAULT)?;
    for s in &cfg.sources {
        if !valid_part(&s.owner) || !valid_part(&s.repo) || !valid_part(&s.git_ref) {
            return fail("bad_source", format!("fonte inválida: {}/{}@{}", s.owner, s.repo, s.git_ref));
        }
    }
    Ok(cfg.sources)
}

/// Redirects may only go to GitHub hosts (github.com and its subdomains, githubusercontent.com and its subdomains).
pub fn redirect_host_ok(host: &str) -> bool {
    host == "github.com" || host.ends_with(".github.com") || host == "githubusercontent.com" || host.ends_with(".githubusercontent.com")
}

/// HTTP client for the marketplace: a User-Agent, a timeout, at most 3 redirects, and only GitHub hosts.
pub fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Regente")
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 3 {
                return attempt.error("redirecionamentos demais");
            }
            let ok = attempt.url().scheme() == "https" && attempt.url().host_str().is_some_and(redirect_host_ok);
            if ok {
                attempt.follow()
            } else {
                attempt.error("redirecionamento para host fora do GitHub")
            }
        }))
        .build()
        .map_err(|e| AppError::new("net", e.to_string()))
}

/// Skills (folders with a `SKILL.md`) and agents (`agents/<name>.md`) of a GitHub tree. Unsafe or
/// invalid names and paths are dropped. The descriptions are filled in later by `refresh`.
pub fn items_from_tree(source: &Source, tree: &Value) -> Vec<Item> {
    let src = format!("{}/{}", source.owner, source.repo);
    let paths: Vec<&str> = tree["tree"].as_array().into_iter().flatten().filter(|e| e["type"] == "blob").filter_map(|e| e["path"].as_str()).collect();
    let mut out = vec![];
    for p in &paths {
        if let Some(dir) = p.strip_suffix("/SKILL.md") {
            let name = dir.rsplit('/').next().unwrap_or(dir);
            if !safe_rel(dir) || !valid_name(name) {
                continue;
            }
            let prefix = format!("{dir}/");
            let files = paths.iter().filter_map(|q| q.strip_prefix(&prefix)).filter(|rel| safe_rel(rel)).map(str::to_string).collect();
            out.push(Item { source: src.clone(), name: name.into(), kind: "skill".into(), dir: dir.into(), files, description: String::new() });
        } else if let Some(stem) = p.strip_prefix("agents/").and_then(|f| f.strip_suffix(".md")) {
            if !valid_name(stem) {
                continue;
            }
            out.push(Item { source: src.clone(), name: stem.into(), kind: "agent".into(), dir: "agents".into(), files: vec![format!("{stem}.md")], description: String::new() });
        }
    }
    out
}

/// Repo path of the file that carries an item's description.
fn describe_path(it: &Item) -> String {
    match it.kind.as_str() {
        "skill" => format!("{}/SKILL.md", it.dir),
        _ => format!("{}/{}", it.dir, it.files[0]),
    }
}

fn raw_url(raw: &str, source: &Source, path: &str) -> String {
    format!("{raw}/{}/{}/{}/{path}", source.owner, source.repo, source.git_ref)
}

/// Body of a GET, capped at `cap` bytes; any non-success status is an error.
async fn fetch_bytes(client: &reqwest::Client, url: &str, cap: usize) -> AppResult<Vec<u8>> {
    let mut resp = client.get(url).send().await.map_err(|e| AppError::new("net", e.to_string()))?;
    if !resp.status().is_success() {
        return fail("net", format!("HTTP {} em {url}", resp.status().as_u16()));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::new("net", e.to_string()))? {
        if buf.len() + chunk.len() > cap {
            return fail("too_big", format!("resposta acima de {cap} bytes: {url}"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn fetch_text(client: &reqwest::Client, url: &str, cap: usize) -> AppResult<String> {
    let bytes = fetch_bytes(client, url, cap).await?;
    String::from_utf8(bytes).map_err(|_| AppError::new("net", "resposta não é texto UTF-8"))
}

/// One line, at most `DESCRIPTION_MAX` chars.
fn one_line(d: &str) -> String {
    d.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(DESCRIPTION_MAX).collect()
}

/// Description of one item. `Err` means the fetch failed (the caller must not cache that as empty).
async fn describe(client: reqwest::Client, sem: Arc<tokio::sync::Semaphore>, url: String) -> Result<String, ()> {
    let Ok(_permit) = sem.acquire().await else { return Err(()) };
    let text = fetch_text(&client, &url, FILE_MAX).await.map_err(|_| ())?;
    Ok(match parse_frontmatter(&text).fields.get("description") {
        Some(FmValue::Str(d)) => one_line(d),
        _ => String::new(),
    })
}

/// Items of one source, with descriptions. The bool is false when a description could not be fetched.
async fn fetch_source(client: &reqwest::Client, api: &str, raw: &str, src: &Source) -> AppResult<(Vec<Item>, bool)> {
    let url = format!("{api}/repos/{}/{}/git/trees/{}?recursive=1", src.owner, src.repo, src.git_ref);
    let tree: Value = serde_json::from_str(&fetch_text(client, &url, TREE_MAX).await?)?;
    if tree["truncated"] == true {
        tracing::warn!("marketplace: árvore de {}/{} truncada pelo GitHub; alguns itens podem faltar", src.owner, src.repo);
    }
    let mut items = items_from_tree(src, &tree);
    let sem = Arc::new(tokio::sync::Semaphore::new(DESCRIBE_CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    for (i, it) in items.iter().enumerate() {
        let (c, s, u) = (client.clone(), sem.clone(), raw_url(raw, src, &describe_path(it)));
        set.spawn(async move { (i, describe(c, s, u).await) });
    }
    let mut complete = true;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((i, Ok(d))) => items[i].description = d,
            _ => complete = false,
        }
    }
    Ok((items, complete))
}

async fn with_timeout<T>(f: impl Future<Output = AppResult<T>>) -> AppResult<T> {
    match tokio::time::timeout(OP_TIMEOUT, f).await {
        Ok(r) => r,
        Err(_) => fail("timeout", "o marketplace demorou demais (120 s)"),
    }
}

/// Every source's items with descriptions. Cached in `data/marketplace-cache.json` for 24 h.
pub async fn refresh(data: &Path, client: &reqwest::Client, force: bool) -> AppResult<Vec<Item>> {
    with_timeout(refresh_from(data, client, API_BASE, RAW_BASE, force)).await
}

async fn refresh_from(data: &Path, client: &reqwest::Client, api: &str, raw: &str, force: bool) -> AppResult<Vec<Item>> {
    let cache_path = data.join("marketplace-cache.json");
    let wanted = sources(data)?;
    // a cache written for other sources is ignored, fresh or stale
    let cache: Option<Cache> = std::fs::read_to_string(&cache_path).ok().and_then(|t| serde_json::from_str::<Cache>(&t).ok()).filter(|c| c.sources == wanted);
    if let Some(c) = cache.as_ref().filter(|_| !force) {
        if now_ms() - c.ts < CACHE_MS {
            return Ok(c.items.clone());
        }
    }
    let mut all = vec![];
    let mut complete = true;
    for src in &wanted {
        match fetch_source(client, api, raw, src).await {
            Ok((items, ok)) => {
                complete &= ok;
                all.extend(items);
            }
            Err(e) => {
                complete = false;
                tracing::warn!("marketplace: fonte {}/{} falhou: {e}", src.owner, src.repo);
                // the stale items of this source stand in for the failed fetch
                if let Some(c) = &cache {
                    let key = format!("{}/{}", src.owner, src.repo);
                    all.extend(c.items.iter().filter(|i| i.source == key).cloned());
                }
            }
        }
    }
    // an incomplete result is not cached: the next call tries the network again
    if complete {
        if let Err(e) = save_json(&cache_path, &Cache { ts: now_ms(), sources: wanted, items: all.clone() }) {
            tracing::warn!("marketplace: cache não gravado: {e}");
        }
    }
    Ok(all)
}

/// Items whose name or description contains `q` (case-insensitive), optionally of one kind.
pub fn search(items: &[Item], q: &str, kind: Option<&str>) -> Vec<Item> {
    let q = q.to_lowercase();
    items
        .iter()
        .filter(|i| kind.is_none_or(|k| i.kind == k))
        .filter(|i| i.name.to_lowercase().contains(&q) || i.description.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Folder where an item of `kind` goes for `target`: claude, codex or regente.
pub fn target_dir(target: &str, kind: &str, data: &Path, home: &Path) -> AppResult<PathBuf> {
    Ok(match (target, kind) {
        ("claude", "skill") => home.join(".claude").join("skills"),
        ("claude", "agent") => home.join(".claude").join("agents"),
        ("codex", "skill") => home.join(".codex").join("skills"),
        ("codex", "agent") => return fail("unsupported", "o Codex não tem agentes"),
        // Antigravity (codelab oficial "Authoring Google Antigravity Skills"): skills globais em ~/.gemini/antigravity/skills
        ("antigravity", "skill") => home.join(".gemini").join("antigravity").join("skills"),
        ("antigravity", "agent") => return fail("unsupported", "o Antigravity não tem pasta pública de agentes"),
        ("regente", "skill") => data.join("skills"),
        ("regente", "agent") => data.join("agents"),
        (_, "skill" | "agent") => return fail("bad_target", "destino inválido: use claude, codex, antigravity ou regente"),
        _ => return fail("bad_kind", "tipo inválido: use skill ou agent"),
    })
}

/// Before anything is downloaded: a safe name and files, and the shape each kind must have.
fn check_item(item: &Item) -> AppResult<()> {
    let shape = match item.kind.as_str() {
        "skill" => item.files.iter().any(|f| f == "SKILL.md"),
        "agent" => item.dir == "agents" && item.files == [format!("{}.md", item.name)],
        _ => false,
    };
    if !valid_name(&item.name) || !safe_rel(&item.dir) || !shape || item.files.len() > FILES_MAX || !item.files.iter().all(|f| safe_rel(f)) {
        return fail("bad_item", format!("item inválido: {}", item.name));
    }
    Ok(())
}

async fn install_from(item: &Item, target: &str, data: &Path, home: &Path, client: &reqwest::Client, raw: &str) -> AppResult<PathBuf> {
    check_item(item)?;
    let root = target_dir(target, &item.kind, data, home)?;
    // only a configured source may be downloaded from: the item comes from a tool argument (an agent's)
    let source = match item.source.split_once('/') {
        Some((o, r)) => sources(data)?.into_iter().find(|s| s.owner == o && s.repo == r),
        None => None,
    };
    let Some(source) = source else { return fail("unknown_source", format!("fonte não configurada: {}", item.source)) };
    let dest = match item.kind.as_str() {
        "skill" => root.join(&item.name),
        _ => root.join(format!("{}.md", item.name)),
    };
    // fetch everything first: a failed download writes nothing and leaves the old install alone
    let mut files = vec![];
    let mut total = 0;
    for rel in &item.files {
        let local = match item.kind.as_str() {
            "skill" => root.join(&item.name).join(rel),
            _ => root.join(rel),
        };
        let body = fetch_bytes(client, &raw_url(raw, &source, &format!("{}/{rel}", item.dir)), FILE_MAX).await?;
        total += body.len();
        if total > INSTALL_MAX {
            return fail("too_big", "instalação acima de 10 MB");
        }
        files.push((local, body));
    }
    // the old install goes to the Recycle Bin, so files it no longer has do not mix with the new ones
    if dest.exists() {
        trash::delete(&dest).map_err(|e| AppError::new("trash", e.to_string()))?;
    }
    for (local, body) in files {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&local, body)?;
    }
    Ok(dest)
}

/// Downloads an item into the target folder, replacing its previous install.
pub async fn install(item: &Item, target: &str, data: &Path, home: &Path, client: &reqwest::Client) -> AppResult<PathBuf> {
    with_timeout(install_from(item, target, data, home, client, RAW_BASE)).await
}

pub fn installed(item: &Item, target: &str, data: &Path, home: &Path) -> bool {
    if !valid_name(&item.name) {
        return false;
    }
    let Ok(root) = target_dir(target, &item.kind, data, home) else { return false };
    match item.kind.as_str() {
        "skill" => root.join(&item.name).join("SKILL.md").is_file(),
        _ => root.join(format!("{}.md", item.name)).is_file(),
    }
}

/// Sends the installed item to the Recycle Bin.
pub fn uninstall(name: &str, kind: &str, target: &str, data: &Path, home: &Path) -> AppResult<()> {
    if !valid_name(name) {
        return fail("bad_name", "nome inválido");
    }
    let root = target_dir(target, kind, data, home)?;
    let path = match kind {
        "skill" => root.join(name),
        _ => root.join(format!("{name}.md")),
    };
    if !path.exists() {
        return fail("not_installed", format!("{name} não está instalado em {target}"));
    }
    trash::delete(&path).map_err(|e| AppError::new("trash", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/gh-tree.json")).unwrap()
    }

    fn src() -> Source {
        Source { owner: "fixture".into(), repo: "skills".into(), git_ref: "main".into() }
    }

    fn item(kind: &str, name: &str, dir: &str, files: &[&str]) -> Item {
        Item { source: "fixture/skills".into(), name: name.into(), kind: kind.into(), dir: dir.into(), files: files.iter().map(|s| s.to_string()).collect(), description: String::new() }
    }

    /// The marketplace config of a test data dir: the fixture source only.
    fn configure(data: &Path) {
        std::fs::write(data.join("marketplace.json"), r#"{"sources":[{"owner":"fixture","repo":"skills","ref":"main"}]}"#).unwrap();
    }

    #[test]
    fn items_from_tree_finds_skills_and_agents() {
        let items = items_from_tree(&src(), &tree());
        assert_eq!(items.len(), 3, "{items:?}");
        let pdf = items.iter().find(|i| i.name == "pdf").unwrap();
        assert_eq!(pdf.kind, "skill");
        assert_eq!(pdf.files, ["SKILL.md", "scripts/a.py"]);
        assert!(items.iter().any(|i| i.name == "docx" && i.kind == "skill"));
        let agent = items.iter().find(|i| i.kind == "agent").unwrap();
        assert_eq!((agent.name.as_str(), agent.dir.as_str()), ("reviewer", "agents"));
        assert!(!items.iter().any(|i| i.name == "README.md"));
    }

    #[test]
    fn unsafe_paths_and_names_are_dropped() {
        let t = serde_json::json!({"tree": [
            {"path": "skills/../evil/SKILL.md", "type": "blob"},
            {"path": "skills/Ok Name/SKILL.md", "type": "blob"},
            {"path": "agents/../x.md", "type": "blob"},
            {"path": "skills/good/SKILL.md", "type": "blob"},
            {"path": "skills/good/..\\..\\evil.txt", "type": "blob"},
            {"path": "skills/con/SKILL.md", "type": "blob"},
            {"path": "skills/good/nul.txt", "type": "blob"},
            {"path": "skills/good/tilde~1.md", "type": "blob"},
            {"path": "skills/good/trailing./x.md", "type": "blob"},
        ]});
        let items = items_from_tree(&src(), &t);
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0].files, ["SKILL.md"]);
    }

    #[test]
    fn reserved_and_odd_path_components_are_refused() {
        for p in ["skills/con/x", "a/NUL.txt", "a/com1", "a/b./c", "a/b /c", "a~/b", "a/b:c"] {
            assert!(!safe_rel(p), "{p}");
        }
        assert!(safe_rel("skills/pdf/scripts/a.py") && safe_rel(".github/x"));
    }

    #[test]
    fn redirects_only_to_github_hosts() {
        assert!(redirect_host_ok("github.com") && redirect_host_ok("objects.githubusercontent.com") && redirect_host_ok("api.github.com"));
        assert!(!redirect_host_ok("evil.example") && !redirect_host_ok("github.com.evil.example") && !redirect_host_ok("notgithub.com"));
    }

    #[test]
    fn search_by_name_and_kind() {
        let items = items_from_tree(&src(), &tree());
        assert_eq!(search(&items, "PD", None).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["pdf"]);
        let agents = search(&items, "", Some("agent"));
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "reviewer");
    }

    #[test]
    fn target_dir_maps_each_target() {
        let (data, home) = (Path::new("D/data"), Path::new("H/home"));
        assert_eq!(target_dir("claude", "skill", data, home).unwrap(), home.join(".claude").join("skills"));
        assert_eq!(target_dir("claude", "agent", data, home).unwrap(), home.join(".claude").join("agents"));
        assert_eq!(target_dir("codex", "skill", data, home).unwrap(), home.join(".codex").join("skills"));
        assert_eq!(target_dir("regente", "skill", data, home).unwrap(), data.join("skills"));
        assert_eq!(target_dir("regente", "agent", data, home).unwrap(), data.join("agents"));
        assert!(target_dir("codex", "agent", data, home).is_err());
        assert_eq!(target_dir("antigravity", "skill", data, home).unwrap(), home.join(".gemini").join("antigravity").join("skills"));
        assert!(target_dir("antigravity", "agent", data, home).is_err());
        assert!(target_dir("nada", "skill", data, home).is_err());
    }

    #[tokio::test]
    async fn traversal_is_refused_before_any_download() {
        let (data, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        configure(data.path());
        let c = client().unwrap();
        let bad = item("skill", "../escape", "skills/x", &["SKILL.md"]);
        assert!(install(&bad, "regente", data.path(), home.path(), &c).await.is_err());
        let bad_file = item("skill", "ok", "skills/x", &["../../escape.md"]);
        assert!(install(&bad_file, "regente", data.path(), home.path(), &c).await.is_err());
        // an agent must be agents/<name>.md, and a skill must carry SKILL.md
        let agent_wrong_dir = item("agent", "ok", "skills", &["ok.md"]);
        assert!(install(&agent_wrong_dir, "regente", data.path(), home.path(), &c).await.is_err());
        let skill_no_manifest = item("skill", "ok", "skills/ok", &["scripts/a.py"]);
        assert!(install(&skill_no_manifest, "regente", data.path(), home.path(), &c).await.is_err());
        // a source that is not configured is refused too
        let unknown = Item { source: "evil/x".into(), ..item("skill", "ok", "skills/x", &["SKILL.md"]) };
        assert!(install(&unknown, "regente", data.path(), home.path(), &c).await.is_err());
        assert!(!data.path().join("skills").exists());
    }

    /// Serves the fixture files of `pdf` from a local port, standing in for raw.githubusercontent.com.
    async fn fixture_server() -> String {
        let app = axum::Router::new().fallback(|uri: axum::http::Uri| async move {
            match uri.path() {
                "/fixture/skills/main/skills/pdf/SKILL.md" => (axum::http::StatusCode::OK, "---\nname: pdf\ndescription: Lê PDF\n---\ncorpo"),
                "/fixture/skills/main/skills/pdf/scripts/a.py" => (axum::http::StatusCode::OK, "print(1)"),
                _ => (axum::http::StatusCode::NOT_FOUND, ""),
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn install_writes_files_under_the_target_only() {
        let base = fixture_server().await;
        let (data, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        configure(data.path());
        let c = client().unwrap();
        let pdf = item("skill", "pdf", "skills/pdf", &["SKILL.md", "scripts/a.py"]);
        let dest = install_from(&pdf, "claude", data.path(), home.path(), &c, &base).await.unwrap();
        assert_eq!(dest, home.path().join(".claude").join("skills").join("pdf"));
        assert_eq!(std::fs::read_to_string(dest.join("scripts").join("a.py")).unwrap(), "print(1)");
        assert!(installed(&pdf, "claude", data.path(), home.path()));
        assert!(!installed(&pdf, "codex", data.path(), home.path()));

        // a missing remote file fails the whole install and writes nothing
        let broken = item("skill", "docx", "skills/docx", &["SKILL.md"]);
        assert!(install_from(&broken, "regente", data.path(), home.path(), &c, &base).await.is_err());
        assert!(!data.path().join("skills").join("docx").exists());
    }

    #[tokio::test]
    async fn stale_cache_stands_in_when_the_source_fails() {
        // every GitHub call answers 500
        let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let data = tempfile::tempdir().unwrap();
        configure(data.path());
        // an expired cache (ts 0) of the same sources
        let cached = Cache { ts: 0, sources: sources(data.path()).unwrap(), items: items_from_tree(&src(), &tree()) };
        save_json(&data.path().join("marketplace-cache.json"), &cached).unwrap();
        let got = refresh_from(data.path(), &client().unwrap(), &base, &base, false).await.unwrap();
        assert_eq!(got.len(), 3, "{got:?}");
        // the stale cache is not rewritten with the failed result
        let after: Cache = serde_json::from_str(&std::fs::read_to_string(data.path().join("marketplace-cache.json")).unwrap()).unwrap();
        assert_eq!(after.ts, 0);

        // a cache of other sources is not used
        let other = Cache { sources: vec![], ..cached };
        save_json(&data.path().join("marketplace-cache.json"), &other).unwrap();
        assert!(refresh_from(data.path(), &client().unwrap(), &base, &base, false).await.unwrap().is_empty());
    }

    #[tokio::test]
    #[ignore = "rede: consulta o GitHub de verdade"]
    async fn live_refresh() {
        let data = tempfile::tempdir().unwrap();
        let items = refresh(data.path(), &client().unwrap(), false).await.unwrap();
        assert!(!items.is_empty());
    }
}
