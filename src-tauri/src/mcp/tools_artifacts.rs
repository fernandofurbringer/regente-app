use super::{caller_row, note, ok_text, tool_err, RegenteMcp};
use crate::artefatos;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    /// Caminho do artefato (o `path` que veio na entrega).
    pub path: String,
    /// Campos a devolver (JSON e CSV). Padrão: todos.
    pub fields: Option<Vec<String>>,
    /// Primeiro item ou linha (padrão 0).
    pub offset: Option<usize>,
    /// Quantos itens ou linhas. Padrão: o que couber em 16 KB.
    pub limit: Option<usize>,
    /// Filtro: trecho que deve aparecer, sem diferenciar maiúsculas.
    pub query: Option<String>,
    /// Formato da saída: auto, json, csv, toon ou md.
    pub format: Option<String>,
}

#[tool_router(router = artifacts_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Lê só a parte pedida de um artefato da sua missão (campos, linhas, filtro). Até 16 KB.
    #[tool(description = "Lê só a parte pedida de um artefato da sua missão (campos, linhas, filtro). Até 16 KB")]
    async fn artifact_read(&self, Parameters(a): Parameters<ReadArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "artifact_read", &parts);
        let r = (|| -> crate::error::AppResult<String> {
            let me = caller_row(&self.state, &parts)?;
            let file = artefatos::resolve(Path::new(&me.cwd), me.mission_id.as_deref(), &a.path)?;
            artefatos::read_slice(&file, a.fields.as_deref(), a.offset.unwrap_or(0), a.limit, a.query.as_deref(), a.format.as_deref())
        })();
        match r {
            Ok(texto) => ok_text(texto),
            Err(e) => tool_err(e),
        }
    }
}
