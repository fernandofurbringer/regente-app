use crate::error::AppResult;
use crate::projeto::ProjetoPrefs;
use crate::providers::ProvidersConfig;
use serde_json::Value;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const PROVIDERS_DEFAULT: &str = include_str!("../resources/providers.default.json");
pub const PRICES_DEFAULT: &str = include_str!("../resources/prices.default.json");

/// Stack padrão do terminal: Nerd Font primeiro (ícones do oh-my-posh/Starship). Espelho em `src/lib/terminalFont.ts`.
pub const TERMINAL_FONT_DEFAULT: &str = "'FiraCode Nerd Font', 'FiraCode Nerd Font Mono', 'CaskaydiaCove Nerd Font', 'CaskaydiaMono Nerd Font', 'JetBrainsMono Nerd Font', 'MesloLGS NF', 'Hack Nerd Font', 'Symbols Nerd Font Mono', 'Cascadia Mono', Consolas, monospace";
/// Padrão antigo: quem ainda tem exatamente esta string é migrado para `TERMINAL_FONT_DEFAULT`.
pub const TERMINAL_FONT_OLD_DEFAULT: &str = "'Cascadia Mono', Consolas, monospace";
/// Linhas de histórico por terminal (xterm): padrão 5000; faixa 1000 a 20000.
pub const SCROLLBACK_DEFAULT: u32 = 5000;

/// Limita o histórico do terminal à faixa aceita pela interface.
/// Ajustes do painel Execuções (E1b). Os limites são aplicados em `settings_set`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecucoesPrefs {
    /// Promover execuções a painel próprio (segundo plano, comando longo ou tempo do limite).
    pub promover: bool,
    /// Segundos rodando até promover.
    pub promover_apos_s: u64,
    /// Segundos após um sucesso até fechar o painel promovido. 0 = mantém.
    pub fechar_ok_s: u64,
    /// Painéis promovidos abertos por painel do Claude.
    pub max_promovidos: u8,
    /// CLIs sem execuções (ex.: "claude").
    pub desligado_clis: Vec<String>,
}

impl Default for ExecucoesPrefs {
    fn default() -> Self {
        Self { promover: false, promover_apos_s: 5, fechar_ok_s: 20, max_promovidos: 6, desligado_clis: vec![] }
    }
}

/// Voz local (Supertonic 3): voz, velocidade e qualidade (passos). Os limites ficam em `clamp`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct VozLocalPrefs {
    pub voz: String,
    pub velocidade: f32,
    pub passos: u8,
}

impl Default for VozLocalPrefs {
    fn default() -> Self {
        Self { voz: "F1".into(), velocidade: 1.05, passos: 8 }
    }
}

impl VozLocalPrefs {
    pub fn clamp(&mut self) {
        if !crate::voz_local::VOZES.contains(&self.voz.as_str()) {
            self.voz = "F1".into();
        }
        self.velocidade = if self.velocidade.is_finite() { self.velocidade.clamp(0.8, 1.3) } else { 1.05 };
        if !matches!(self.passos, 4 | 8 | 16) {
            self.passos = 8;
        }
    }
}

