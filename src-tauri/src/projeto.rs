//! `.regente/projeto.json`: a configuração do projeto que o Regente lê na raiz do workspace (stacks, comandos,
//! glossário, perfil). Chaves desconhecidas são preservadas. Nenhum comando do projeto roda sem confiança (hash).
use crate::db::Workspace;
use crate::error::{fail, AppResult};
use crate::files::IGNORED;
use crate::git::is_repo;
use crate::runtime::AppState;
use crate::settings::save_json;
use crate::util::lk;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

pub const VERSAO: u64 = 1;
pub const PERFIS: [&str; 3] = ["economico", "equilibrado", "precisao"];
const ARQUIVO_MAX: u64 = 256 * 1024;
const CMD_MAX: usize = 300;
const CONFIANCA_MAX: usize = 500;
const CACHE_KEYS: [&str; 4] = ["cargoTargetCompartilhado", "nodeModulesHardlink", "venvCompartilhado", "preaquecer"];

type S<'a> = State<'a, Arc<AppState>>;

/// Preferências do projeto em Configurações. `confianca`: workspace -> hash dos comandos confiados.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjetoPrefs {
    pub auto_criar: bool,
    pub confianca: BTreeMap<String, String>,
}

impl Default for ProjetoPrefs {
    fn default() -> Self {
        Self { auto_criar: true, confianca: BTreeMap::new() }
    }
}

impl ProjetoPrefs {
    pub fn clamp(&mut self) {
        while self.confianca.len() > CONFIANCA_MAX {
            self.confianca.pop_first();
        }
    }
}

/// Comandos detectados (ou editados). Só entram no JSON os campos preenchidos.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Comandos {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub testar: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub relacionados: BTreeMap<String, String>,
}

/// Manifestos em até dois níveis (raiz = 0), sem as pastas de `files::IGNORED`. Mais rasos primeiro.
/// Devolve (pasta relativa com `/`, nome do arquivo).
fn manifests(root: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut level = vec![(String::new(), root.to_path_buf())];
    for _ in 0..=2 {
        let mut next = vec![];
        for (rel, dir) in level {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !IGNORED.contains(&name.as_str()) {
                        let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                        next.push((child, e.path()));
                    }
                } else if ft.is_file() && is_manifest(&name) {
                    out.push((rel.clone(), name));
                }
            }
        }
        level = next;
    }
    out
}

fn is_manifest(name: &str) -> bool {
    matches!(name, "Cargo.toml" | "package.json" | "pyproject.toml" | "requirements.txt" | "setup.py" | "go.mod")
        || name.ends_with(".csproj")
        || name.ends_with(".sln")
}

fn stack_of(name: &str) -> &'static str {
    match name {
        "Cargo.toml" => "rust",
        "package.json" => "node",
        "go.mod" => "go",
        n if n.ends_with(".csproj") || n.ends_with(".sln") => "dotnet",
        _ => "python",
    }
}

/// Stacks presentes na raiz ou em até dois níveis de pastas.
pub fn detect_stacks(root: &Path) -> Vec<String> {
    let set: BTreeSet<&str> = manifests(root).iter().map(|(_, n)| stack_of(n)).collect();
    set.into_iter().map(String::from).collect()
}

