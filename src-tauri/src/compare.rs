//! Comparar abordagens (A1c, RM-42): o mesmo pedido em 2 a 4 variantes (CLI/modelo), cada uma uma missão livre
//! com worktree próprio. O resumo é só leitura; "Ficar com esta" integra a vencedora e arquiva as outras.

use crate::commands::archive_mission;
use crate::db::{CompareVariant, Mission, Workspace};
use crate::error::{fail, AppResult};
use crate::missions;
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::util::{new_id, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tauri::State;

type S<'a> = State<'a, Arc<AppState>>;

pub const MIN_VARIANTS: usize = 2;
pub const MAX_VARIANTS: usize = 4;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantSpec {
    pub label: String,
    pub cli: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// Uma variante no resumo da comparação (dados lidos do git, do banco e da verificação).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Variante {
    pub mission_id: String,
    pub label: String,
    pub status: String,
    pub files: usize,
    pub add: u32,
    pub del: u32,
    pub cost_usd: f64,
    pub duration_ms: i64,
    pub verify: Option<Value>,
    /// Algum painel da variante ainda está trabalhando (a comparação espera).
    pub working: bool,
    /// Linhas de `git status --porcelain` no worktree (0 se arquivada ou sem worktree): aviso antes de "Ficar com esta".
    pub uncommitted: usize,
}

/// Soma de arquivos (contagem, linhas +, linhas -) de uma lista de (add, del).
pub fn somar_arquivos(linhas: &[(u32, u32)]) -> (usize, u32, u32) {
    linhas.iter().fold((0, 0, 0), |(n, a, d), (x, y)| (n + 1, a + x, d + y))
}

/// Arquivos que a missão muda contra a base do worktree (0 sem worktree). Também usado pelo digest (A1.6).
pub fn arquivos_da_missao(m: &Mission) -> (usize, u32, u32) {
    let arquivos = match missions::worktree_of(m) {
        Ok(wt) => crate::review::changed_files(&wt.path, &wt.base_branch).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let pares: Vec<(u32, u32)> = arquivos.iter().map(|f| (f.add, f.del)).collect();
    somar_arquivos(&pares)
}

/// Tempo da variante: do início da missão até o fim (ou até agora, se ainda não fechou). Nunca negativo.
pub fn tempo_ms(inicio: i64, fim: Option<i64>, agora: i64) -> i64 {
    (fim.unwrap_or(agora) - inicio).max(0)
}

/// Espaço livre no volume do repositório e tamanho estimado de um worktree (`git count-objects -v`, em bytes).
/// ponytail: o tamanho do .git estima o checkout; repo cheio de binário versionado sai subestimado.
pub fn espaco(ws_path: &str) -> (Option<u64>, u64) {
    let livre = crate::modelos::espaco_livre(Path::new(ws_path));
    let estimado = crate::git::git(Path::new(ws_path), &["count-objects", "-v"])
        .map(|saida| {
            saida
                .lines()
                .filter_map(|l| l.split_once(": "))
                .filter(|(k, _)| *k == "size" || *k == "size-pack")
                .filter_map(|(_, v)| v.trim().parse::<u64>().ok())
                .sum::<u64>()
                * 1024
        })
        .unwrap_or(0);
    (livre, estimado)
}

/// Uma variante: missão livre com worktree próprio, a linha do grupo e a fila do primeiro painel.
fn criar_variante(s: &AppState, ws: &Workspace, group: &str, nome: &str, prompt: &str, v: &VariantSpec, permission: Permission, criadas: &mut Vec<Mission>) -> AppResult<()> {
    let (m, cwd) = missions::new_livre(ws, &format!("{nome} ({})", v.label.trim()), true)?;
    s.db.insert_mission(&m)?;
    criadas.push(m.clone());
    crate::worktree_cache::acelerar_com(s, ws, &m, false);
    s.db.insert_compare_variant(group, &m.id, v.label.trim())?;
    let spec = PaneSpec {
        workspace_id: ws.id.clone(),
        mission_id: Some(m.id.clone()),
        kind: PaneKind::Agent,
        cli: Some(v.cli.trim().to_string()),
        model: v.model.clone().filter(|x| !x.trim().is_empty()),
        permission,
        cwd,
        prompt: Some(prompt.to_string()),
        ..Default::default()
    };
    s.enqueue_mission(&m.id, vec![spec]);
    Ok(())
}

/// Falhou no meio: as variantes já criadas saem (fila cancelada, worktree removido) e somem do grupo.
fn desfazer(s: &AppState, criadas: &[Mission]) {
    for m in criadas {
        if let Err(e) = archive_mission(s, m.id.clone()) {
            tracing::warn!("variante {} não arquivada no rollback da comparação: {}", m.id, e.code());
        }
        let _ = s.db.delete_compare_variant(&m.id);
    }
}

/// Cria as N missões (uma por variante, cada uma com o mesmo prompt) e grava o grupo. Falha no meio desfaz as criadas.
pub fn start(s: &AppState, workspace_id: &str, name: &str, prompt: &str, variants: &[VariantSpec], permission: Permission) -> AppResult<Vec<CompareVariant>> {
    if !(MIN_VARIANTS..=MAX_VARIANTS).contains(&variants.len()) {
        return fail("bad_variants", format!("escolha de {MIN_VARIANTS} a {MAX_VARIANTS} variantes"));
    }
    if prompt.trim().is_empty() {
        return fail("bad_prompt", "escreva o pedido que todas as variantes vão receber");
    }
    if variants.iter().any(|v| v.label.trim().is_empty() || v.cli.trim().is_empty()) {
        return fail("bad_variants", "cada variante precisa de nome e CLI");
    }
    let clis = crate::util::lk(&s.providers).clis.clone();
    for v in variants {
        if !clis.iter().any(|c| c.id == v.cli.trim()) {
            return fail("bad_variants", format!("CLI desconhecida: {}", v.cli.trim()));
        }
        if let Some(model) = v.model.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
            crate::mcp::tools_panes::plain_arg(model)?;
        }
    }
    let ws = s.db.get_workspace(workspace_id)?;
    let n = variants.len();
    if s.list_panes(workspace_id)?.len() + n > crate::mcp::MCP_PANE_MAX {
        return fail("too_many_panes", format!("não cabem {n} painéis a mais neste workspace (limite de {}); feche algum antes", crate::mcp::MCP_PANE_MAX));
    }
    if let (Some(livre), estimado) = espaco(&ws.path) {
        if livre < estimado.saturating_mul(n as u64) {
            return fail("disk_low", "espaço livre insuficiente para as cópias de trabalho das variantes");
        }
    }
    let group = new_id();
    let nome = if name.trim().is_empty() { "Comparação" } else { name.trim() };
    let mut criadas = Vec::new();
    for v in variants {
        if let Err(e) = criar_variante(s, &ws, &group, nome, prompt, v, permission, &mut criadas) {
            desfazer(s, &criadas);
            return Err(e);
        }
    }
    s.db.compare_variants(&group)
}

/// Resumo das variantes de um grupo (leitura: git diff contra a base, custo do banco, última verificação).
pub fn resumo(s: &AppState, group_id: &str) -> AppResult<Value> {
    let vs = s.db.compare_variants(group_id)?;
    if vs.is_empty() {
        return fail("bad_group", "comparação não encontrada");
    }
    let agora = now_ms();
    let mut linhas = Vec::new();
    for v in &vs {
        let m: Mission = s.db.get_mission(&v.mission_id)?;
        let (files, add, del) = arquivos_da_missao(&m);
        let custo: f64 = s.db.mission_totals(&m.workspace_id)?.into_iter().find(|t| t.0 == m.id).map_or(0.0, |t| t.3);
        let paineis = s.db.mission_panes(&m.id)?;
        let inicio = paineis.iter().map(|p| p.created_at).min().unwrap_or(m.created_at);
        let working = paineis.iter().any(|p| p.closed_at.is_none() && matches!(p.status.as_str(), "trabalhando" | "iniciando"));
        let fim = if paineis.iter().any(|p| p.closed_at.is_none()) { None } else { paineis.iter().filter_map(|p| p.closed_at).max() };
        let verify = crate::verificacao::visao(s, &m.id)?.get("ultimo").filter(|x| !x.is_null()).cloned();
        let uncommitted = match (m.archived_at, missions::worktree_of(&m)) {
            (None, Ok(wt)) => crate::git::git(Path::new(&wt.path), &["status", "--porcelain"]).map_or(0, |saida| saida.lines().filter(|l| !l.trim().is_empty()).count()),
            _ => 0,
        };
        linhas.push(Variante {
            uncommitted,
            mission_id: m.id.clone(),
            label: v.label.clone(),
            status: m.status.clone(),
            files,
            add,
            del,
            cost_usd: custo,
            duration_ms: tempo_ms(inicio, fim, agora),
            verify,
            working,
        });
    }
    let (arquivos, add, del) = linhas.iter().fold((0, 0, 0), |(n, a, d), l| (n + l.files, a + l.add, d + l.del));
    let custo: f64 = linhas.iter().map(|l| l.cost_usd).sum();
    let pronta = !linhas.iter().any(|l| l.working);
    Ok(serde_json::json!({ "groupId": group_id, "pronta": pronta, "variantes": linhas, "total": { "files": arquivos, "add": add, "del": del, "costUsd": custo } }))
}

/// "Ficar com esta": integra a vencedora pelo fluxo normal e arquiva as outras. Idempotente: a integração pula se a
/// vencedora já está concluída, e as perdedoras já arquivadas somem; as que falham voltam em `falhas` (pelo rótulo).
pub fn keep(s: &AppState, group_id: &str, mission_id: &str) -> AppResult<Value> {
    let vs = s.db.compare_variants(group_id)?;
    if !vs.iter().any(|v| v.mission_id == mission_id) {
        return fail("bad_group", "a missão não pertence a esta comparação");
    }
    for v in &vs {
        let m = s.db.get_mission(&v.mission_id)?;
        let trabalhando = s.db.mission_panes(&m.id)?.iter().any(|p| p.closed_at.is_none() && matches!(p.status.as_str(), "trabalhando" | "pergunta" | "iniciando"));
        if trabalhando {
            return fail("busy", format!("a variante {} ainda está trabalhando; espere todas terminarem", v.label));
        }
    }
    let vencedora = s.db.get_mission(mission_id)?;
    if vencedora.archived_at.is_some() {
        return fail("bad_state", "a variante escolhida já foi arquivada");
    }
    if vencedora.status != "concluida" {
        missions::integrate_mission(s, mission_id)?;
    }
    let mut falhas = Vec::new();
    for v in vs.iter().filter(|v| v.mission_id != mission_id) {
        if s.db.get_mission(&v.mission_id)?.archived_at.is_some() {
            continue;
        }
        if archive_mission(s, v.mission_id.clone()).is_err() {
            falhas.push(v.label.clone());
        }
    }
    Ok(json!({ "integrada": true, "falhas": falhas }))
}

#[tauri::command(async)]
pub fn compare_start(s: S<'_>, workspace_id: String, name: String, prompt: String, variants: Vec<VariantSpec>, permission: Option<Permission>) -> AppResult<Vec<CompareVariant>> {
    // o padrão é o do assistente de painel (Perguntar), não Auto
    start(&s, &workspace_id, &name, &prompt, &variants, permission.unwrap_or_default())
}

/// Espaço livre e tamanho estimado de uma cópia de trabalho do workspace (o formulário avisa antes de criar).
#[tauri::command(async)]
pub fn compare_espaco(s: S<'_>, workspace_id: String) -> AppResult<Value> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let (livre, estimado) = espaco(&ws.path);
    Ok(json!({ "livreBytes": livre, "estimadoBytes": estimado }))
}

#[tauri::command(async)]
pub fn compare_get(s: S<'_>, group_id: String) -> AppResult<Value> {
    resumo(&s, &group_id)
}

#[tauri::command(async)]
pub fn compare_keep(s: S<'_>, group_id: String, mission_id: String) -> AppResult<Value> {
    keep(&s, &group_id, &mission_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soma_de_arquivos_e_linhas() {
        assert_eq!(somar_arquivos(&[]), (0, 0, 0));
        assert_eq!(somar_arquivos(&[(3, 1), (10, 0), (0, 4)]), (3, 13, 5));
    }

    #[test]
    fn tempo_nunca_negativo_e_usa_agora_com_painel_aberto() {
        assert_eq!(tempo_ms(1_000, Some(4_000), 9_000), 3_000);
        assert_eq!(tempo_ms(1_000, None, 9_000), 8_000);
        assert_eq!(tempo_ms(5_000, Some(1_000), 9_000), 0);
    }

    #[test]
    fn grupo_exige_de_duas_a_quatro_variantes() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        let uma = vec![VariantSpec { label: "a".into(), cli: "claude".into(), model: None }];
        let err = start(&s, "ws", "x", "faça", &uma, Permission::Pend).unwrap_err();
        assert_eq!(err.code(), "bad_variants");
        let cinco: Vec<VariantSpec> = (0..5).map(|i| VariantSpec { label: format!("v{i}"), cli: "claude".into(), model: None }).collect();
        assert_eq!(start(&s, "ws", "x", "faça", &cinco, Permission::Pend).unwrap_err().code(), "bad_variants");
    }

    #[test]
    fn variante_fora_do_grupo_nao_e_integrada() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        s.db.insert_compare_variant("g1", "m1", "a").unwrap();
        assert_eq!(keep(&s, "g1", "outra").unwrap_err().code(), "bad_group");
        assert_eq!(s.db.compare_group_of("m1").unwrap().as_deref(), Some("g1"));
    }

    #[test]
    fn cli_desconhecida_e_modelo_com_traco_sao_recusados() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        let fora = vec![VariantSpec { label: "a".into(), cli: "nao-existe".into(), model: None }, VariantSpec { label: "b".into(), cli: "nao-existe".into(), model: None }];
        assert_eq!(start(&s, "ws", "x", "faça", &fora, Permission::Pend).unwrap_err().code(), "bad_variants");
        let cli = crate::util::lk(&s.providers).clis.first().map(|c| c.id.clone()).unwrap();
        let modelo = vec![VariantSpec { label: "a".into(), cli: cli.clone(), model: Some("--x".into()) }, VariantSpec { label: "b".into(), cli, model: None }];
        assert_eq!(start(&s, "ws", "x", "faça", &modelo, Permission::Pend).unwrap_err().code(), "bad_arg");
    }
}
