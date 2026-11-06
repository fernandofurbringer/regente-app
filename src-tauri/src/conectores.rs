//! Conectores: servidores MCP cadastrados uma vez no Regente e entregues a todo painel do Claude, seja qual for a
//! conta. Cada conta do Claude Code tem a própria lista em `.claude.json`; com os conectores, não é preciso cadastrar
//! o mesmo MCP em cada conta. A lista (nome, transporte, resumo sem segredo, ligado) fica em `data/conectores.json`;
//! a configuração completa (no formato do `mcpServers` do Claude, que pode ter chaves) fica no Cofre
//! (`conector:<nome>`) e só vai para o `mcp.json` do painel, que é apagado quando o painel fecha e no boot.
//! O Context7 vem cadastrado e ligado. Conector de login OAuth do claude.ai não entra: o login é de cada conta.
use crate::error::{fail, AppResult};
use crate::runtime::AppState;
use crate::util::{home_dir, lk};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const ARQUIVO: &str = "conectores.json";
pub const CONTEXT7: &str = "context7";
const CONTEXT7_URL: &str = "https://mcp.context7.com/mcp";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Conector {
    /// Também é o nome do servidor MCP no painel (`mcp__<nome>__...`).
    pub nome: String,
    /// `http`, `sse` ou `stdio`.
    pub transporte: String,
    /// URL sem a query, ou o comando: o que a tela mostra (nunca uma chave).
    pub resumo: String,
    pub ativo: bool,
}

/// Servidor que existe em alguma conta do Claude e pode virar conector (sem a configuração: ela tem chaves).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Importavel {
    pub nome: String,
    pub transporte: String,
    pub conta: String,
}

fn cofre(nome: &str) -> String {
    format!("conector:{nome}")
}

/// Letras, dígitos, `-` e `_`, até 48; `regente` é o servidor do próprio Regente.
pub fn nome_valido(nome: &str) -> bool {
    (1..=48).contains(&nome.len()) && nome != "regente" && nome.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A configuração do Context7 (a chave é opcional: sem ela o limite de consultas é menor).
pub fn context7(chave: Option<&str>) -> Value {
    let mut v = json!({"type": "http", "url": CONTEXT7_URL});
    if let Some(k) = chave.map(str::trim).filter(|k| !k.is_empty()) {
        v["headers"] = json!({"CONTEXT7_API_KEY": k});
    }
    v
}

/// Confere a configuração (formato do `mcpServers` do Claude) e devolve `(transporte, resumo)`.
/// stdio: `command` (com `args` opcional); remoto: `url` http(s) com `type` http (padrão) ou sse.
pub fn validar(cfg: &Value) -> AppResult<(String, String)> {
    if !cfg.is_object() {
        return fail("bad_connector", "a configuração precisa ser um objeto JSON");
    }
    if let Some(cmd) = cfg["command"].as_str() {
        if cmd.trim().is_empty() {
            return fail("bad_connector", "command vazio");
        }
        if !cfg["args"].is_null() && !cfg["args"].as_array().is_some_and(|a| a.iter().all(Value::is_string)) {
            return fail("bad_connector", "args precisa ser uma lista de textos");
        }
        let args: Vec<&str> = cfg["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let resumo: String = std::iter::once(cmd).chain(args).collect::<Vec<_>>().join(" ").chars().take(120).collect();
        return Ok(("stdio".into(), resumo));
    }
    let Some(url) = cfg["url"].as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://")) else {
        return fail("bad_connector", "falta command (stdio) ou url http(s)");
    };
    let tipo = cfg["type"].as_str().unwrap_or("http");
    if !matches!(tipo, "http" | "sse") {
        return fail("bad_connector", format!("type {tipo} não é suportado (use http ou sse)"));
    }
    // a query pode carregar chave (`?api_key=`): a tela vê só o endereço
    let resumo = url.split(['?', '#']).next().unwrap_or(url).to_string();
    Ok((tipo.into(), resumo))
}

fn caminho(data: &Path) -> PathBuf {
    data.join(ARQUIVO)
}

/// A lista salva; sem arquivo (primeira vez), o Context7 ligado.
pub fn carregar(data: &Path) -> Vec<Conector> {
    match std::fs::read_to_string(caminho(data)) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
            tracing::warn!("conectores.json ilegível ({e}); seguindo sem conectores");
            vec![]
        }),
        Err(_) => vec![Conector { nome: CONTEXT7.into(), transporte: "http".into(), resumo: CONTEXT7_URL.into(), ativo: true }],
    }
}

