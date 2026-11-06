//! Ferramentas do índice de código (RM-24, H1-05): `code_outline`, `code_symbols` e `code_search`. Só leitura, por isso
//! não estão em `MUTATING_TOOLS`. A raiz é o cwd do painel que chama; caminho fora dela é recusado.

use super::{caller_row, note, ok_text, tool_err, RegenteMcp};
use crate::codeindex::{com_indice, rel_de, search};
use crate::error::{fail, AppError, AppResult};
use crate::files;
use crate::runtime::AppState;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

/// Teto de cada resposta em caracteres (anunciado em `_meta` anthropic/maxResultSizeChars).
const RESPOSTA_MAX: usize = 6000;
/// Aviso anexado às respostas de `code_outline` e `code_symbols` quando a última passada parou no teto.
const AVISO_PARCIAL: &str = "(índice parcial; tente de novo em alguns segundos)";
/// Símbolos de topo por página de `code_outline`.
const OUTLINE_PAGINA: usize = 150;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutlineArgs {
    /// Arquivo, relativo à pasta do painel ou absoluto dentro dela.
    pub path: String,
    /// concise (padrão): tipo, nome e linhas. detailed: também a linha da assinatura.
    pub format: Option<String>,
    /// Número devolvido em "Próximo cursor" da página anterior.
    pub cursor: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SymbolsArgs {
    /// Nome (ou parte do nome) da definição.
    pub query: String,
    /// function, struct, enum, trait, type, mod, const, static, class, interface ou method.
    pub kind: Option<String>,
    /// Glob sobre o caminho relativo, ex.: src/**/*.rs.
    pub path_glob: Option<String>,
    /// Quantas definições (padrão 20, máximo 100).
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    /// Expressão regular (sintaxe do ripgrep).
    pub pattern: String,
    /// Glob sobre o caminho relativo, ex.: src/**/*.ts.
    pub path_glob: Option<String>,
    /// Linhas de contexto antes e depois (padrão 1, máximo 5).
    pub context: Option<usize>,
    /// Quantos acertos (padrão 30, máximo 200).
    pub limit: Option<usize>,
}

/// `_meta` que avisa ao cliente o teto da resposta destas ferramentas.
fn cap_meta() -> rmcp::model::MetaObject {
    let mut m = serde_json::Map::new();
    m.insert("anthropic/maxResultSizeChars".into(), serde_json::json!(RESPOSTA_MAX));
    m.into()
}

/// Raiz do índice para o painel que chama. Recusa workspace sensível, remoto e com o índice desligado no perfil.
fn raiz_do_chamador(s: &AppState, parts: &http::request::Parts) -> AppResult<PathBuf> {
    let me = caller_row(s, parts)?;
    let raiz = PathBuf::from(&me.cwd);
    if s.is_sensitive(&me.workspace_id) || crate::codeindex::toca_sensivel(s, &raiz) {
        crate::codeindex::esquecer(&raiz);
        return fail("indice_sensivel", "índice de código indisponível: workspace sensível");
    }
    if crate::remote::parse_ws_path(&me.cwd).is_some() {
        return fail("indice_remoto", "índice de código não funciona em painel remoto");
    }
    if !crate::perfil::efetivo_de(s, &me.workspace_id, me.profile.as_deref()).0.indice_codigo {
        return fail("indice_desligado", "Índice de código desligado neste workspace (Configurações → Perfil de trabalho)");
    }
    Ok(raiz)
}

/// Caminho pedido, dentro da raiz (links, `..` e absolutos fora são recusados pelo `ensure_within`). Devolve o normalizado.
fn arquivo_no_workspace(raiz: &Path, p: &str) -> AppResult<PathBuf> {
    let abs = if Path::new(p).is_absolute() { PathBuf::from(p) } else { raiz.join(p) };
    files::ensure_within(&[raiz.to_path_buf()], &abs)?;
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    Ok(out)
}

fn glob_de(g: Option<String>) -> AppResult<Option<glob::Pattern>> {
    g.map(|g| glob::Pattern::new(&g).map_err(|e| AppError::new("glob_invalido", e.to_string()))).transpose()
}

/// Corta a resposta no teto de caracteres, com aviso.
fn limitar(texto: String) -> String {
    if texto.chars().count() <= RESPOSTA_MAX {
        return texto;
    }
    let mut t: String = texto.chars().take(RESPOSTA_MAX).collect();
    t.push_str("\n… (resposta cortada)");
    t
}

#[tool_router(router = code_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Símbolos de topo de um arquivo (tipo, nome, linhas), sem corpos. Use antes de ler o arquivo inteiro.
    #[tool(description = "Símbolos de topo de um arquivo (tipo, nome, linhas), sem corpos. Use antes de ler o arquivo inteiro.", meta = cap_meta())]
    async fn code_outline(&self, Parameters(a): Parameters<OutlineArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "code_outline", &parts);
        let s = self.state.clone();
        let r = super::blocking(move || -> AppResult<String> {
            let raiz = raiz_do_chamador(&s, &parts)?;
            let detalhe = match a.format.as_deref().unwrap_or("concise") {
                "concise" => false,
                "detailed" => true,
                _ => return fail("formato_invalido", "format: concise ou detailed"),
            };
            let inicio: usize = match a.cursor.as_deref() {
                None => 0,
                Some(c) => c.parse().map_err(|_| AppError::new("cursor_invalido", "cursor inválido"))?,
            };
            let arq = arquivo_no_workspace(&raiz, &a.path)?;
            let Some(rel) = rel_de(&raiz, &arq) else { return fail("fora_da_raiz", "caminho fora do workspace") };
            com_indice(&raiz, |idx| {
                let Some(e) = idx.arquivos.get(&rel) else {
                    let motivo = if idx.parcial { "arquivo ainda não indexado (índice parcial; tente de novo em alguns segundos)" } else { "arquivo sem índice: linguagem não suportada, ignorado pelo .gitignore ou grande demais" };
                    return Err(AppError::new("sem_indice", motivo));
                };
                let tops: Vec<_> = e.simbolos.iter().filter(|x| x.top).collect();
                let mut out = format!("# {rel} · {} símbolos de topo
", tops.len());
                // página até o orçamento de caracteres (sem o aviso de cursor); o cursor aponta para o primeiro que não coube
                let orcamento = RESPOSTA_MAX - 80;
                let fim = inicio.saturating_add(OUTLINE_PAGINA).min(tops.len());
                let mut i = inicio;
                while i < fim {
                    let x = tops[i];
                    let mut l = format!("{} {} L{}-{}", x.kind, x.name, x.line, x.end_line);
                    if detalhe {
                        l.push_str(&format!("  {}", x.sig));
                    }
                    l.push('\n');
                    if i > inicio && out.chars().count() + l.chars().count() > orcamento {
                        break;
                    }
                    out.push_str(&l);
                    i += 1;
                }
                if i < tops.len() {
                    out.push_str(&format!("Próximo cursor: {i}
"));
                }
                if idx.parcial {
                    out.push_str(&format!("{AVISO_PARCIAL}\n"));
                }
                Ok(limitar(out))
            })
        })
        .await;
        match r {
            Ok(t) => ok_text(t),
            Err(e) => tool_err(e),
        }
    }

    /// Definições pelo nome no workspace, da mais parecida para a menos, com arquivo, linha e assinatura.
    #[tool(description = "Definições pelo nome no workspace, da mais parecida para a menos, com arquivo, linha e assinatura.", meta = cap_meta())]
    async fn code_symbols(&self, Parameters(a): Parameters<SymbolsArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "code_symbols", &parts);
        let s = self.state.clone();
        let r = super::blocking(move || -> AppResult<String> {
            let raiz = raiz_do_chamador(&s, &parts)?;
            if a.query.trim().is_empty() {
                return fail("consulta_vazia", "informe um nome para buscar");
            }
            let glob = glob_de(a.path_glob)?;
            let limite = a.limit.unwrap_or(20).clamp(1, 100);
            com_indice(&raiz, |idx| {
                let hits = search::ranking(idx, &a.query, a.kind.as_deref(), glob.as_ref(), limite);
                // índice parcial nunca responde "nenhuma" sem dizer que pode faltar coisa
                let aviso = if idx.parcial { format!("{AVISO_PARCIAL}\n") } else { String::new() };
                if hits.is_empty() {
                    return Ok(format!("Nenhuma definição encontrada.{}", if idx.parcial { format!(" {AVISO_PARCIAL}") } else { String::new() }));
                }
                let linhas: String = hits.iter().map(|h| format!("{}:{} {} {} — {}\n", h.rel, h.simbolo.line, h.simbolo.kind, h.simbolo.name, h.simbolo.sig)).collect();
                Ok(limitar(format!("{linhas}{aviso}")))
            })
        })
        .await;
        match r {
            Ok(t) => ok_text(t),
            Err(e) => tool_err(e),
        }
    }

    /// Busca regex no código do workspace (respeita .gitignore): arquivo:linha: texto, com contexto e limite.
    #[tool(description = "Busca regex no código do workspace (respeita .gitignore): arquivo:linha: texto, com contexto e limite.", meta = cap_meta())]
    async fn code_search(&self, Parameters(a): Parameters<SearchArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "code_search", &parts);
        let s = self.state.clone();
        let r = super::blocking(move || -> AppResult<String> {
            let raiz = raiz_do_chamador(&s, &parts)?;
            let op = search::Opcoes {
                glob: glob_de(a.path_glob)?,
                contexto: a.context.unwrap_or(1).min(5),
                limite: a.limit.unwrap_or(30).clamp(1, 200),
                max_chars: RESPOSTA_MAX,
            };
            search::grep(&raiz, &a.pattern, &op)
        })
        .await;
        match r {
            Ok(t) => ok_text(t),
            Err(e) => tool_err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::PaneRow;
    use crate::mcp::CallerPane;
    use std::sync::Arc;

    fn parts_de(pane: &str) -> http::request::Parts {
        let (mut parts, _) = http::Request::builder().body(()).unwrap().into_parts();
        parts.extensions.insert(CallerPane(pane.into()));
        parts
    }

    /// Estado com um workspace e um painel cujo cwd é a pasta do workspace.
    fn estado(d: &Path, profile: Option<&str>) -> (Arc<AppState>, String) {
        let s = AppState::new(d.join("data"), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: serde_json::Value| {})).unwrap();
        let ws = d.join("proj");
        std::fs::create_dir_all(&ws).unwrap();
        let w = s.db.upsert_workspace(&ws.to_string_lossy()).unwrap();
        s.db.insert_pane(&PaneRow { id: "p".into(), workspace_id: w.id.clone(), kind: "agent".into(), name: "p".into(), cwd: ws.to_string_lossy().to_string(), status: "ocioso".into(), profile: profile.map(String::from), ..Default::default() }).unwrap();
        (s, w.id)
    }

    #[test]
    fn paths_outside_the_root_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let raiz = d.path().join("proj");
        std::fs::create_dir_all(raiz.join("src")).unwrap();
        std::fs::write(raiz.join("src").join("a.rs"), "fn a() {}").unwrap();
        assert!(arquivo_no_workspace(&raiz, "src/a.rs").is_ok());
        assert!(arquivo_no_workspace(&raiz, "src/../src/a.rs").is_ok(), "`..` que fica dentro passa");
        assert_eq!(arquivo_no_workspace(&raiz, "../fora.rs").unwrap_err().code(), "outside_workspace");
        let absoluto_fora = d.path().join("fora.rs");
        assert_eq!(arquivo_no_workspace(&raiz, &absoluto_fora.to_string_lossy()).unwrap_err().code(), "outside_workspace");
    }

    #[test]
    fn index_is_off_for_profiles_without_it_and_refused_in_sensitive_workspaces() {
        let d = tempfile::tempdir().unwrap();
        let (s, ws) = estado(d.path(), Some("precisao"));
        let erro = raiz_do_chamador(&s, &parts_de("p")).unwrap_err();
        assert_eq!(erro.code(), "indice_desligado");
        assert!(erro.to_string().contains("Perfil de trabalho"));

        let d2 = tempfile::tempdir().unwrap();
        let (s2, ws2) = estado(d2.path(), Some("economico"));
        assert!(raiz_do_chamador(&s2, &parts_de("p")).is_ok(), "econômico liga o índice");
        crate::util::lk(&s2.settings).sensitive_workspaces.push(ws2.clone());
        assert_eq!(raiz_do_chamador(&s2, &parts_de("p")).unwrap_err().code(), "indice_sensivel");
        let _ = ws;
    }

    #[test]
    fn sensitive_workspace_inside_the_root_blocks_the_root() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = estado(d.path(), Some("economico"));
        let pasta = d.path().join("proj").join("cliente");
        std::fs::create_dir_all(&pasta).unwrap();
        let filho = s.db.upsert_workspace(&pasta.to_string_lossy()).unwrap();
        crate::util::lk(&s.settings).sensitive_workspaces.push(filho.id.clone());
        assert_eq!(raiz_do_chamador(&s, &parts_de("p")).unwrap_err().code(), "indice_sensivel");
    }

    #[test]
    fn no_caller_header_means_no_index() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = estado(d.path(), Some("economico"));
        let (sem, _) = http::Request::builder().body(()).unwrap().into_parts();
        assert_eq!(raiz_do_chamador(&s, &sem).unwrap_err().code(), "no_caller");
    }

    #[test]
    fn the_cap_is_announced_in_meta() {
        let m = cap_meta();
        assert_eq!(m["anthropic/maxResultSizeChars"], serde_json::json!(6000));
    }

    #[test]
    fn answers_are_capped() {
        let grande = "x".repeat(RESPOSTA_MAX + 50);
        let cortada = limitar(grande);
        assert!(cortada.ends_with("(resposta cortada)"));
        assert!(cortada.chars().count() <= RESPOSTA_MAX + 30);
    }
}
