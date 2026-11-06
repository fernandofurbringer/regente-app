//! Servidores MCP que o usuário já configurou no Claude Code: `mcpServers` do `~/.claude.json` e do `.mcp.json`
//! do projeto. Leitura só (os arquivos do usuário nunca mudam). A UI recebe nome, origem e transporte;
//! a configuração completa (env, headers, tokens) só vai para o arquivo do painel e nunca é impressa.
use crate::error::AppResult;
use crate::error::fail;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Só o que a UI mostra: nome, origem (usuário | projeto) e transporte.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub source: String,
    pub transport: String,
}

/// Servidor com a configuração completa; usado só para copiar ao arquivo do painel.
#[derive(Debug, Clone)]
pub struct UserServer {
    pub info: McpServerInfo,
    pub config: Value,
}

/// Arquivo `.claude.json`: dentro de `CLAUDE_CONFIG_DIR` quando ele está definido, senão em `home`.
pub fn claude_json_path(home: &Path) -> PathBuf {
    claude_json_in(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from), home)
}

/// Pura: o `.claude.json` a partir da pasta de config (se houver) ou da home.
pub fn claude_json_in(config_dir: Option<PathBuf>, home: &Path) -> PathBuf {
    config_dir.unwrap_or_else(|| home.to_path_buf()).join(".claude.json")
}

/// Servidores do Claude para um projeto, pela precedência do Claude Code: local (`projects[<projeto>]` do `.claude.json`),
/// depois projeto (`.mcp.json`), depois usuário (`mcpServers` do topo). Nome repetido fica com o primeiro;
/// `regente` é reservado (o do Regente é sempre o próprio).
pub fn claude_servers(claude_json: &Path, project_dir: &Path) -> Vec<UserServer> {
    let cfg = read_json(claude_json);
    let local = cfg.as_ref().map(|v| local_entries(v, project_dir)).unwrap_or_default();
    let user = cfg.as_ref().map(|v| entries(&v["mcpServers"], "usuário")).unwrap_or_default();
    let project = read_json(&project_dir.join(".mcp.json")).map(|v| entries(&v["mcpServers"], "projeto")).unwrap_or_default();
    let mut out: Vec<UserServer> = vec![];
    for s in local.into_iter().chain(project).chain(user) {
        if s.info.name != "regente" && !out.iter().any(|o| o.info.name == s.info.name) {
            out.push(s);
        }
    }
    out
}

/// Para a UI do perfil `custom`: se a IA filtra servidores e, se filtra, quais existem no projeto.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub supported: bool,
    pub servers: Vec<McpServerInfo>,
}

/// Só o Claude filtra servidores (`--strict-mcp-config`); os outros CLIs respondem "não suportado".
pub fn listing(cli: &str, claude_json: &Path, project_dir: &Path) -> Listing {
    if cli != "claude" {
        return Listing { supported: false, servers: vec![] };
    }
    Listing { supported: true, servers: claude_servers(claude_json, project_dir).into_iter().map(|s| s.info).collect() }
}

/// Os servidores escolhidos, na ordem pedida. Nome que não existe é erro (sem adivinhar).
pub fn pick(servers: &[UserServer], keep: &[String]) -> AppResult<Vec<UserServer>> {
    keep.iter()
        .map(|n| match servers.iter().find(|s| &s.info.name == n) {
            Some(s) => Ok(s.clone()),
            None => fail("bad_mcp_server", format!("servidor MCP {n} não está na configuração do Claude")),
        })
        .collect()
}

/// Pura: dos nomes escolhidos, os que ainda estão na configuração (`keep`) e os que sumiram (`gone`).
pub fn split_available(all: &[UserServer], wanted: &[String]) -> (Vec<String>, Vec<String>) {
    wanted.iter().cloned().partition(|n| all.iter().any(|s| &s.info.name == n))
}

/// Recibo do perfil: o que ficou e o que saiu, só por nome.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub profile: String,
    pub kept: Vec<String>,
    pub excluded: Vec<String>,
}