pub fn clamp_scrollback(n: u32) -> u32 {
    n.clamp(1000, 20000)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    pub terminal_font: String,
    pub terminal_font_size: u16,
    /// Linhas de histórico do terminal (ver `clamp_scrollback`).
    pub scrollback: u32,
    pub default_shell: String,
    pub onboarding_done: bool,
    /// cli id -> names of the extra accounts (the "principal" account is implicit)
    pub accounts: BTreeMap<String, Vec<String>>,
    /// Proxy HTTPS por conta extra, chave `cli/conta`: vira `HTTPS_PROXY` só nos painéis dessa conta.
    pub account_proxy: BTreeMap<String, String>,
    pub spawn_concurrency: usize,
    /// Workspaces (ids) with memory off: their brief is not injected into the spawn.
    pub memory_disabled: Vec<String>,
    /// Workspaces (ids) em modo sensível: rotas com segredo do Cofre não abrem painéis, e a tela de detalhes mascara os valores do Cofre.
    pub sensitive_workspaces: Vec<String>,
    /// Favoritos do catálogo: `agent:<nome>`, `skill:<nome>` ou `recipe:<id>`. Aparecem primeiro nas listas.
    pub favorites: Vec<String>,
    /// Cards de início rápido que o usuário fechou (ids de `quickstart.ts`).
    pub quickstart_dismissed: Vec<String>,
    pub auto_hibernate: AutoHibernate,
    /// Pre-allows the Regente MCP tools (`mcp__regente__*`) in the Claude panes, so they don't ask before each call.
    pub allow_regente_tools: bool,
    /// Comandos de shell (Bash/PowerShell) chegam ao painel Execuções pelos hooks Pre/PostToolUse (E1.4). Desligado: sem esses hooks.
    pub execucoes: bool,
    /// Painel Execuções: promoção a painel próprio, limites e CLIs sem execuções (`execucoes.rs`).
    pub execucoes_prefs: ExecucoesPrefs,
    /// Um painel por subagente do Claude (SubagentStart/Stop). Desligado: os subagentes não viram painel.
    pub subagent_panes: bool,
    /// Segundos até fechar um subagente concluído. 0 = mantém aberto.
    pub subagent_close_secs: u64,
    /// Bloco B: subagentes e comandos ficam dentro do painel do Claude (faixa de atividade). Marca que a migração
    /// dos padrões já rodou uma vez; quem voltar ao painel próprio depois não é migrado de novo.
    /// `serde(default)` no campo: chave ausente (arquivo antigo) = false, e a migração roda; `Settings::default()` = true.
    #[serde(default)]
    pub inline_v1: bool,
    /// Canal das atualizações (URL do latest.json). Vazio = sem canal configurado; ver `update.rs`.
    pub update_endpoint: String,
    /// Atalhos da janela editados pelo usuário: ação -> combo. Ausente = padrão (ver `src/lib/shortcuts.ts`).
    pub shortcuts: BTreeMap<String, String>,
    /// Avisos do Windows (ver `notify.rs`).
    pub notify: NotifyPrefs,
    /// Orçamento de custo (ver `budget.rs`).
    pub budget: Budget,
    // ▼ trilha A
    /// Perfis de trabalho (ver `perfil.rs`): padrão, edições dos presets e ajustes por workspace.
    pub perfis: crate::perfil::PerfisPrefs,
    /// Piloto automático de eficiência (T1, ver `autoajuste.rs`): modo e o que foi aprendido.
    pub auto_ajuste: crate::autoajuste::AutoAjustePrefs,
    // ▲ trilha A
    // ▼ trilha B
    /// Reabre as abas e restaura os painéis da véspera sem perguntar (banner em `RestorePrompt.tsx`).
    pub restore_on_open: bool,
    /// Leitor de tela no terminal (`screenReaderMode` do xterm): o buffer vira texto para o leitor, com custo de desempenho.
    pub screen_reader_mode: bool,
    /// Checkpoints também na raiz do workspace (só se for repositório). Desligado por padrão (`checkpoints.rs`).
    pub checkpoints_raiz: bool,
    /// Memória: sugestões automáticas e expiração (ver `memory.rs`).
    pub memoria: crate::memory::MemoriaPrefs,
    /// Busca por significado (M1c): ligada só por `embed_set_ativo`, depois da instalação (ver `embed.rs`).
    pub busca_semantica: crate::embed::EmbedPrefs,
    /// Voz local Supertonic 3 (ver `voz_local`): voz, velocidade e qualidade.
    pub voz_local: VozLocalPrefs,
    // ▲ trilha B
    // ▼ trilha C
    pub projeto: ProjetoPrefs,
    /// "Enquanto você estava fora" (A1.6): minutos sem foco para o digest aparecer (ver `away.rs`).
    pub ausencia: crate::away::AusenciaPrefs,
    // ▲ trilha C
}

/// Limites em US$ (diário e por missão). `None` = sem limite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Budget {
    pub daily_usd: Option<f64>,
    pub mission_usd: Option<f64>,
    /// Ao atingir o limite da missão, ela não abre painel novo até o usuário aumentar o limite.
    pub pause_on_limit: bool,
}

/// Quais avisos do Windows existem. Ligados por padrão.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct NotifyPrefs {
    /// Agente de IA terminou um trabalho de pelo menos 20 s.
    pub finished: bool,
    /// Agente de IA fez uma pergunta.
    pub asks: bool,
    /// Gate novo aguardando decisão.
    pub gates: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self { finished: true, asks: true, gates: true }
    }
}