fn salvar(data: &Path, lista: &[Conector]) -> AppResult<()> {
    crate::settings::save_json(&caminho(data), &lista)
}

impl AppState {
    pub fn conectores(&self) -> Vec<Conector> {
        carregar(&self.data)
    }

    /// A configuração de um conector: a do Cofre; o Context7 sem Cofre usa a padrão (sem chave).
    fn config_do_conector(&self, nome: &str) -> Option<Value> {
        let salvo = self.secret(&cofre(nome)).ok().flatten().and_then(|t| serde_json::from_str(&t).ok());
        salvo.or_else(|| (nome == CONTEXT7).then(|| context7(None)))
    }

    /// Cadastra ou troca um conector (fica ligado).
    pub fn conector_salvar(&self, nome: &str, config: &Value) -> AppResult<Vec<Conector>> {
        if !nome_valido(nome) {
            return fail("bad_connector", "nome: letras, números, - e _ (até 48); regente é reservado");
        }
        let (transporte, resumo) = validar(config)?;
        self.vault()?.as_ref().expect("opened").set(&cofre(nome), &config.to_string())?;
        let mut lista = self.conectores();
        let novo = Conector { nome: nome.into(), transporte, resumo, ativo: true };
        match lista.iter_mut().find(|c| c.nome == nome) {
            Some(c) => *c = novo,
            None => lista.push(novo),
        }
        salvar(&self.data, &lista)?;
        Ok(lista)
    }

    pub fn conector_ativar(&self, nome: &str, ativo: bool) -> AppResult<Vec<Conector>> {
        let mut lista = self.conectores();
        let Some(c) = lista.iter_mut().find(|c| c.nome == nome) else {
            return fail("unknown_connector", format!("conector {nome} não existe"));
        };
        c.ativo = ativo;
        salvar(&self.data, &lista)?;
        Ok(lista)
    }

    pub fn conector_remover(&self, nome: &str) -> AppResult<Vec<Conector>> {
        let mut lista = self.conectores();
        lista.retain(|c| c.nome != nome);
        salvar(&self.data, &lista)?;
        self.vault()?.as_ref().expect("opened").delete(&cofre(nome))?;
        Ok(lista)
    }

    /// O `.claude.json` de uma conta do Claude (a principal segue `CLAUDE_CONFIG_DIR` ou a home, como o Claude Code).
    pub fn claude_json_da_conta(&self, conta: Option<&str>) -> PathBuf {
        match conta.filter(|a| !a.is_empty() && *a != "principal") {
            Some(a) => crate::mcp::user_servers::claude_json_in(Some(self.config_dir_for("claude", Some(a))), &home_dir()),
            None => crate::mcp::user_servers::claude_json_path(&home_dir()),
        }
    }

    fn contas_do_claude(&self) -> Vec<String> {
        let extras = lk(&self.settings).accounts.get("claude").cloned().unwrap_or_default();
        std::iter::once("principal".to_string()).chain(extras).collect()
    }

    /// Servidores de usuário (`mcpServers` do topo) de cada conta do Claude que ainda não são conectores.
    pub fn conectores_importaveis(&self) -> Vec<Importavel> {
        let ja: Vec<String> = self.conectores().into_iter().map(|c| c.nome).collect();
        let mut out: Vec<Importavel> = vec![];
        for conta in self.contas_do_claude() {
            for s in crate::mcp::user_servers::user_level(&self.claude_json_da_conta(Some(&conta))) {
                if !ja.contains(&s.info.name) && !out.iter().any(|o| o.nome == s.info.name) && validar(&s.config).is_ok() {
                    out.push(Importavel { nome: s.info.name, transporte: s.info.transport, conta: conta.clone() });
                }
            }
        }
        out
    }