/// Comandos do projeto para as stacks detectadas. Node só lê o `package.json` da raiz; Rust aceita o `Cargo.toml` aninhado.
pub fn detect_commands(root: &Path, stacks: &[String]) -> Comandos {
    let has = |s: &str| stacks.iter().any(|x| x == s);
    let found = manifests(root);
    let mut c = Comandos::default();
    if has("node") {
        if let Some(pkg) = read_pkg(&root.join("package.json")) {
            let runner = if root.join("pnpm-lock.yaml").is_file() {
                "pnpm"
            } else if root.join("yarn.lock").is_file() {
                "yarn"
            } else {
                "npm"
            };
            let script = |k: &str| pkg.pointer(&format!("/scripts/{k}")).and_then(Value::as_str).filter(|s| !s.trim().is_empty());
            // o `npm init` põe um "test" que só falha; não é um comando de verdade
            if script("test").is_some_and(|t| !t.contains("no test specified")) {
                c.testar = Some(format!("{runner} test"));
            }
            if script("build").is_some() {
                c.build = Some(format!("{runner} run build"));
            }
            let has_dep = |n: &str| ["dependencies", "devDependencies"].iter().any(|k| pkg.get(*k).and_then(|d| d.get(n)).is_some());
            if has_dep("vitest") {
                c.relacionados.insert("vitest".into(), "npx vitest related {arquivos} --run".into());
            } else if has_dep("jest") {
                c.relacionados.insert("jest".into(), "npx jest --findRelatedTests {arquivos}".into());
            }
        }
    }
    if has("rust") {
        if let Some((dir, _)) = found.iter().find(|(_, n)| n == "Cargo.toml") {
            let manifest = if dir.is_empty() { "Cargo.toml".to_string() } else { format!("{dir}/Cargo.toml") };
            let flag = if dir.is_empty() { String::new() } else { format!(" --manifest-path {manifest}") };
            // o node da raiz (npm test / npm run build) vence quando existem os dois
            c.testar.get_or_insert(format!("cargo test{flag}"));
            c.build.get_or_insert(format!("cargo build{flag}"));
            c.relacionados.insert("cargo".into(), format!("cargo test{flag} {{filtro}} -q"));
        }
    }
    if has("python") {
        c.testar = Some("pytest -q".into());
        c.relacionados.insert("pytest".into(), "pytest -q {arquivos}".into());
    }
    if has("go") {
        c.testar = Some("go test ./...".into());
        c.relacionados.insert("go".into(), "go test {pacotes}".into());
    }
    if has("dotnet") {
        c.testar = Some("dotnet test".into());
        c.relacionados.insert("dotnet".into(), "dotnet test --filter {filtro}".into());
    }
    c
}

fn read_pkg(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn local_root(path: &str) -> AppResult<PathBuf> {
    if crate::remote::is_remote(path) {
        return fail("projeto_remoto", "projeto.json só existe em workspaces locais");
    }
    Ok(PathBuf::from(path))
}

/// Symlink ou junção (ponto de reparo no Windows).
pub(crate) fn e_link(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    let reparo = {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        m.file_attributes() & REPARSE_POINT != 0
    };
    #[cfg(not(windows))]
    let reparo = false;
    m.file_type().is_symlink() || reparo
}

/// `.regente` e `projeto.json` não podem ser links: o Regente não grava nem lê por eles.
pub(crate) fn guarda_regente(root: &Path) -> AppResult<()> {
    let dir = root.join(".regente");
    let casos = [
        (dir.clone(), "A pasta .regente é um link; o Regente não grava por ele."),
        (dir.join("projeto.json"), "projeto.json é um link; o Regente não grava nem lê por ele."),
    ];
    for (p, msg) in casos {
        if std::fs::symlink_metadata(&p).is_ok_and(|m| e_link(&m)) {
            return fail("regente_link", msg);
        }
    }
    Ok(())
}

/// Lê `.regente/projeto.json`: None se não existe ou não é JSON. Recusa links e arquivos acima de 256 KB.
/// Mantém as chaves desconhecidas.
pub fn load(root: &Path) -> AppResult<Option<Value>> {
    let path = root.join(".regente").join("projeto.json");
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if e_link(&meta) {
        return fail("regente_link", "projeto.json é um link; o Regente só lê arquivo comum.");
    }
    if !meta.is_file() {
        return fail("projeto_invalido", "projeto.json precisa ser um arquivo comum.");
    }
    if meta.len() > ARQUIVO_MAX {
        return fail("projeto_grande", "projeto.json tem mais de 256 KB; reduza o arquivo.");
    }
    // o limite vale de novo na leitura: o arquivo pode crescer entre o stat e o read
    let mut texto = String::new();
    std::fs::File::open(&path)?.take(ARQUIVO_MAX + 1).read_to_string(&mut texto)?;
    if texto.len() as u64 > ARQUIVO_MAX {
        return fail("projeto_grande", "projeto.json tem mais de 256 KB; reduza o arquivo.");
    }
    let mut v: Option<Value> = serde_json::from_str(&texto).ok();
    // chave de `comandos` que este Regente não conhece sai da leitura (com aviso): senão o próximo salvar falharia
    if let Some(c) = v.as_mut().and_then(|v| v.get_mut("comandos")).and_then(Value::as_object_mut) {
        let desconhecidas: Vec<String> = c.keys().filter(|k| !COMANDOS.contains(&k.as_str())).cloned().collect();
        for k in desconhecidas {
            tracing::warn!("projeto.json: comando desconhecido '{k}' ignorado");
            c.remove(&k);
        }
    }
    Ok(v)
}

/// Chaves conhecidas de `comandos`.
const COMANDOS: [&str; 3] = ["build", "testar", "relacionados"];

/// Controle e caracteres bidi/invisíveis que disfarçam o comando na tela (o que a pessoa confia tem que ser o que roda).
fn caractere_proibido(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}')
}

