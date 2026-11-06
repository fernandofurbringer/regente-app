use super::{caller_row, finish, note, require_top, RegenteMcp};
use crate::db::Memory;
use crate::error::{fail, AppResult};
use crate::util::now_ms;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

const KINDS: &[&str] = &["fato", "decisao", "risco", "checkpoint"];

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WriteArgs {
    /// Tipo: fato, decisao, risco ou checkpoint.
    pub kind: String,
    /// Escopo: omitido = o workspace (e a missão, se houver); "global" = preferência do usuário para todos os projetos (só topo; vira sugestão).
    pub escopo: Option<String>,
    /// Texto da memória (curto e autocontido).
    pub text: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadArgs {
    /// Quantas memórias devolver, das mais recentes (padrão 20).
    pub limit: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Termos da busca.
    pub q: String,
    /// Quantos resultados (padrão 10).
    pub limit: Option<i64>,
    /// Modo: "auto" (padrão), "palavra" ou "significado" (ainda cai na busca por palavra).
    pub mode: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PinArgs {
    /// Id da memória (de memory_read ou memory_search).
    pub id: i64,
    /// true fixa (entra primeiro no brief e não expira); false desfixa.
    pub pinned: bool,
}

#[tool_router(router = memory_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Grava na memória do workspace (e da missão, se você estiver numa) um fato, decisão, risco ou checkpoint.
    #[tool(description = "Grava na memória do workspace (e da missão, se você estiver numa) um fato, decisão, risco ou checkpoint")]
    async fn memory_write(&self, Parameters(a): Parameters<WriteArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "memory_write", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            if !KINDS.contains(&a.kind.as_str()) {
                return fail("bad_kind", "tipo: fato, decisao, risco ou checkpoint");
            }
            // global: preferência do usuário para todos os projetos; só topo escreve, e a escrita vira sugestão (revisão em Memória)
            let global = match a.escopo.as_deref() {
                None | Some("workspace") => false,
                Some("global") => true,
                Some(_) => return fail("bad_escopo", "escopo: workspace ou global"),
            };
            if global {
                require_top(&me)?;
            }
            let text = crate::memory::clean(&crate::diag::redact(&a.text));
            if text.is_empty() {
                return fail("bad_text", "o texto não pode ficar vazio");
            }
            if text.chars().count() > crate::memory::TEXT_MAX {
                return fail("text_too_long", "a memória passa de 1000 caracteres");
            }
            let (workspace_id, mission_id) = if global { (crate::memory::GLOBAL.to_string(), None) } else { (me.workspace_id.clone(), me.mission_id.clone()) };
            let status = estado_da_escrita(global, super::papel_curto(me.role.as_deref()));
            let id = crate::memory::save(&s.db, &Memory {
                id: 0,
                workspace_id,
                mission_id,
                kind: a.kind,
                text,
                author_pane: Some(me.id.clone()),
                created_at: now_ms(),
                status: status.into(),
                ..Default::default()
            })?;
            Ok(json!({"id": id}))
        })())
    }

    /// Memórias do workspace, das mais recentes.
    #[tool(description = "Memórias do workspace, das mais recentes")]
    async fn memory_read(&self, Parameters(a): Parameters<ReadArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "memory_read", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            Ok(json!({"memories": s.db.memories(&me.workspace_id, None, a.limit.unwrap_or(20))?, "preferencias": s.db.memories(crate::memory::GLOBAL, None, crate::memory::PREFS_MAX as i64)?}))
        })())
    }

    /// Busca nas memórias do workspace e nas preferências globais (texto completo).
    #[tool(description = "Busca nas memórias do workspace e nas preferências globais (texto completo)")]
    async fn memory_search(&self, Parameters(a): Parameters<SearchArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "memory_search", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let mode = a.mode.as_deref().unwrap_or("auto");
            if !["auto", "palavra", "significado"].contains(&mode) {
                return fail("bad_mode", "modo: auto, palavra ou significado");
            }
            let limit = a.limit.unwrap_or(10).max(0);
            // "palavra" força a busca por palavra; "auto" e "significado" usam a híbrida com a busca ligada (o modelo sobe sob demanda)
            let por_significado = mode != "palavra" && crate::embed::ligada();
            let buscar = |ws: &str| -> AppResult<Vec<crate::db::Memory>> {
                if por_significado { crate::embed::buscar(&s.db, ws, &a.q, limit) } else { s.db.search_memories(ws, &a.q, limit) }
            };
            let mut found = buscar(&me.workspace_id)?;
            if me.workspace_id != crate::memory::GLOBAL {
                found.extend(buscar(crate::memory::GLOBAL)?);
            }
            found.truncate(limit as usize);
            let nota = (mode == "significado" && !por_significado).then_some("busca por significado não instalada ou desligada: usei busca por palavra");
            Ok(json!({"memories": found, "modo": if por_significado { "significado" } else { "palavra" }, "nota": nota}))
        })())
    }

    /// Fixa ou desfixa uma memória do workspace. Só piloto, chefe ou maestro. Fixada entra primeiro no brief e não expira.
    #[tool(description = "Fixa ou desfixa uma memória do workspace (só piloto, chefe ou maestro)")]
    async fn memory_pin(&self, Parameters(a): Parameters<PinArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "memory_pin", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            require_top(&me)?;
            match s.db.get_memory(a.id)? {
                Some(m) if m.workspace_id == me.workspace_id => {}
                _ => return fail("not_found", "memória não encontrada neste workspace"),
            }
            s.db.set_memory_pinned(a.id, a.pinned)?;
            Ok(json!({"id": a.id, "pinned": a.pinned}))
        })())
    }
}

/// Estado de uma memória que um agente grava: global e papéis curtos (ajudantes, executores) viram sugestão, à espera
/// de revisão; o topo grava ativa.
pub(crate) fn estado_da_escrita(global: bool, papel_curto: bool) -> &'static str {
    if global || papel_curto {
        "sugerida"
    } else {
        "ativa"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_roles_write_suggestions_and_the_top_writes_active() {
        assert_eq!(estado_da_escrita(false, super::super::papel_curto(Some("executor"))), "sugerida");
        assert_eq!(estado_da_escrita(false, super::super::papel_curto(Some("ajudante"))), "sugerida");
        assert_eq!(estado_da_escrita(false, super::super::papel_curto(Some("piloto"))), "ativa");
        assert_eq!(estado_da_escrita(false, super::super::papel_curto(None)), "ativa");
        assert_eq!(estado_da_escrita(true, false), "sugerida", "global sempre sugere");
    }
}
