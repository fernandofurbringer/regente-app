//! Ferramenta de captura para o agente (F3): `shot_capture` tira a tela (ou uma região) do monitor sob o cursor e salva
//! na pasta `assets` do painel de quem chamou. Só funciona com `agentCapture` ligado em Configurações → Captura de tela (padrão:
//! desligado), e está em `MUTATING_TOOLS`, então painéis pend e plan pedem confirmação.

use super::tools_media::ensure_assets;
use super::{blocking, caller_row, finish, note, RegenteMcp};
use crate::error::AppResult;
use crate::shot;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, JsonSchema)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct CaptureArgs {
    /// Região do monitor em pixels; sem ela, o monitor inteiro.
    pub region: Option<Region>,
}

#[tool_router(router = shot_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Captura a tela (ou uma região) e salva em `<pasta do painel>/assets`. Devolve o caminho.
    #[tool(description = "Captura a tela do monitor sob o cursor (ou uma região) e salva em <pasta do painel>/assets; só se o usuário liberou em Configurações → Captura de tela")]
    async fn shot_capture(&self, Parameters(a): Parameters<CaptureArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "shot_capture", &parts);
        let r: AppResult<Value> = async {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            shot::agent_allowed(s)?;
            let dir = ensure_assets(&me)?;
            let region = a.region.map(|r| (r.x, r.y, r.w, r.h));
            let path = blocking(move || shot::capture_for_agent(&dir, region)).await?;
            // the user must see that an agent took the screen
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            s.emit("shot-saved", json!({"path": path.to_string_lossy(), "name": name, "pane": me.name}));
            Ok(json!({"path": path}))
        }
        .await;
        finish(r)
    }
}

#[cfg(test)]
mod tests {
    use crate::shot;

    #[test]
    fn the_agent_gets_nothing_until_the_user_allows_it() {
        let d = tempfile::tempdir().unwrap();
        let s = crate::runtime::AppState::new(d.path().into(), "C:/x/regente.exe".into(), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).unwrap();
        assert_eq!(shot::agent_allowed(&s).unwrap_err().code(), "agent_capture_off");
    }
}