fn check_cmd(v: &Value) -> AppResult<()> {
    match v {
        Value::Null => Ok(()),
        Value::String(s) if s.chars().count() <= CMD_MAX && !s.chars().any(caractere_proibido) => Ok(()),
        _ => fail("projeto_invalido", "comando inválido: texto de até 300 caracteres, sem quebra de linha nem caractere de controle"),
    }
}

/// Valida o documento inteiro. O glossário passa pela mesma checagem de caminho das revisões (dentro da raiz).
pub fn validate(root: &Path, v: &Value) -> AppResult<()> {
    let Some(obj) = v.as_object() else { return fail("projeto_invalido", "projeto.json precisa ser um objeto") };
    if obj.get("versao").and_then(Value::as_u64) != Some(VERSAO) {
        return fail("projeto_invalido", "versao precisa ser 1");
    }
    if let Some(c) = obj.get("comandos").filter(|x| !x.is_null()) {
        let Some(c) = c.as_object() else { return fail("projeto_invalido", "comandos precisa ser um objeto") };
        for (k, val) in c {
            match k.as_str() {
                "build" | "testar" => check_cmd(val)?,
                "relacionados" => {
                    let Some(r) = val.as_object() else { return fail("projeto_invalido", "relacionados precisa ser um objeto") };
                    r.values().try_for_each(check_cmd)?;
                }
                outro => return fail("projeto_invalido", format!("comando desconhecido: {outro}")),
            }
        }
    }
    match obj.get("glossario").filter(|x| !x.is_null()) {
        None => {}
        Some(Value::String(g)) => {
            crate::review::checked(root, g)?;
        }
        Some(_) => return fail("projeto_invalido", "glossario precisa ser um caminho de texto"),
    }
    match obj.get("perfil").filter(|x| !x.is_null()) {
        None => {}
        Some(Value::String(p)) if PERFIS.contains(&p.as_str()) => {}
        Some(_) => return fail("projeto_invalido", "perfil desconhecido"),
    }
    if let Some(cache) = obj.get("cacheBuild").filter(|x| !x.is_null()) {
        let Some(cache) = cache.as_object() else { return fail("projeto_invalido", "cacheBuild precisa ser um objeto") };
        for k in CACHE_KEYS {
            if cache.get(k).is_some_and(|x| !x.is_boolean()) {
                return fail("projeto_invalido", format!("cacheBuild.{k} precisa ser verdadeiro ou falso"));
            }
        }
    }
    if let Some(g) = obj.get("git").filter(|x| !x.is_null()) {
        let Some(g) = g.as_object() else { return fail("projeto_invalido", "git precisa ser um objeto") };
        match g.get("base").filter(|x| !x.is_null()) {
            None => {}
            Some(Value::String(b)) if !b.is_empty() && b.chars().count() <= 100 && !b.chars().any(caractere_proibido) && crate::gitflow::check_base(b).is_ok() => {}
            Some(_) => return fail("projeto_invalido", "git.base precisa ser o nome de uma branch (ex.: dev)"),
        }
    }
    Ok(())
}