/// Favoritos válidos: só `agent:`, `skill:` ou `recipe:` com nome curto, sem repetir, até 500.
pub fn clean_favorites(list: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in list {
        let ok = f.len() <= 160 && ["agent:", "skill:", "recipe:"].iter().any(|p| f.starts_with(p) && f.len() > p.len());
        if ok && !out.contains(&f) {
            out.push(f);
        }
        if out.len() == 500 {
            break;
        }
    }
    out
}

/// Atalho salvo: id curto e combo de texto, sem controles; combos vazios ou gigantes somem.
pub fn clean_shortcuts(map: &mut BTreeMap<String, String>) {
    map.retain(|id, combo| {
        let ok = |s: &str, max: usize| !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control);
        ok(id, 40) && ok(combo, 40)
    });
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            terminal_font: TERMINAL_FONT_DEFAULT.into(),
            terminal_font_size: 13,
            scrollback: SCROLLBACK_DEFAULT,
            default_shell: "powershell".into(),
            onboarding_done: false,
            accounts: BTreeMap::new(),
            account_proxy: BTreeMap::new(),
            spawn_concurrency: 4,
            memory_disabled: vec![],
            sensitive_workspaces: vec![],
            favorites: vec![],
            quickstart_dismissed: vec![],
            auto_hibernate: AutoHibernate::default(),
            allow_regente_tools: true,
            execucoes: true,
            execucoes_prefs: ExecucoesPrefs::default(),
            subagent_panes: false,
            subagent_close_secs: 30,
            inline_v1: true,
            update_endpoint: String::new(),
            shortcuts: BTreeMap::new(),
            notify: NotifyPrefs::default(),
            budget: Budget::default(),
            // ▼ trilha A
            perfis: crate::perfil::PerfisPrefs::default(),
            auto_ajuste: crate::autoajuste::AutoAjustePrefs::default(),
            // ▲ trilha A
            // ▼ trilha B
            restore_on_open: false,
            screen_reader_mode: false,
            checkpoints_raiz: false,
            memoria: crate::memory::MemoriaPrefs::default(),
            busca_semantica: crate::embed::EmbedPrefs::default(),
            voz_local: VozLocalPrefs::default(),
            // ▲ trilha B
            // ▼ trilha C
            projeto: ProjetoPrefs::default(),
            ausencia: crate::away::AusenciaPrefs::default(),
            // ▲ trilha C
        }
    }
}

/// Automatic hibernation (experimental, off by default): only Claude panes, see `hibernate.rs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoHibernate {
    pub enabled: bool,
    /// A pane must be idle this long (no input, no output) before it can hibernate.
    pub idle_minutes: u64,
    /// Never hibernate below this many open Claude panes.
    pub resident_floor: usize,
}

impl Default for AutoHibernate {
    fn default() -> Self {
        Self { enabled: false, idle_minutes: 10, resident_floor: 2 }
    }
}