    /// Copia o servidor `nome` da conta `conta` (o `.claude.json` não muda) para os conectores.
    pub fn conector_importar(&self, nome: &str, conta: &str) -> AppResult<Vec<Conector>> {
        if !self.contas_do_claude().iter().any(|c| c == conta) {
            return fail("unknown_account", format!("a conta {conta} não está cadastrada"));
        }
        let servidores = crate::mcp::user_servers::user_level(&self.claude_json_da_conta(Some(conta)));
        let Some(s) = servidores.into_iter().find(|s| s.info.name == nome) else {
            return fail("unknown_connector", format!("{nome} não está na conta {conta}"));
        };
        self.conector_salvar(nome, &s.config)
    }

    /// Os conectores ligados, com a configuração, para o `mcp.json` de um painel do Claude. Workspace em modo
    /// sensível não recebe nenhum (as configurações vêm do Cofre).
    pub fn conectores_para_painel(&self, workspace_id: &str) -> Vec<(String, Value)> {
        if self.is_sensitive(workspace_id) {
            return vec![];
        }
        self.conectores()
            .into_iter()
            .filter(|c| c.ativo)
            .filter_map(|c| match self.config_do_conector(&c.nome) {
                Some(cfg) => Some((c.nome, cfg)),
                None => {
                    tracing::warn!("conector {}: configuração não está no Cofre; ficou de fora", c.nome);
                    None
                }
            })
            .collect()
    }
}