/// Valida e grava de forma atômica (tmp + rename), como `files::write_text`.
pub fn save(root: &Path, v: &Value) -> AppResult<()> {
    validate(root, v)?;
    guarda_regente(root)?;
    let dir = root.join(".regente");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("projeto.json");
    let tmp = dir.join("projeto.json.regente-tmp");
    if let Err(e) = escreve_novo(&tmp, serde_json::to_string_pretty(v)?.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Grava em arquivo novo: antes tira o que estiver no caminho (um link plantado é removido, nunca seguido) e abre com `create_new`.
pub(crate) fn escreve_novo(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)
}

/// `perfil` do arquivo, se for um id conhecido. `perfil::efetivo` (F0a) o consome.
pub fn perfil_preferido(root: &Path) -> Option<String> {
    let p = load(root).ok().flatten()?.get("perfil")?.as_str()?.to_string();
    PERFIS.contains(&p.as_str()).then_some(p)
}

/// `git.base` do projeto: a branch de onde nascem as specs e para onde vão os PRs (ex.: `dev`). None sem arquivo ou chave.
pub fn git_base(root: &Path) -> Option<String> {
    load(root).ok().flatten()?.pointer("/git/base")?.as_str().filter(|b| !b.is_empty()).map(String::from)
}

/// Canonical JSON (chaves ordenadas), para o hash não depender da ordem de inserção.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let items: Vec<String> = keys.iter().map(|k| format!("{}:{}", Value::String((*k).clone()), canonical(&m[*k]))).collect();
            format!("{{{}}}", items.join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// SHA-256 (hex) de `comandos` + `cacheBuild.preaquecer`. Muda quando um comando muda; a confiança guarda este hash.
pub fn comandos_hash(v: &Value) -> String {
    let base = json!({
        "comandos": v.get("comandos").cloned().unwrap_or(Value::Null),
        "preaquecer": v.pointer("/cacheBuild/preaquecer").cloned().unwrap_or(Value::Null),
    });
    Sha256::digest(canonical(&base).as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Confiança: o hash atual dos comandos do workspace é o que o usuário confirmou. Arquivo inválido nunca é confiável.
pub fn confiado(prefs: &ProjetoPrefs, ws_id: &str, root: &Path) -> bool {
    let Ok(Some(v)) = load(root) else { return false };
    validate(root, &v).is_ok() && prefs.confianca.get(ws_id).is_some_and(|h| *h == comandos_hash(&v))
}

/// Grava a confiança no hash que a pessoa viu. Recusa arquivo inválido, falta de hash e hash que não bate com
/// o arquivo atual (`projeto_mudou`): a tela precisa mostrar o arquivo de novo antes de confiar.
pub fn confiar(prefs: &mut ProjetoPrefs, ws_id: &str, root: &Path, hash: Option<String>) -> AppResult<()> {
    let v = load(root)?.ok_or_else(|| crate::error::AppError::new("sem_projeto", "o projeto ainda não tem .regente/projeto.json"))?;
    validate(root, &v)?;
    let Some(visto) = hash else {
        return fail("hash_ausente", "revise os comandos na tela antes de confiar");
    };
    if visto != comandos_hash(&v) {
        return fail("projeto_mudou", "O projeto.json mudou desde que você abriu; revise de novo");
    }
    prefs.confianca.insert(ws_id.into(), visto);
    Ok(())
}

/// Cria `.regente/projeto.json` com a configuração detectada, se `auto_criar` e o arquivo não existir.
/// Workspaces `ssh://` ficam de fora. Também ajusta a exclusão do git (`missions::ensure_excluded`), só quando
/// o Regente vai criar o arquivo ou a pasta `.regente` já existe. Retorna se criou o arquivo.
pub fn ensure(ws_path: &str, auto_criar: bool) -> AppResult<bool> {
    let Ok(root) = local_root(ws_path) else { return Ok(false) };
    if !root.is_dir() {
        return Ok(false);
    }
    guarda_regente(&root)?;
    if !auto_criar && !root.join(".regente").exists() {
        return Ok(false);
    }
    if is_repo(&root) {
        if let Err(e) = crate::missions::ensure_excluded(&root) {
            tracing::warn!("exclusão do .regente no git não ajustada em {ws_path}: {}", e.code());
        }
    }
    if !auto_criar || root.join(".regente").join("projeto.json").exists() {
        return Ok(false);
    }
    let stacks = detect_stacks(&root);
    let comandos = detect_commands(&root, &stacks);
    let doc = json!({
        "versao": VERSAO,
        "stacks": stacks,
        "comandos": comandos,
        "cacheBuild": {"cargoTargetCompartilhado": true, "nodeModulesHardlink": true, "venvCompartilhado": false, "preaquecer": true},
    });
    save(&root, &doc)?;
    Ok(true)
}

/// Chamado por `workspace_open`. Nunca falha a abertura: registra o erro e segue.
pub fn on_open(s: &AppState, ws: &Workspace) {
    let auto = lk(&s.settings).projeto.auto_criar;
    match ensure(&ws.path, auto) {
        Ok(true) => {
            let comandos = load(Path::new(&ws.path)).ok().flatten().and_then(|v| v.get("comandos").cloned()).unwrap_or(Value::Null);
            s.emit("projeto-criado", json!({"workspaceId": ws.id, "comandos": comandos}));
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("projeto.json não criado em {}: {}", ws.path, e.code()),
    }
}

/// Estado para a tela: o arquivo (ou null), o hash dos comandos que ele tem (o que a confiança grava), o erro
/// de leitura se houver, o que foi detectado agora e o caminho. `hash` vem da mesma leitura que `valor`.
#[tauri::command(async)]
pub fn projeto_get(s: S<'_>, workspace_id: String) -> AppResult<Value> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let root = local_root(&ws.path)?;
    let stacks = detect_stacks(&root);
    let comandos = detect_commands(&root, &stacks);
    let (valor, erro) = match load(&root) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(e.to_string())),
    };
    let hash = valor.as_ref().filter(|v| validate(&root, v).is_ok()).map(comandos_hash);
    Ok(json!({
        "valor": valor,
        "hash": hash,
        "erro": erro,
        "caminho": root.join(".regente").join("projeto.json").to_string_lossy(),
        "detectado": {"stacks": stacks, "comandos": comandos},
    }))
}

/// Caminho absoluto do glossário, pela mesma checagem de `validate` (dentro da raiz, sem `..`).
#[tauri::command(async)]
pub fn projeto_glossario(s: S<'_>, workspace_id: String, rel: String) -> AppResult<String> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let root = local_root(&ws.path)?;
    Ok(crate::review::checked(&root, &rel)?.to_string_lossy().into_owned())
}

#[tauri::command(async)]
pub fn projeto_save(s: S<'_>, workspace_id: String, value: Value) -> AppResult<()> {
    let ws = s.db.get_workspace(&workspace_id)?;
    save(&local_root(&ws.path)?, &value)
}

/// Confia (ou deixa de confiar) nos comandos do projeto. `hash` é o que a tela mostrou (`projeto_get`).
/// O arquivo é relido e validado; se o hash não bate, o arquivo mudou e a confiança não é gravada.
#[tauri::command(async)]
pub fn projeto_trust(s: S<'_>, workspace_id: String, trust: bool, hash: Option<String>) -> AppResult<()> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let root = local_root(&ws.path)?;
    let mut cur = lk(&s.settings);
    if trust {
        confiar(&mut cur.projeto, &workspace_id, &root, hash)?;
    } else {
        cur.projeto.confianca.remove(&workspace_id);
    }
    cur.projeto.clamp();
    save_json(&s.data.join("settings.json"), &*cur)
}

#[tauri::command(async)]
pub fn projeto_trusted(s: S<'_>, workspace_id: String) -> AppResult<bool> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let root = local_root(&ws.path)?;
    Ok(confiado(&lk(&s.settings).projeto, &workspace_id, &root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn put(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn detects_nested_cargo_and_root_node() {
        let d = tmp();
        put(d.path(), "package.json", r#"{"scripts":{"test":"vitest run","build":"tsc"},"devDependencies":{"vitest":"1"}}"#);
        put(d.path(), "src-tauri/Cargo.toml", "[package]\nname=\"x\"\n");
        put(d.path(), "node_modules/x/go.mod", "module x");
        let stacks = detect_stacks(d.path());
        assert_eq!(stacks, ["node", "rust"]);
        let c = detect_commands(d.path(), &stacks);
        assert_eq!(c.testar.as_deref(), Some("npm test"));
        assert_eq!(c.build.as_deref(), Some("npm run build"));
        assert_eq!(c.relacionados["vitest"], "npx vitest related {arquivos} --run");
        assert_eq!(c.relacionados["cargo"], "cargo test --manifest-path src-tauri/Cargo.toml {filtro} -q");
    }

    #[test]
    fn pnpm_lock_and_placeholder_test_script() {
        let d = tmp();
        put(d.path(), "package.json", r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1","build":"vite build"}}"#);
        put(d.path(), "pnpm-lock.yaml", "");
        let c = detect_commands(d.path(), &detect_stacks(d.path()));
        assert_eq!(c.testar, None);
        assert_eq!(c.build.as_deref(), Some("pnpm run build"));
    }

    #[test]
    fn empty_folder_has_nothing() {
        let d = tmp();
        assert!(detect_stacks(d.path()).is_empty());
        assert_eq!(detect_commands(d.path(), &[]), Comandos::default());
    }

    #[test]
    fn validate_rejects_bad_documents() {
        let d = tmp();
        let ok = json!({"versao": 1, "comandos": {"testar": "cargo test"}});
        assert!(validate(d.path(), &ok).is_ok());
        let bad = [
            json!({"versao": 2}),
            json!({"versao": 1, "comandos": {"build": "a\nb"}}),
            json!({"versao": 1, "comandos": {"build": "a\u{0}b"}}),
            json!({"versao": 1, "comandos": {"testar": "x".repeat(301)}}),
            json!({"versao": 1, "comandos": {"relacionados": {"v": "ok\r"}}}),
            json!({"versao": 1, "glossario": "/etc/passwd"}),
            json!({"versao": 1, "glossario": "..\\fora.md"}),
            json!({"versao": 1, "glossario": "../fora.md"}),
            json!({"versao": 1, "perfil": "turbo"}),
            json!({"versao": 1, "cacheBuild": {"preaquecer": "sim"}}),
            json!({"versao": 1, "comandos": {"build": "npm\u{202E}run"}}),
            json!({"versao": 1, "comandos": {"testar": "a\u{7}b"}}),
            json!({"versao": 1, "comandos": {"testar": "a\u{2028}b"}}),
        ];
        for b in bad {
            assert!(validate(d.path(), &b).is_err(), "{b}");
        }
        assert!(validate(d.path(), &json!({"versao": 1, "glossario": ".regente/glossario.md", "perfil": "precisao"})).is_ok());
    }

    #[test]
    fn unknown_keys_round_trip() {
        let d = tmp();
        let doc = json!({"versao": 1, "futuro": {"a": [1, 2]}, "comandos": {"testar": "pytest -q"}});
        save(d.path(), &doc).unwrap();
        assert_eq!(load(d.path()).unwrap().unwrap(), doc);
        assert!(!d.path().join(".regente").join("projeto.json.regente-tmp").exists());
    }

    #[test]
    fn rejects_unknown_command_keys() {
        let d = tmp();
        let doc = json!({"versao": 1, "comandos": {"testar": "pytest -q", "novo": "x"}});
        assert_eq!(validate(d.path(), &doc).unwrap_err().code(), "projeto_invalido");
        assert_eq!(save(d.path(), &doc).unwrap_err().code(), "projeto_invalido");
    }

    #[test]
    fn explicit_save_says_which_command_key_is_unknown() {
        let d = tmp();
        let erro = save(d.path(), &json!({"versao": 1, "comandos": {"novo": "x"}})).unwrap_err();
        assert!(erro.to_string().contains("novo"), "{erro}");
    }

    #[test]
    fn load_drops_unknown_command_keys_so_the_next_save_works() {
        let d = tmp();
        put(d.path(), ".regente/projeto.json", r#"{"versao":1,"comandos":{"testar":"pytest -q","novo":"x"}}"#);
        let lido = load(d.path()).unwrap().unwrap();
        assert!(lido["comandos"].get("novo").is_none());
        assert_eq!(lido["comandos"]["testar"], "pytest -q");
        save(d.path(), &lido).unwrap();
    }

    #[test]
    fn save_never_follows_a_planted_tmp_link() {
        let fora = tmp();
        let alvo = fora.path().join("alvo.txt");
        std::fs::write(&alvo, "intacto").unwrap();
        let d = tmp();
        std::fs::create_dir_all(d.path().join(".regente")).unwrap();
        if !ligar(&alvo, &d.path().join(".regente").join("projeto.json.regente-tmp"), false) {
            eprintln!("symlink indisponível; teste de link pulado");
            return;
        }
        save(d.path(), &json!({"versao": 1, "comandos": {"testar": "pytest -q"}})).unwrap();
        assert_eq!(std::fs::read_to_string(&alvo).unwrap(), "intacto");
        assert_eq!(load(d.path()).unwrap().unwrap()["comandos"]["testar"], "pytest -q");
    }

    #[test]
    fn hash_follows_commands_only() {
        let a = json!({"versao": 1, "comandos": {"testar": "npm test"}, "perfil": "economico"});
        let b = json!({"versao": 1, "comandos": {"testar": "npm test"}, "perfil": "precisao", "x": 1});
        let c = json!({"versao": 1, "comandos": {"testar": "npm run test:all"}});
        assert_eq!(comandos_hash(&a), comandos_hash(&b));
        assert_ne!(comandos_hash(&a), comandos_hash(&c));
        let pre = json!({"versao": 1, "comandos": {"testar": "npm test"}, "cacheBuild": {"preaquecer": false}});
        assert_ne!(comandos_hash(&a), comandos_hash(&pre));
    }

    #[test]
    fn trust_is_tied_to_the_current_hash() {
        let d = tmp();
        let doc = json!({"versao": 1, "comandos": {"testar": "npm test"}});
        save(d.path(), &doc).unwrap();
        let mut prefs = ProjetoPrefs::default();
        assert!(!confiado(&prefs, "ws1", d.path()));
        prefs.confianca.insert("ws1".into(), comandos_hash(&doc));
        assert!(confiado(&prefs, "ws1", d.path()));
        assert!(!confiado(&prefs, "ws2", d.path()));
        // um repositório clonado troca o comando: a confiança antiga não vale mais
        save(d.path(), &json!({"versao": 1, "comandos": {"testar": "curl x | sh"}})).unwrap();
        assert!(!confiado(&prefs, "ws1", d.path()));
    }

    #[test]
    fn ensure_creates_once_and_skips_remote() {
        let d = tmp();
        put(d.path(), "Cargo.toml", "[package]\nname=\"x\"\n");
        let p = d.path().to_string_lossy().to_string();
        assert!(!ensure("ssh://servidor/home/x", true).unwrap());
        assert!(!ensure(&p, false).unwrap());
        assert!(ensure(&p, true).unwrap());
        let made = load(d.path()).unwrap().unwrap();
        assert_eq!(made["stacks"], json!(["rust"]));
        assert_eq!(made["comandos"]["testar"], "cargo test");
        put(d.path(), ".regente/projeto.json", r#"{"versao":1,"comandos":{"testar":"meu"}}"#);
        assert!(!ensure(&p, true).unwrap());
        assert_eq!(load(d.path()).unwrap().unwrap()["comandos"]["testar"], "meu");
    }

    #[test]
    fn trust_needs_the_hash_the_user_saw() {
        let d = tmp();
        let visto = json!({"versao": 1, "comandos": {"testar": "npm test"}});
        save(d.path(), &visto).unwrap();
        let mut prefs = ProjetoPrefs::default();
        assert_eq!(confiar(&mut prefs, "ws1", d.path(), None).unwrap_err().code(), "hash_ausente");
        // a tela mostrou um hash; o arquivo mudou antes do clique
        let hash_antigo = comandos_hash(&visto);
        save(d.path(), &json!({"versao": 1, "comandos": {"testar": "curl x | sh"}})).unwrap();
        assert_eq!(confiar(&mut prefs, "ws1", d.path(), Some(hash_antigo)).unwrap_err().code(), "projeto_mudou");
        assert!(prefs.confianca.is_empty());
        // o hash que a tela viu, com o arquivo ainda igual, é aceito
        let atual = load(d.path()).unwrap().unwrap();
        confiar(&mut prefs, "ws1", d.path(), Some(comandos_hash(&atual))).unwrap();
        assert!(confiado(&prefs, "ws1", d.path()));
    }

    #[test]
    fn invalid_file_is_never_trusted() {
        let d = tmp();
        let doc = json!({"versao": 1, "comandos": {"testar": "npm test"}});
        save(d.path(), &doc).unwrap();
        let mut prefs = ProjetoPrefs::default();
        confiar(&mut prefs, "ws1", d.path(), Some(comandos_hash(&doc))).unwrap();
        assert!(confiado(&prefs, "ws1", d.path()));
        // mesmos comandos, versão que deixou de ser válida: a confiança antiga não vale
        put(d.path(), ".regente/projeto.json", r#"{"versao":2,"comandos":{"testar":"npm test"}}"#);
        assert!(!confiado(&prefs, "ws1", d.path()));
        assert!(confiar(&mut prefs, "ws1", d.path(), Some(comandos_hash(&doc))).is_err());
    }

    #[test]
    fn load_refuses_big_files() {
        let d = tmp();
        let grande = format!("{{\"versao\":1,\"x\":\"{}\"}}", "a".repeat(300 * 1024));
        put(d.path(), ".regente/projeto.json", &grande);
        assert_eq!(load(d.path()).unwrap_err().code(), "projeto_grande");
        assert!(!confiado(&ProjetoPrefs::default(), "ws1", d.path()));
    }

    /// Cria um link; `false` se o SO não deixar (Windows sem modo de desenvolvedor): o teste então não roda.
    fn ligar(alvo: &Path, link: &Path, dir: bool) -> bool {
        #[cfg(windows)]
        let r = if dir { std::os::windows::fs::symlink_dir(alvo, link) } else { std::os::windows::fs::symlink_file(alvo, link) };
        #[cfg(unix)]
        let r = {
            let _ = dir;
            std::os::unix::fs::symlink(alvo, link)
        };
        r.is_ok()
    }

    #[test]
    fn refuses_linked_projeto_and_regente_dir() {
        let fora = tmp();
        put(fora.path(), "alvo.json", r#"{"versao":1,"comandos":{"testar":"curl x | sh"}}"#);
        // projeto.json é um link para fora da pasta
        let d = tmp();
        std::fs::create_dir_all(d.path().join(".regente")).unwrap();
        if !ligar(&fora.path().join("alvo.json"), &d.path().join(".regente").join("projeto.json"), false) {
            eprintln!("symlink indisponível; teste de link pulado");
            return;
        }
        assert_eq!(load(d.path()).unwrap_err().code(), "regente_link");
        assert!(!confiado(&ProjetoPrefs::default(), "ws1", d.path()));
        assert_eq!(save(d.path(), &json!({"versao": 1})).unwrap_err().code(), "regente_link");
        // a pasta .regente é um link: nada é gravado no alvo
        let d2 = tmp();
        let destino = fora.path().join("destino");
        std::fs::create_dir_all(&destino).unwrap();
        assert!(ligar(&destino, &d2.path().join(".regente"), true));
        assert_eq!(save(d2.path(), &json!({"versao": 1})).unwrap_err().code(), "regente_link");
        assert_eq!(ensure(&d2.path().to_string_lossy(), true).unwrap_err().code(), "regente_link");
        assert!(!destino.join("projeto.json").exists());
    }

    #[test]
    fn git_base_is_validated_and_read() {
        let d = tempfile::tempdir().unwrap();
        let ok = json!({"versao": 1, "git": {"base": "dev"}});
        validate(d.path(), &ok).unwrap();
        assert_eq!(validate(d.path(), &json!({"versao": 1, "git": {"base": "a b"}})).unwrap_err().code(), "projeto_invalido");
        assert_eq!(validate(d.path(), &json!({"versao": 1, "git": "dev"})).unwrap_err().code(), "projeto_invalido");
        assert_eq!(git_base(d.path()), None);
        std::fs::create_dir_all(d.path().join(".regente")).unwrap();
        std::fs::write(d.path().join(".regente").join("projeto.json"), ok.to_string()).unwrap();
        assert_eq!(git_base(d.path()).as_deref(), Some("dev"));
    }
}