/// Remove comentários `//` e `/* */` fora de strings (o settings.json do Windows Terminal aceita comentários).
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && chars.peek() == Some(&'/') {
            while chars.peek().is_some_and(|&n| n != '\n') {
                chars.next();
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = ' ';
            for n in chars.by_ref() {
                if prev == '*' && n == '/' {
                    break;
                }
                prev = n;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Nome de fonte seguro para o CSS `font-family`: sem aspas, `;`, chaves, barra invertida ou controles; até 64 chars.
pub fn sanitize_font_face(face: &str) -> Option<String> {
    let f = face.trim();
    if f.is_empty() || f.chars().any(|c| matches!(c, '\'' | '"' | ';' | '{' | '}' | '\\') || c.is_control()) {
        return None;
    }
    Some(f.chars().take(64).collect())
}

/// Face de fonte do settings.json do Windows Terminal: o perfil padrão, depois `profiles.defaults`.
/// Aceita `font.face` e o legado `fontFace`. Qualquer erro vira None.
pub fn wt_font_face(json_text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(&strip_jsonc(json_text)).ok()?;
    let face_of = |o: &Value| o["font"]["face"].as_str().or(o["fontFace"].as_str()).and_then(sanitize_font_face);
    let default_guid = v["defaultProfile"].as_str();
    let profile = default_guid.and_then(|g| v["profiles"]["list"].as_array()?.iter().find(|p| p["guid"].as_str() == Some(g)));
    profile.and_then(face_of).or_else(|| face_of(&v["profiles"]["defaults"]))
}

/// Lê (só leitura) o settings.json do Windows Terminal instalado, Stable ou Preview.
pub fn wt_face_from_disk() -> Option<String> {
    let packages = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Packages");
    std::fs::read_dir(packages).ok()?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("Microsoft.WindowsTerminal")).find_map(|e| {
        let text = std::fs::read_to_string(e.path().join("LocalState").join("settings.json")).ok()?;
        wt_font_face(&text)
    })
}

/// Stack do terminal para uma face (ou o padrão, sem face).
pub fn font_stack(face: Option<&str>) -> String {
    match face.and_then(sanitize_font_face) {
        Some(f) => format!("'{f}', {TERMINAL_FONT_DEFAULT}"),
        None => TERMINAL_FONT_DEFAULT.to_owned(),
    }
}

/// Primeiro boot (sem settings.json) ou fonte no padrão antigo: herda a face do Windows Terminal.
/// Fonte customizada fica como está. Devolve true se mudou.
pub fn migrate_terminal_font(s: &mut Settings, existed: bool, wt_face: impl FnOnce() -> Option<String>) -> bool {
    if existed && s.terminal_font != TERMINAL_FONT_OLD_DEFAULT {
        return false;
    }
    let next = font_stack(wt_face().as_deref());
    let changed = s.terminal_font != next;
    s.terminal_font = next;
    changed
}

/// Bloco B, uma vez: ajustes gravados antes dele passam a subagentes e comandos dentro do painel. Devolve true se mudou.
pub fn migrate_inline(s: &mut Settings) -> bool {
    if s.inline_v1 {
        return false;
    }
    s.subagent_panes = false;
    s.execucoes_prefs.promover = false;
    s.inline_v1 = true;
    true
}

/// Reads JSON; a missing or invalid file returns Default (settings never block the boot).
pub fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(text) = std::fs::read_to_string(path) else { return T::default() };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!("{} inválido ({e}); copiando para .bad e usando padrão", path.display());
        let mut bad = path.as_os_str().to_owned();
        bad.push(".bad");
        let _ = std::fs::copy(path, bad);
        T::default()
    })
}

