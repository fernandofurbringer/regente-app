use super::{caller_row, finish, mission_in_ws, note, own_mission, pane_in_ws, RegenteMcp};
use crate::artefatos::{self, Artefato};
use crate::db::{Handoff, PaneRow};
use crate::error::{fail, AppResult};
use crate::formato;
use crate::orchestrator::{submit_handoff, HandoffInput};
use crate::runtime::AppState;
use crate::util::new_id;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::Path;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitArgs {
    /// Resumo da entrega, em uma ou duas frases.
    pub summary: String,
    /// Dados estruturados da entrega (validados pelo schema do painel, se houver).
    pub data: Option<Value>,
    /// Arquivos produzidos (caminhos).
    pub files: Option<Vec<String>>,
    /// Resultado grande: o arquivo que você escreveu (`path`) ou o conteúdo que o Regente grava (`content`).
    pub artifact: Option<ArtifactArg>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactArg {
    /// Arquivo que você já escreveu, em `.regente/artefatos/<missão>/`, relativo à pasta do painel.
    pub path: Option<String>,
    /// Conteúdo que o Regente grava: texto (vira Markdown) ou JSON.
    pub content: Option<Value>,
    /// auto (padrão), json, csv, toon ou md.
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListArgs {
    /// Id da missão (padrão: a sua).
    pub mission: Option<String>,
    /// Quantas entregas devolver (padrão 20).
    pub limit: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetArgs {
    /// Id da entrega.
    pub id: String,
    /// true traz dados e arquivos completos (padrão: resumo, chaves e artefato).
    pub detail: Option<bool>,
}

#[tool_router(router = handoff_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// ÚNICA forma correta de dizer que terminou. Depois de chamar, pare.
    #[tool(description = "ÚNICA forma correta de dizer que terminou. Depois de chamar, pare.", meta = super::sempre_carregada())]
    async fn handoff_submit(&self, Parameters(a): Parameters<SubmitArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "handoff_submit", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            crate::orchestrator::checar_limites(&a.summary, a.data.as_ref(), a.files.as_deref().unwrap_or_default())?;
            let data = match a.artifact {
                Some(art) => Some(anexar(s, &me, art, a.data)?),
                None => a.data,
            };
            let (resumo, dados) = (a.summary.clone(), data.clone());
            let h = submit_handoff(s, &me.id, HandoffInput { summary: a.summary, data, files: a.files.unwrap_or_default() })?;
            // executor de um plano: a entrega (ou a pergunta) anda o plano (planexec)
            crate::planexec::on_handoff(s, &me, dados.as_ref(), &resumo, h.to_pane.as_deref());
            Ok(json!({"id": h.id, "deliveredTo": h.to_pane}))
        })())
    }

    /// Entregas da missão (ou da missão informada), das mais recentes.
    #[tool(description = "Entregas da missão (ou da missão informada), das mais recentes")]
    async fn handoff_list(&self, Parameters(a): Parameters<ListArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "handoff_list", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let mission = match a.mission {
                Some(id) => mission_in_ws(s, &me.workspace_id, &id)?.id,
                None => own_mission(&me)?,
            };
            let rows = s.db.handoffs(Some(&mission), None, a.limit.unwrap_or(20).clamp(1, 200))?;
            Ok(json!({"handoffs": rows.iter().map(|h| resumo_entrega(s, h)).collect::<Vec<_>>()}))
        })())
    }

    /// Uma entrega: resumo, chaves dos dados e artefato (detail=true traz tudo).
    #[tool(description = "Uma entrega: resumo, chaves dos dados e artefato. detail=true traz tudo (dados e arquivos)")]
    async fn handoff_get(&self, Parameters(a): Parameters<GetArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "handoff_get", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            let h = s.db.get_handoff(&a.id)?;
            // the handoff's sender must be in the caller's workspace
            pane_in_ws(s, &me.workspace_id, &h.from_pane)?;
            if a.detail.unwrap_or(false) {
                return Ok(json!(h));
            }
            let data = dados(&h);
            let chaves: Vec<&String> = data.as_ref().and_then(Value::as_object).map(|m| m.keys().collect()).unwrap_or_default();
            Ok(json!({"id": h.id, "from": h.from_pane, "summary": h.summary, "dataKeys": chaves, "artifact": data.as_ref().and_then(|d| d.get("artifact")).cloned()}))
        })())
    }
}

/// Resultado grande da entrega: o arquivo fica na pasta do painel e a entrega leva só a descrição (RM-19).
fn anexar(s: &AppState, me: &PaneRow, art: ArtifactArg, data: Option<Value>) -> AppResult<Value> {
    let cwd = Path::new(&me.cwd);
    let missao = me.mission_id.as_deref();
    let descricao: Artefato = match (art.path, art.content) {
        (Some(p), None) => artefatos::registrar_existente(cwd, missao, &p)?,
        (None, Some(c)) => {
            let perfil = crate::perfil::efetivo(s, &me.workspace_id, missao)?.0.artefatos.formato;
            let pedido = artefatos::escolher(art.format.as_deref(), &c, &perfil)?;
            let f = formato::efetivo(&c, pedido);
            let texto = formato::encode(&c, f);
            let gravado = artefatos::write(cwd, missao.unwrap_or(artefatos::SCOPE_WS), &new_id(), &texto, artefatos::ext_de(f))?;
            artefatos::com_valor(gravado, &c)
        }
        _ => return fail("bad_artifact", "artefato: informe path (arquivo seu) ou content (o Regente grava), não os dois"),
    };
    let mut m = match data {
        Some(Value::Object(m)) => m,
        None | Some(Value::Null) => Map::new(),
        Some(_) => return fail("bad_data", "com artefato, os dados da entrega precisam ser um objeto"),
    };
    m.insert("artifact".into(), json!(descricao));
    Ok(Value::Object(m))
}

/// Dados de uma entrega (JSON), se houver.
fn dados(h: &Handoff) -> Option<Value> {
    h.data_json.as_deref().and_then(|t| serde_json::from_str(t).ok())
}

/// Linha concisa de `handoff_list`: sem dados, resumo de até 120 caracteres.
fn resumo_entrega(s: &AppState, h: &Handoff) -> Value {
    let d = dados(h);
    json!({
        "id": h.id,
        "from": h.from_pane,
        "taskType": s.db.get_pane(&h.from_pane).ok().and_then(|p| p.task_type),
        "ok": d.as_ref().and_then(|d| d.get("ok")).and_then(Value::as_bool).unwrap_or(true),
        "summary": h.summary.chars().take(120).collect::<String>(),
    })
}