/// Pura: `regente` sempre fica; dos servidores do usuário, fica o que foi escolhido e sai o resto.
pub fn receipt(profile: &str, all: &[UserServer], kept: &[UserServer]) -> Receipt {
    let mut names = vec!["regente".to_string()];
    names.extend(kept.iter().map(|s| s.info.name.clone()));
    let excluded = all.iter().filter(|s| !kept.iter().any(|k| k.info.name == s.info.name)).map(|s| s.info.name.clone()).collect();
    Receipt { profile: profile.into(), kept: names, excluded }
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Escopo local: `projects[<raiz do projeto>].mcpServers` no `.claude.json`. A chave é comparada sem caixa e sem diferença de barras.
fn local_entries(cfg: &Value, project_dir: &Path) -> Vec<UserServer> {
    let key = path_key(&project_root(project_dir).to_string_lossy());
    let Some(projects) = cfg["projects"].as_object() else { return vec![] };
    projects.iter().find(|(k, _)| path_key(k) == key).map(|(_, p)| entries(&p["mcpServers"], "local")).unwrap_or_default()
}

/// Raiz do repositório git que contém a pasta (sobe até achar `.git`); fora de git, a própria pasta.
fn project_root(dir: &Path) -> PathBuf {
    let mut cur = dir.to_path_buf();
    loop {
        if cur.join(".git").exists() {
            return cur;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => return dir.to_path_buf(),
        }
    }
}

/// Caminho comparável: barras viradas para `\`, sem barra final, minúsculas (o Windows não diferencia caixa).
fn path_key(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Só os servidores de usuário (`mcpServers` do topo) de um `.claude.json`, sem o `regente`.
pub fn user_level(claude_json: &Path) -> Vec<UserServer> {
    read_json(claude_json).map(|v| entries(&v["mcpServers"], "usuário")).unwrap_or_default().into_iter().filter(|s| s.info.name != "regente").collect()
}

fn entries(v: &Value, source: &str) -> Vec<UserServer> {
    let Some(map) = v.as_object() else { return vec![] };
    map.iter()
        .filter(|(_, c)| c.is_object())
        .map(|(name, c)| UserServer { info: McpServerInfo { name: name.clone(), source: source.into(), transport: transport(c) }, config: c.clone() })
        .collect()
}

/// Transporte pelo campo `type`; sem ele, `command` é stdio e `url` é http.
fn transport(c: &Value) -> String {
    match c["type"].as_str() {
        Some(t) => t.into(),
        None if c.get("command").is_some() => "stdio".into(),
        None if c.get("url").is_some() => "http".into(),
        None => "desconhecido".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(p: &Path, v: Value) {
        std::fs::write(p, v.to_string()).unwrap();
    }

    #[test]
    fn a_name_in_several_scopes_keeps_local_then_project_then_user() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let home = d.path().join(".claude.json");
        write(&home, json!({
            "mcpServers": {"x": {"command": "do-usuario"}, "so-user": {"command": "u"}},
            "projects": {repo.to_string_lossy().to_string(): {"mcpServers": {"x": {"command": "do-local"}}}}
        }));
        write(&repo.join(".mcp.json"), json!({"mcpServers": {"x": {"command": "do-projeto"}}}));
        let s = claude_servers(&home, &repo);
        let x: Vec<_> = s.iter().filter(|u| u.info.name == "x").collect();
        assert_eq!(x.len(), 1, "um servidor por nome");
        assert_eq!(x[0].info.source, "local", "local vence");
        assert_eq!(x[0].config["command"], "do-local");
        assert!(s.iter().any(|u| u.info.name == "so-user"), "os nomes só do usuário continuam");
        // sem local, o do projeto vence o do usuário
        write(&home, json!({"mcpServers": {"x": {"command": "do-usuario"}}}));
        let s = claude_servers(&home, &repo);
        assert_eq!(s.iter().find(|u| u.info.name == "x").unwrap().info.source, "projeto");
    }

    #[test]
    fn reads_project_then_user_and_keeps_only_names_in_info() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join(".claude.json");
        let proj = d.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        write(&home, json!({"mcpServers": {"github": {"type": "http", "url": "https://x", "headers": {"Authorization": "Bearer SECRET"}}, "regente": {"command": "x"}}}));
        write(&proj.join(".mcp.json"), json!({"mcpServers": {"db": {"command": "node", "env": {"PASS": "SECRET"}}, "github": {"command": "other"}}}));
        let s = claude_servers(&home, &proj);
        let names: Vec<_> = s.iter().map(|x| x.info.name.as_str()).collect();
        assert_eq!(names.len(), 2, "regente is reserved and duplicates keep one");
        let github = s.iter().find(|x| x.info.name == "github").unwrap();
        assert_eq!((github.info.source.as_str(), github.info.transport.as_str()), ("projeto", "stdio"), "projeto vence usuário");
        let db = s.iter().find(|x| x.info.name == "db").unwrap();
        assert_eq!(db.info.source, "projeto");
        // a UI só recebe a info: nada de env ou headers
        let shown = serde_json::to_string(&s.iter().map(|x| &x.info).collect::<Vec<_>>()).unwrap();
        assert!(!shown.contains("SECRET"));
    }

    #[test]
    fn local_scope_is_the_projects_entry_matched_by_repo_root_and_case() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let sub = repo.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let home = d.path().join(".claude.json");
        // o Claude guarda a chave com outra caixa e com barras invertidas
        let stored = repo.to_string_lossy().to_uppercase().replace('\\', "/");
        write(&home, json!({"mcpServers": {"usuario": {"command": "u"}}, "projects": {
            "C:/outro": {"mcpServers": {"fora": {"command": "f"}}},
            stored: {"mcpServers": {"local-srv": {"type": "http", "url": "https://x"}}}
        }}));
        let s = claude_servers(&home, &sub);
        let names: Vec<_> = s.iter().map(|x| (x.info.name.as_str(), x.info.source.as_str())).collect();
        assert_eq!(names, [("local-srv", "local"), ("usuario", "usuário")]);
        assert_eq!(s[0].info.transport, "http");
    }

    #[test]
    fn outside_git_the_exact_folder_is_the_project() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("solto");
        std::fs::create_dir_all(&proj).unwrap();
        let home = d.path().join(".claude.json");
        write(&home, json!({"projects": {proj.to_string_lossy().to_string(): {"mcpServers": {"a": {"command": "x"}}}}}));
        assert_eq!(claude_servers(&home, &proj)[0].info.source, "local");
        assert!(claude_servers(&home, &d.path().join("outra")).is_empty());
    }

    #[test]
    fn a_removed_server_is_split_out_so_a_respawn_keeps_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join(".claude.json");
        write(&home, json!({"mcpServers": {"a": {"command": "x"}, "b": {"command": "y"}}}));
        let all = claude_servers(&home, d.path());
        let (keep, gone) = split_available(&all, &["a".to_string(), "sumiu".to_string()]);
        assert_eq!((keep, gone), (vec!["a".to_string()], vec!["sumiu".to_string()]));
    }

    #[test]
    fn config_dir_moves_the_claude_json() {
        let home = Path::new("C:/Users/eu");
        assert_eq!(claude_json_in(None, home), home.join(".claude.json"));
        assert_eq!(claude_json_in(Some(PathBuf::from("C:/cfg")), home), Path::new("C:/cfg").join(".claude.json"));
    }

    #[test]
    fn missing_or_broken_files_give_no_servers() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".claude.json"), "{ quebrado").unwrap();
        assert!(claude_servers(&d.path().join(".claude.json"), d.path()).is_empty());
        assert!(claude_servers(&d.path().join("nao-existe.json"), d.path()).is_empty());
    }

    #[test]
    fn pick_refuses_unknown_names() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join(".claude.json");
        write(&home, json!({"mcpServers": {"a": {"command": "x"}, "b": {"command": "y"}}}));
        let all = claude_servers(&home, d.path());
        let got = pick(&all, &["b".to_string()]).unwrap();
        assert_eq!(got[0].info.name, "b");
        assert!(pick(&all, &["zz".to_string()]).is_err());
    }

    #[test]
    fn only_claude_offers_the_filter() {
        let d = tempfile::tempdir().unwrap();
        assert!(!listing("codex", &d.path().join("x.json"), d.path()).supported);
        assert!(listing("claude", &d.path().join("x.json"), d.path()).supported);
    }

    #[test]
    fn receipt_lists_kept_and_excluded_by_name() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join(".claude.json");
        write(&home, json!({"mcpServers": {"a": {"command": "x"}, "b": {"command": "y"}, "c": {"command": "z"}}}));
        let all = claude_servers(&home, d.path());
        let kept = pick(&all, &["c".to_string()]).unwrap();
        let r = receipt("custom", &all, &kept);
        assert_eq!(r.kept, ["regente", "c"]);
        assert_eq!(r.excluded, ["a", "b"]);
        let minimal = receipt("minimal", &all, &[]);
        assert_eq!(minimal.kept, ["regente"]);
        assert_eq!(minimal.excluded, ["a", "b", "c"]);
    }
}