/// Reads JSON; if the file is missing, writes `default_json` first. Invalid JSON is an error
/// (the user's file is never overwritten).
pub fn load_or_seed<T: DeserializeOwned>(path: &Path, default_json: &str) -> AppResult<T> {
    if !path.exists() {
        std::fs::write(path, default_json)?;
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// Fields added to CLI definitions after F1; an F1 `providers.json` gets them from the defaults.
const NEW_CLI_FIELDS: [&str; 5] = ["promptArgs", "systemArgs", "disallowArgs", "mcp", "pluginArgs"];

/// Proxy HTTPS de uma conta. Vazio = sem proxy. Só http/https com host, e sem usuário nem senha na URL:
/// credenciais não ficam guardadas nem aparecem no ambiente que a UI mostra.
pub fn clean_proxy(raw: &str) -> AppResult<String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    let url = reqwest::Url::parse(t).map_err(|_| crate::error::AppError::new("bad_proxy", "proxy inválido: use http:// ou https:// com um host"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return crate::error::fail("bad_proxy", "proxy inválido: use http:// ou https:// com um host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return crate::error::fail("bad_proxy", "proxy com usuário e senha não é aceito: remova as credenciais da URL");
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// `HTTPS_PROXY` of the extra account `acc` of `cli`, if one is set.
pub fn proxy_for(map: &BTreeMap<String, String>, cli: &str, acc: &str) -> Option<String> {
    map.get(&format!("{cli}/{acc}")).filter(|u| !u.is_empty()).cloned()
}

/// Copies the new CLI fields from the default (matched by `id`) into the user's objects, only where the
/// user has no such key. Routes the user lacks are appended, and so is the install command of a CLI whose
/// install is null, but only the first time each default is seen: `seenDefaults` (`route:<id>`,
/// `install:<cli>`, `cost:<cli>`) records what was merged, so a default the user deleted or cleared does not come back.
/// A file without the list (F1/F2) merges as before, once. Returns true if anything changed.
pub fn merge_new_fields(user: &mut Value, default: &Value) -> bool {
    let mut changed = false;
    let mut seen: Vec<String> = user["seenDefaults"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let before = seen.len();
    if let (Some(user_routes), Some(default_routes)) = (user.get_mut("routes").and_then(Value::as_array_mut), default.get("routes").and_then(Value::as_array)) {
        for route in default_routes {
            let Some(id) = route["id"].as_str() else { continue };
            let key = format!("route:{id}");
            if !seen.contains(&key) {
                if !user_routes.iter().any(|u| u["id"].as_str() == Some(id)) {
                    user_routes.push(route.clone());
                    changed = true;
                }
                seen.push(key);
            }
        }
    }
    if let (Some(user_clis), Some(default_clis)) = (user.get_mut("clis").and_then(Value::as_array_mut), default.get("clis").and_then(Value::as_array)) {
        for cli in user_clis.iter_mut() {
            let Some(obj) = cli.as_object_mut() else { continue };
            let Some(id) = obj.get("id").and_then(Value::as_str).map(str::to_owned) else { continue };
            let Some(def) = default_clis.iter().find(|d| d["id"].as_str() == Some(id.as_str())) else { continue };
            let key = format!("install:{id}");
            // a CLI with no install command in the user file gets the verified one from the defaults, once
            if !seen.contains(&key) {
                if obj.get("install").is_some_and(Value::is_null) && def["install"].is_string() {
                    obj.insert("install".into(), def["install"].clone());
                    changed = true;
                }
                seen.push(key);
            }
            // um CLI sem fonte de custo no arquivo do usuário ganha a dos padrões, uma vez só
            let cost_key = format!("cost:{id}");
            if !seen.contains(&cost_key) {
                if obj.get("cost").is_some_and(Value::is_null) && def["cost"].is_string() {
                    obj.insert("cost".into(), def["cost"].clone());
                    changed = true;
                }
                seen.push(cost_key);
            }
            for field in NEW_CLI_FIELDS {
                if !obj.contains_key(field) {
                    if let Some(v) = def.get(field) {
                        obj.insert(field.into(), v.clone());
                        changed = true;
                    }
                }
            }
        }
    }
    if seen.len() != before {
        if let Some(obj) = user.as_object_mut() {
            obj.insert("seenDefaults".into(), serde_json::json!(seen));
        }
        changed = true;
    }
    changed
}

/// Loads `providers.json`: seeds it when missing, migrates F1 files in place (new CLI fields only) and
/// falls back to the built-in defaults on invalid JSON, leaving that file untouched.
pub fn load_providers(path: &Path) -> ProvidersConfig {
    let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).expect("valid default");
    // Some(merged) only when the file needs the migration; nothing is written before the config parses.
    let loaded = (|| -> AppResult<(ProvidersConfig, Option<Value>)> {
        if !path.exists() {
            std::fs::write(path, PROVIDERS_DEFAULT)?;
        }
        let mut user: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let changed = merge_new_fields(&mut user, &default);
        let cfg = serde_json::from_value(user.clone())?;
        Ok((cfg, changed.then_some(user)))
    })();
    match loaded {
        Ok((cfg, merged)) => {
            if let Some(merged) = merged {
                if let Err(e) = save_json(path, &merged) {
                    tracing::warn!("não foi possível migrar {}: {e}", path.display());
                }
            }
            cfg
        }
        Err(e) => {
            tracing::error!("{} inválido, usando padrão: {e}", path.display());
            serde_json::from_str(PROVIDERS_DEFAULT).expect("valid default")
        }
    }
}

/// Atomic write: tmp + rename.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("{}.tmp", crate::util::new_id()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_proxy_accepts_http_and_https_hosts_only_without_credentials() {
        assert_eq!(clean_proxy("  http://proxy.local:3128/ ").unwrap(), "http://proxy.local:3128");
        assert_eq!(clean_proxy("").unwrap(), "");
        for bad in ["socks5://h:1080", "proxy.local:3128", "http://", "https://user:senha@proxy.local", "http://u@proxy.local"] {
            assert_eq!(clean_proxy(bad).unwrap_err().code(), "bad_proxy", "{bad}");
        }
        let mut map = BTreeMap::new();
        map.insert("claude/trabalho".to_string(), "http://p:1".to_string());
        assert_eq!(proxy_for(&map, "claude", "trabalho").as_deref(), Some("http://p:1"));
        assert_eq!(proxy_for(&map, "claude", "principal"), None);
    }

    #[test]
    fn an_old_providers_file_gets_the_new_routes_without_losing_its_own() {
        let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        let mut user = serde_json::json!({"clis": [{"id": "cursor", "install": null}], "routes": [{"id": "openrouter", "label": "meu", "baseUrl": "https://x", "secret": null, "models": []}]});
        assert!(merge_new_fields(&mut user, &default));
        let ids: Vec<&str> = user["routes"].as_array().unwrap().iter().filter_map(|r| r["id"].as_str()).collect();
        assert!(ids.contains(&"fireworks") && ids.contains(&"mimo") && ids.contains(&"llamacpp"), "{ids:?}");
        assert_eq!(user["routes"][0]["label"], "meu");
        assert!(user["clis"][0]["install"].as_str().is_some_and(|c| c.contains("cursor.com/install")), "{user}");
        assert!(!merge_new_fields(&mut user, &default), "second merge adds nothing");
    }

    #[test]
    fn an_old_opencode_without_cost_gets_the_default_once() {
        let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        assert_eq!(default["clis"].as_array().unwrap().iter().find(|c| c["id"] == "opencode").unwrap()["cost"], "opencode");
        let mut user = serde_json::json!({"clis": [{"id": "opencode", "install": "npm i -g opencode-ai", "cost": null}], "routes": []});
        assert!(merge_new_fields(&mut user, &default));
        assert_eq!(user["clis"][0]["cost"], "opencode");
        // o usuário desligou o custo depois: não volta
        user["clis"][0]["cost"] = Value::Null;
        assert!(!merge_new_fields(&mut user, &default));
        assert!(user["clis"][0]["cost"].is_null());
    }

    #[test]
    fn favorites_keep_only_known_prefixes_once() {
        let got = clean_favorites(vec!["agent:a".into(), "agent:a".into(), "x:y".into(), "skill:".into(), "recipe:r1".into()]);
        assert_eq!(got, vec!["agent:a".to_string(), "recipe:r1".to_string()]);
    }


    #[test]
    fn missing_file_gives_default_and_partial_json_fills_defaults() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        assert_eq!(load_or_default::<Settings>(&p), Settings::default());
        std::fs::write(&p, r#"{"theme":"light"}"#).unwrap();
        let s: Settings = load_or_default(&p);
        assert_eq!(s.theme, "light");
        assert_eq!(s.spawn_concurrency, 4);
        assert_eq!(s.scrollback, 5000);
    }

    #[test]
    fn scrollback_is_clamped_to_the_ui_range() {
        assert_eq!(clamp_scrollback(0), 1000);
        assert_eq!(clamp_scrollback(5000), 5000);
        assert_eq!(clamp_scrollback(10_000), 10_000);
        assert_eq!(clamp_scrollback(u32::MAX), 20_000);
    }

    #[test]
    fn invalid_file_is_backed_up() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.json");
        std::fs::write(&p, "{nope").unwrap();
        assert_eq!(load_or_default::<Settings>(&p), Settings::default());
        assert_eq!(std::fs::read_to_string(d.path().join("settings.json.bad")).unwrap(), "{nope");
    }

    #[test]
    fn save_then_load_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let mut s = Settings::default();
        s.accounts.insert("claude".into(), vec!["trabalho".into()]);
        save_json(&p, &s).unwrap();
        assert_eq!(load_or_default::<Settings>(&p), s);
    }

    #[test]
    fn wt_font_face_reads_defaults_profile_override_and_missing() {
        let with_comments = r#"{
            // comentario de linha
            "defaultProfile": "{574e775e}", /* bloco
            com barra // dentro */
            "profiles": {
                "defaults": { "font": { "face": "FiraCode Nerd Font" } },
                "list": [ { "guid": "{574e775e}", "name": "PowerShell" } ]
            }
        }"#;
        assert_eq!(wt_font_face(with_comments).as_deref(), Some("FiraCode Nerd Font"));
        let defaults_only = r#"{"profiles":{"defaults":{"fontFace":"Hack Nerd Font"}}}"#;
        assert_eq!(wt_font_face(defaults_only).as_deref(), Some("Hack Nerd Font"));
        let override_wins = r#"{
            "defaultProfile": "{a}",
            "profiles": {
                "defaults": { "font": { "face": "Cascadia Code" } },
                "list": [ { "guid": "{a}", "font": { "face": "JetBrainsMono Nerd Font" } } ]
            }
        }"#;
        assert_eq!(wt_font_face(override_wins).as_deref(), Some("JetBrainsMono Nerd Font"));
        assert_eq!(wt_font_face("{}"), None);
        assert_eq!(wt_font_face("{nope"), None);
        // comentário e aspas dentro de string não confundem o parser
        assert_eq!(wt_font_face(r#"{"profiles":{"defaults":{"font":{"face":"A // B"}}}}"#).as_deref(), Some("A // B"));
        assert_eq!(wt_font_face(r#"{"profiles":{"defaults":{"font":{"face":"Bad'Quote"}}}}"#), None);
    }

    #[test]
    fn sanitize_refuses_css_breaking_names_and_caps_length() {
        assert_eq!(sanitize_font_face("  Hack Nerd Font "), Some("Hack Nerd Font".into()));
        for bad in ["", "a'b", "a\"b", "a;b", "a{b}", "a\\b", "a\nb"] {
            assert_eq!(sanitize_font_face(bad), None, "{bad:?}");
        }
        assert_eq!(sanitize_font_face(&"x".repeat(100)).unwrap().chars().count(), 64);
    }

    #[test]
    fn migrate_moves_old_default_and_keeps_custom_font() {
        // primeiro boot: herda a face do WT
        let mut s = Settings::default();
        assert!(migrate_terminal_font(&mut s, false, || Some("Hack Nerd Font".into())));
        assert_eq!(s.terminal_font, font_stack(Some("Hack Nerd Font")));
        // primeiro boot sem WT: padrão novo, sem face
        let mut s = Settings::default();
        migrate_terminal_font(&mut s, false, || None);
        assert_eq!(s.terminal_font, TERMINAL_FONT_DEFAULT);
        // padrão antigo migra
        let mut s = Settings { terminal_font: TERMINAL_FONT_OLD_DEFAULT.into(), ..Settings::default() };
        assert!(migrate_terminal_font(&mut s, true, || Some("FiraCode Nerd Font".into())));
        assert_eq!(s.terminal_font, font_stack(Some("FiraCode Nerd Font")));
        // fonte customizada fica intacta, mesmo com WT disponível
        let mut s = Settings { terminal_font: "'Iosevka', monospace".into(), ..Settings::default() };
        assert!(!migrate_terminal_font(&mut s, true, || panic!("não deve consultar o WT")));
        assert_eq!(s.terminal_font, "'Iosevka', monospace");
    }

    #[test]
    fn shortcuts_default_empty_and_bad_entries_are_dropped() {
        assert!(Settings::default().shortcuts.is_empty());
        assert_eq!(Settings::default().notify, NotifyPrefs { finished: true, asks: true, gates: true });
        let mut m = BTreeMap::new();
        m.insert("palette".to_string(), "Ctrl+Alt+K".to_string());
        m.insert(String::new(), "Ctrl+Q".to_string());
        m.insert("newTerminal".to_string(), String::new());
        m.insert("pane1".to_string(), "Ctrl+\u{7}1".to_string());
        m.insert("x".repeat(41), "Ctrl+Q".to_string());
        clean_shortcuts(&mut m);
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["palette"]);
    }

    #[test]
    fn seed_writes_default_once() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("prices.json");
        let v: serde_json::Value = load_or_seed(&p, PRICES_DEFAULT).unwrap();
        assert!(v["models"].as_array().unwrap().len() >= 10);
        std::fs::write(&p, r#"{"models":[]}"#).unwrap();
        let v: serde_json::Value = load_or_seed(&p, PRICES_DEFAULT).unwrap();
        assert!(v["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn default_providers_is_valid_json_with_core_clis() {
        let v: serde_json::Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        let ids: Vec<_> = v["clis"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap().to_string()).collect();
        for want in ["claude", "codex", "gemini", "opencode"] {
            assert!(ids.contains(&want.to_string()), "{want}");
        }
    }

    fn f1_shaped() -> Value {
        // the F1 file: the same defaults without the four new CLI fields
        let mut v: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        for cli in v["clis"].as_array_mut().unwrap() {
            for k in NEW_CLI_FIELDS {
                cli.as_object_mut().unwrap().remove(k);
            }
        }
        v
    }

    fn cli_named<'a>(v: &'a Value, id: &str) -> &'a Value {
        v["clis"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap()
    }

    #[test]
    fn f1_providers_gain_new_fields() {
        let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        let mut user = f1_shaped();
        assert!(merge_new_fields(&mut user, &default));
        let claude = cli_named(&user, "claude");
        assert_eq!(claude["disallowArgs"], serde_json::json!(["--disallowedTools", "{tools}"]));
        assert_eq!(claude["mcp"], "claude");
        assert_eq!(claude["promptArgs"], serde_json::json!(["{prompt}"]));
        // and the merged file is a valid config
        assert!(serde_json::from_value::<ProvidersConfig>(user).is_ok());
    }

    #[test]
    fn a_default_the_user_deleted_does_not_come_back() {
        let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        let mut user = f1_shaped();
        assert!(merge_new_fields(&mut user, &default), "primeira mescla: rotas e install entram e ficam marcados");
        user["routes"].as_array_mut().unwrap().retain(|r| r["id"] != "openrouter");
        assert!(!merge_new_fields(&mut user, &default), "apagada pelo usuário: nada volta, nada muda");
        assert!(user["routes"].as_array().unwrap().iter().all(|r| r["id"] != "openrouter"));
        assert!(user["seenDefaults"].as_array().unwrap().iter().any(|k| k == "route:openrouter"));
    }

    #[test]
    fn explicit_empty_system_args_stays_empty() {
        let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        let mut user = f1_shaped();
        let claude = user["clis"].as_array_mut().unwrap().iter_mut().find(|c| c["id"] == "claude").unwrap();
        claude["systemArgs"] = serde_json::json!([]);
        merge_new_fields(&mut user, &default);
        assert_eq!(cli_named(&user, "claude")["systemArgs"], serde_json::json!([]));
    }

    #[test]
    fn user_cli_without_default_is_untouched() {
        let default: Value = serde_json::from_str(PROVIDERS_DEFAULT).unwrap();
        let mut user = f1_shaped();
        let custom = serde_json::json!({"id": "minha-ia", "label": "Minha IA", "command": "minha"});
        user["clis"].as_array_mut().unwrap().push(custom.clone());
        merge_new_fields(&mut user, &default);
        assert_eq!(cli_named(&user, "minha-ia"), &custom);
    }

    #[test]
    fn load_providers_migrates_file_and_keeps_invalid_one() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("providers.json");
        std::fs::write(&p, f1_shaped().to_string()).unwrap();
        let cfg = load_providers(&p);
        assert_eq!(cli_named(&serde_json::to_value(&cfg).unwrap(), "claude")["mcp"], "claude");
        let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(cli_named(&on_disk, "claude")["mcp"], "claude");

        std::fs::write(&p, "{nope").unwrap();
        let cfg = load_providers(&p);
        assert!(!cfg.clis.is_empty());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{nope");
    }

    #[test]
    fn inline_migration_flips_the_defaults_once() {
        // ajustes gravados antes do bloco B: subagentes em painel e promoção ligada
        let mut s: Settings = serde_json::from_str(r#"{"subagentPanes": true, "execucoesPrefs": {"promover": true}}"#).unwrap();
        assert!(!s.inline_v1);
        assert!(migrate_inline(&mut s));
        assert!(!s.subagent_panes && !s.execucoes_prefs.promover && s.inline_v1);
        // quem voltar ao modo antigo depois não é migrado de novo
        s.subagent_panes = true;
        s.execucoes_prefs.promover = true;
        assert!(!migrate_inline(&mut s));
        assert!(s.subagent_panes && s.execucoes_prefs.promover);
    }

    #[test]
    fn fresh_settings_are_already_inline() {
        let s = Settings::default();
        assert!(!s.subagent_panes && !s.execucoes_prefs.promover && s.inline_v1);
    }
}
