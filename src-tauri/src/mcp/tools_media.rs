//! Ferramentas de mídia (F3): imagem, vídeo e áudio por IA. Gravam em `<pasta do painel>/assets/` e, com `open`,
//! abrem um painel de imagem ou vídeo na missão. Chamam APIs pagas: passam pelos limites de gasto e por uma
//! geração por vez de cada tipo (`media::acquire`) antes de qualquer chamada de rede. Referências de imagem
//! precisam estar dentro da pasta do painel. `media_providers` não gasta nada e não está em `MUTATING_TOOLS`.

use super::tools_panes::plain_arg;
use super::{caller_row, check_spawn_budget, check_texts, finish, not_archived, note, RegenteMcp};
use crate::db::PaneRow;
use crate::error::{fail, AppError, AppResult};
use crate::media::{self, ImageReq, Ticket};
use crate::providers::{PaneKind, PaneSpec};
use crate::runtime::AppState;
use crate::squads::{self, MediaAllow};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageArgs {
    /// Descrição da imagem.
    pub prompt: String,
    /// Proporção: 1:1, 16:9, 9:16, 4:3, 3:4, 3:2 ou 2:3.
    pub aspect: Option<String>,
    /// Imagem de referência, dentro da pasta do painel.
    pub reference_image: Option<String>,
    /// google ou openai (padrão: o configurado em Mídia).
    pub provider: Option<String>,
    /// Modelo do provedor (padrão: o configurado).
    pub model: Option<String>,
    /// Abre um painel de imagem na missão com o resultado.
    pub open: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VideoArgs {
    /// Descrição do vídeo.
    pub prompt: String,
    /// Primeiro quadro, dentro da pasta do painel.
    pub start_image: Option<String>,
    /// Último quadro, dentro da pasta do painel.
    pub end_image: Option<String>,
    /// Modelo do Replicate, owner/nome (padrão: o configurado).
    pub model: Option<String>,
    /// Abre um painel de vídeo na missão com o resultado.
    pub open: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioArgs {
    /// Texto a falar (voz) ou descrição do efeito (efeito).
    pub text: String,
    /// voz (padrão) ou efeito.
    pub kind: Option<String>,
    /// Id da voz do ElevenLabs (padrão: a configurada).
    pub voice: Option<String>,
}

/// `<cwd>/assets`, created now. Canonical, and it must stay inside the canonical pane folder (a symlink
/// called `assets` pointing elsewhere is refused).
pub(crate) fn ensure_assets(me: &PaneRow) -> AppResult<PathBuf> {
    let root = std::fs::canonicalize(&me.cwd).map_err(|_| AppError::new("bad_cwd", "pasta do painel inexistente"))?;
    let dir = root.join("assets");
    std::fs::create_dir_all(&dir)?;
    let dir = std::fs::canonicalize(&dir)?;
    if !dir.starts_with(&root) {
        return fail("outside_workspace", "a pasta assets fica fora da pasta do painel");
    }
    Ok(dir)
}

/// A reference image the agent names: it must resolve inside the caller's folder, so a paid API gets only what the
/// workspace holds.
fn inside(cwd: &str, p: &str) -> AppResult<PathBuf> {
    let root = std::fs::canonicalize(cwd).map_err(|_| AppError::new("bad_cwd", "pasta do painel inexistente"))?;
    let path = std::fs::canonicalize(Path::new(cwd).join(p)).map_err(|_| AppError::new("not_found", "imagem de referência não existe"))?;
    if !path.starts_with(&root) {
        return fail("outside_workspace", "a imagem de referência precisa estar na pasta do painel");
    }
    Ok(path)
}

/// The media allowance (`media: {providers, models}`) of the caller's mission squad. A mission or squad that cannot be
/// read is an error, never "no restriction" (fail closed).
fn squad_media(s: &AppState, me: &PaneRow) -> AppResult<Option<MediaAllow>> {
    let Some(mission_id) = me.mission_id.as_deref() else { return Ok(None) };
    let mission = s.db.get_mission(mission_id)?;
    let Some(squad_id) = mission.squad_id.as_deref() else { return Ok(None) };
    let squad = squads::get(&s.data, squad_id).ok_or_else(|| AppError::new("squad_missing", "a squad desta missão não foi encontrada: a mídia está bloqueada"))?;
    Ok(squad.media)
}

/// Everything that runs before a paid call: with `open`, the mission must be open and have a pane slot; the
/// spending caps and the one-generation-per-kind slot; the assets folder. The returned ticket holds the slot.
fn gate(s: &AppState, me: &PaneRow, kind: &str, open: bool) -> AppResult<(PathBuf, Ticket)> {
    if open {
        not_archived(s, me)?;
        check_spawn_budget(s, me, 1)?;
    }
    let ticket = media::acquire(s, kind, me.mission_id.as_deref())?;
    Ok((ensure_assets(me)?, ticket))
}

/// Opens a pane that shows the file in the caller's mission (no process: the pane only reads the file).
fn open_pane(s: &AppState, me: &PaneRow, kind: PaneKind, path: &Path) -> AppResult<Value> {
    not_archived(s, me)?;
    check_spawn_budget(s, me, 1)?;
    let row = s.spawn_pane(PaneSpec {
        workspace_id: me.workspace_id.clone(),
        mission_id: me.mission_id.clone(),
        kind,
        cwd: me.cwd.clone(),
        url_or_path: Some(path.display().to_string()),
        ..Default::default()
    })?;
    Ok(json!({"paneId": row.id}))
}

#[tool_router(router = media_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Gera uma imagem (Gemini ou GPT Image) e salva em `<pasta do painel>/assets`. Devolve o caminho.
    #[tool(description = "Gera uma imagem (Gemini ou GPT Image) e salva em <pasta do painel>/assets; open=true abre um painel de imagem")]
    async fn image_generate(&self, Parameters(a): Parameters<ImageArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "image_generate", &parts);
        let r: AppResult<Value> = async {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            check_texts(&[a.prompt.as_str()])?;
            for v in [a.provider.as_deref(), a.model.as_deref()].into_iter().flatten() {
                plain_arg(v)?;
            }
            let reference = a.reference_image.as_deref().map(|p| inside(&me.cwd, p)).transpose()?;
            let allow = squad_media(s, &me)?;
            let open = a.open.unwrap_or(false);
            let (dir, _slot) = gate(s, &me, "image", open)?;
            let req = ImageReq { prompt: a.prompt.clone(), aspect: a.aspect.clone(), reference, provider: a.provider.clone(), model: a.model.clone() };
            let path = media::image_generate(s, req, &dir, allow.as_ref()).await?;
            if open {
                let pane = open_pane(s, &me, PaneKind::Image, &path)?;
                return Ok(json!({"path": path, "pane": pane}));
            }
            Ok(json!({"path": path}))
        }
        .await;
        finish(r)
    }

    /// Gera um vídeo (Replicate) a partir de um texto, com quadros inicial e final opcionais. Salva em `assets`.
    #[tool(description = "Gera um vídeo (Replicate) a partir de texto e quadros opcionais; salva em <pasta do painel>/assets; open=true abre um painel de vídeo")]
    async fn video_generate(&self, Parameters(a): Parameters<VideoArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "video_generate", &parts);
        let r: AppResult<Value> = async {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            check_texts(&[a.prompt.as_str()])?;
            if let Some(m) = a.model.as_deref() {
                plain_arg(m)?;
            }
            let start = a.start_image.as_deref().map(|p| inside(&me.cwd, p)).transpose()?;
            let end = a.end_image.as_deref().map(|p| inside(&me.cwd, p)).transpose()?;
            let allow = squad_media(s, &me)?;
            let open = a.open.unwrap_or(false);
            let (dir, _slot) = gate(s, &me, "video", open)?;
            let path = media::video_generate(s, &a.prompt, start, end, a.model.clone(), &dir, allow.as_ref()).await?;
            if open {
                let pane = open_pane(s, &me, PaneKind::Video, &path)?;
                return Ok(json!({"path": path, "pane": pane}));
            }
            Ok(json!({"path": path}))
        }
        .await;
        finish(r)
    }

    /// Gera áudio: voz (texto falado, voz local ou ElevenLabs) ou efeito sonoro (ElevenLabs). Salva em `assets`.
    #[tool(description = "Gera áudio. voz: local grátis (ou ElevenLabs); efeito: ElevenLabs. Salva em <pasta do painel>/assets")]
    async fn audio_generate(&self, Parameters(a): Parameters<AudioArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "audio_generate", &parts);
        let r: AppResult<Value> = async {
            let s = &self.state;
            let me = caller_row(s, &parts)?;
            check_texts(&[a.text.as_str()])?;
            if let Some(v) = a.voice.as_deref() {
                plain_arg(v)?;
            }
            let allow = squad_media(s, &me)?;
            let kind = a.kind.clone().unwrap_or_else(|| "voz".into());
            let (dir, _slot) = gate(s, &me, "audio", false)?;
            let path = media::audio_generate(s, &a.text, &kind, a.voice.clone(), &dir, allow.as_ref()).await?;
            Ok(json!({"path": path}))
        }
        .await;
        finish(r)
    }

    /// Provedores de mídia habilitados, modelos e se cada chave está no cofre. Nunca devolve chaves.
    #[tool(description = "Provedores de mídia habilitados, modelos padrão e se cada chave está no cofre (sem valores)")]
    async fn media_providers(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "media_providers", &parts);
        finish((|| -> AppResult<Value> {
            let s = &self.state;
            caller_row(s, &parts)?;
            media::providers_info(s)
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::inside;

    #[test]
    fn reference_images_must_stay_inside_the_pane_folder() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("projeto");
        std::fs::create_dir_all(ws.join("img")).unwrap();
        std::fs::write(ws.join("img/a.png"), b"x").unwrap();
        std::fs::write(d.path().join("segredo.png"), b"x").unwrap();
        let cwd = ws.to_string_lossy().to_string();
        assert!(inside(&cwd, "img/a.png").is_ok());
        assert_eq!(inside(&cwd, "../segredo.png").unwrap_err().code(), "outside_workspace");
        assert_eq!(inside(&cwd, &d.path().join("segredo.png").to_string_lossy()).unwrap_err().code(), "outside_workspace");
        assert_eq!(inside(&cwd, "nao-existe.png").unwrap_err().code(), "not_found");
    }
}