/// Junta os conectores aos servidores do painel: um servidor do usuário escolhido com o mesmo nome fica (é o da conta).
pub fn juntar(servidores: &mut Vec<(String, Value)>, conectores: Vec<(String, Value)>) {
    for (nome, cfg) in conectores {
        if !servidores.iter().any(|(n, _)| *n == nome) {
            servidores.push((nome, cfg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validar_aceita_stdio_e_remoto_e_esconde_a_query() {
        assert_eq!(validar(&json!({"command": "npx", "args": ["-y", "@upstash/context7-mcp"]})).unwrap(), ("stdio".into(), "npx -y @upstash/context7-mcp".into()));
        assert_eq!(validar(&context7(Some("k"))).unwrap(), ("http".into(), CONTEXT7_URL.into()));
        assert_eq!(validar(&json!({"type": "sse", "url": "https://x.io/sse?api_key=SEGREDO"})).unwrap(), ("sse".into(), "https://x.io/sse".into()));
        for ruim in [json!("x"), json!({}), json!({"command": " "}), json!({"command": "a", "args": [1]}), json!({"url": "ftp://x"}), json!({"type": "ws", "url": "https://x"})] {
            assert_eq!(validar(&ruim).unwrap_err().code(), "bad_connector", "{ruim}");
        }
    }

    #[test]
    fn nomes_e_chave_do_context7() {
        for ok in ["context7", "meu-mcp", "a_b2"] {
            assert!(nome_valido(ok), "{ok}");
        }
        for ruim in ["", "regente", "a b", "x/y", &"a".repeat(49)] {
            assert!(!nome_valido(ruim), "{ruim}");
        }
        assert!(context7(None).get("headers").is_none());
        assert!(context7(Some("  ")).get("headers").is_none());
        assert_eq!(context7(Some("ctx7sk")).pointer("/headers/CONTEXT7_API_KEY"), Some(&json!("ctx7sk")));
    }

    #[test]
    fn primeira_vez_tem_o_context7_ligado() {
        let d = tempfile::tempdir().unwrap();
        let l = carregar(d.path());
        assert_eq!(l.len(), 1);
        assert!(l[0].ativo && l[0].nome == CONTEXT7);
        // lista vazia salva continua vazia (o usuário removeu o Context7)
        salvar(d.path(), &[]).unwrap();
        assert!(carregar(d.path()).is_empty());
    }

    #[test]
    fn servidor_escolhido_da_conta_ganha_do_conector_de_mesmo_nome() {
        let mut s = vec![("context7".to_string(), json!({"conta": true}))];
        juntar(&mut s, vec![("context7".into(), json!({"conta": false})), ("outro".into(), json!({}))]);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].1["conta"], true);
        assert_eq!(s[1].0, "outro");
    }

    #[test]
    fn cadastrar_ligar_importar_e_entregar_ao_painel() {
        use std::sync::Arc;
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        s.set_vault_for_tests(crate::vault::Vault::with_key(&d.path().join("vault.json"), [9u8; 32]));
        // padrão: Context7 sem chave
        assert_eq!(s.conectores_para_painel("w"), vec![(CONTEXT7.to_string(), context7(None))]);
        // com chave: vai para o Cofre; a lista guarda só o resumo
        s.conector_salvar(CONTEXT7, &context7(Some("ctx7sk-1"))).unwrap();
        assert!(!std::fs::read_to_string(d.path().join(ARQUIVO)).unwrap().contains("ctx7sk-1"), "chave fora da lista");
        assert_eq!(s.conectores_para_painel("w")[0].1.pointer("/headers/CONTEXT7_API_KEY"), Some(&json!("ctx7sk-1")));
        // desligado não vai; modo sensível não recebe nada
        s.conector_ativar(CONTEXT7, false).unwrap();
        assert!(s.conectores_para_painel("w").is_empty());
        s.conector_ativar(CONTEXT7, true).unwrap();
        lk(&s.settings).sensitive_workspaces.push("w".into());
        assert!(s.conectores_para_painel("w").is_empty());
        assert_eq!(s.conectores_para_painel("outro").len(), 1);
        // importar de uma conta extra: o servidor do .claude.json dela vira conector
        lk(&s.settings).accounts.insert("claude".into(), vec!["b".into()]);
        let cfg_b = s.config_dir_for("claude", Some("b"));
        std::fs::create_dir_all(&cfg_b).unwrap();
        std::fs::write(cfg_b.join(".claude.json"), json!({"mcpServers": {"banco": {"command": "uvx", "args": ["db-mcp"], "env": {"DB": "x"}}, "context7": {"type": "http", "url": CONTEXT7_URL}}}).to_string()).unwrap();
        let imp = s.conectores_importaveis();
        assert!(imp.iter().any(|i| i.nome == "banco" && i.conta == "b" && i.transporte == "stdio"), "{imp:?}");
        assert!(!imp.iter().any(|i| i.nome == CONTEXT7), "já é conector");
        s.conector_importar("banco", "b").unwrap();
        let entregues = s.conectores_para_painel("outro");
        assert_eq!(entregues.iter().find(|(n, _)| n == "banco").unwrap().1["env"]["DB"], "x");
        assert_eq!(s.conector_importar("banco", "zz").unwrap_err().code(), "unknown_account");
        // remover tira da lista e do Cofre
        s.conector_remover("banco").unwrap();
        assert!(s.secret("conector:banco").unwrap().is_none());
        assert!(!s.conectores().iter().any(|c| c.nome == "banco"));
    }

    #[test]
    fn o_painel_do_claude_recebe_o_context7_no_mcp_json_menos_no_perfil_minimo() {
        use crate::providers::{PaneKind, PaneSpec};
        use std::sync::Arc;
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        s.set_vault_for_tests(crate::vault::Vault::with_key(&d.path().join("vault.json"), [9u8; 32]));
        let _ = s.mcp.set(crate::mcp::McpInfo { port: 4321 });
        if let Some(c) = lk(&s.providers).clis.iter_mut().find(|c| c.id == "claude") {
            c.command = "cmd.exe".into(); // o teste não depende do Claude instalado
        }
        let mcp_json = |perfil: &str| {
            let id = crate::util::new_id();
            let spec = PaneSpec { workspace_id: "w".into(), kind: PaneKind::Agent, cli: Some("claude".into()), cwd: d.path().to_string_lossy().into(), mcp_profile: Some(perfil.into()), ..Default::default() };
            s.plan_for_test(&id, &spec).unwrap();
            let v: Value = serde_json::from_str(&std::fs::read_to_string(d.path().join("panes").join(&id).join("mcp.json")).unwrap()).unwrap();
            v["mcpServers"].clone()
        };
        let full = mcp_json("full");
        assert_eq!(full["context7"]["url"], CONTEXT7_URL, "{full}");
        assert!(full["regente"].is_object());
        assert!(mcp_json("minimal").get("context7").is_none());
    }
}
