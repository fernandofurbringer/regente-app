use crate::agentic::{self, AgenticConfig};
use crate::cost::prices::Prices;
use crate::db::{clean_group_name, CronJob, Decision, Gate, Mission, PaneGroup, PaneRow, UsageRow, Workspace};
use crate::error::{fail, AppError, AppResult};
use crate::files::{self, FileText, FsEntry};
use crate::missions;
use crate::providers::{self, Detected, PaneKind, PaneSpec, ProvidersConfig};
use crate::recipes::{self, RecipeDef};
use crate::runtime::AppState;
use crate::settings::{save_json, Settings};
use crate::squads;
use crate::util::{lk, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, Response};
use tauri::State;

type S<'a> = State<'a, Arc<AppState>>;

// ---------- app
#[tauri::command]
pub fn app_info(s: S<'_>) -> Value {
    // CurrentBuildNumber (xterm's windowsPty.buildNumber); 0 if unknown
    let build: u32 = sysinfo::System::kernel_version().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    json!({ "dataDir": s.data, "version": env!("CARGO_PKG_VERSION"), "windowsBuild": build })
}

#[tauri::command]
pub fn startup_path() -> Option<String> {
    crate::path_arg(&std::env::args().collect::<Vec<_>>())
        .map(|p| std::path::absolute(&p).map(|a| a.to_string_lossy().to_string()).unwrap_or(p))
}

// ---------- workspaces
/// A workspace is a folder that is not a drive root (the file tools would then reach the whole disk).
fn check_workspace_dir(path: &str) -> AppResult<()> {
    let p = Path::new(path);
    if !p.is_dir() {
        return fail("bad_path", format!("pasta inexistente: {path}"));
    }
    // canonical first: `C:\.` or `C:\x\..` name the drive root too
    let real = files::strip_verbatim(&std::fs::canonicalize(p)?);
    if real.parent().is_none() {
        return fail("bad_workspace", "escolha uma pasta, não a raiz do disco");
    }
    Ok(())
}

// async: o ssh do pré-requisito remoto pode demorar
#[tauri::command(async)]
pub fn workspace_open(s: S<'_>, path: String) -> AppResult<Workspace> {
    if crate::remote::is_remote(&path) {
        let rem = crate::remote::parse_ws_path(&path).filter(|r| r.path != "/").ok_or_else(|| AppError::new("bad_remote", "use ssh://alias/caminho/absoluto (não a raiz)"))?;
        let check = crate::remote::check(&rem.alias, &rem.path, "claude")?;
        let ws = s.db.upsert_workspace(&crate::remote::ws_url(&rem.alias, &rem.path))?;
        if check.cli_missing {
            s.emit("remote-notice", json!({"workspaceId": ws.id, "text": "claude não encontrado no PATH de login do servidor: os painéis de Claude não vão abrir até instalá-lo."}));
        }
        return Ok(ws);
    }
    check_workspace_dir(&path)?;
    let ws = s.db.upsert_workspace(&path)?;
    crate::projeto::on_open(&s, &ws);
    Ok(ws)
}

/// Hosts do `~/.ssh/config` para o diálogo "Abrir remoto".
#[tauri::command]
pub fn ssh_aliases() -> Vec<String> {
    crate::remote::ssh_aliases()
}

/// Sessões tmux externas (não `regente-*`) do servidor de um workspace remoto.
#[tauri::command(async)]
pub fn remote_sessions(s: S<'_>, workspace_id: String) -> AppResult<Vec<String>> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let rem = crate::remote::remote_cwd(&ws.path)?.ok_or_else(|| AppError::new("not_remote", "o workspace não é remoto"))?;
    Ok(crate::remote::list_sessions(&rem.alias)?.into_iter().filter(|(_, external)| *external).map(|(name, _)| name).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_takes_account_model_and_route_from_the_closed_row() {
        let row = PaneRow { account: Some("trabalho".into()), model: Some("opus".into()), route: Some("rota-x".into()), ..Default::default() };
        let spec = resume_spec("ws", "C:/proj", "claude".into(), "sess".into(), Some(&row));
        assert_eq!((spec.account.as_deref(), spec.model.as_deref(), spec.route.as_deref()), (Some("trabalho"), Some("opus"), Some("rota-x")));
        assert_eq!(spec.resume_session.as_deref(), Some("sess"));
        let bare = resume_spec("ws", "C:/proj", "claude".into(), "sess".into(), None);
        assert!(bare.account.is_none() && bare.model.is_none() && bare.route.is_none());
    }

    #[test]
    fn resuming_a_session_that_is_already_open_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), std::path::PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        lk(&s.providers).clis.push(providers::CliDef { id: "claude".into(), label: "Claude".into(), command: "cmd.exe".into(), session_resume: vec!["--resume".into()], ..Default::default() });
        let ws = s.db.upsert_workspace(&d.path().join("proj").to_string_lossy()).unwrap();
        let sid = "11111111-2222-3333-4444-555555555555";
        s.db.insert_pane(&PaneRow { id: "p".into(), workspace_id: ws.id.clone(), kind: "agent".into(), name: "p".into(), cwd: "C:/".into(), status: "ocioso".into(), session_id: Some(sid.into()), ..Default::default() }).unwrap();
        assert_eq!(resume_session(&s, ws.id.clone(), "claude".into(), sid.into()).unwrap_err().code(), "already_open");
    }

    #[test]
    fn drive_root_is_refused_as_workspace() {
        let root = std::env::temp_dir().ancestors().last().unwrap().to_string_lossy().to_string();
        assert_eq!(check_workspace_dir(&root).unwrap_err().code(), "bad_workspace");
        assert_eq!(check_workspace_dir(&format!("{root}.")).unwrap_err().code(), "bad_workspace");
        assert!(check_workspace_dir(&std::env::temp_dir().to_string_lossy()).is_ok());
    }

    #[test]
    fn full_history_joins_disk_then_memory_and_keeps_the_newest_8mb() {
        assert_eq!(full_history(None, Vec::new()), None, "sem disco e sem buffer: nada");
        assert_eq!(full_history(None, b"ao vivo".to_vec()).unwrap(), b"ao vivo", "buffer sozinho já vale");
        assert_eq!(full_history(Some(b"antigo ".to_vec()), b"novo".to_vec()).unwrap(), b"antigo novo");
        let big = full_history(Some(vec![b'a'; 8 * 1024 * 1024]), b"fim".to_vec()).unwrap();
        assert_eq!(big.len(), 8 * 1024 * 1024);
        assert!(big.ends_with(b"fim"), "o corte tira o começo, não o fim");
    }

    #[test]
    fn adopting_a_worktree_tracks_its_branch_and_archiving_keeps_it() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().join("dados"), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for a in [vec!["init", "-b", "main"], vec!["config", "user.name", "T"], vec!["config", "user.email", "t@e.com"], vec!["config", "init.defaultBranch", "main"]] {
            crate::git::git(&repo, &a).unwrap();
        }
        std::fs::write(repo.join("a.txt"), "x").unwrap();
        crate::git::git(&repo, &["add", "."]).unwrap();
        crate::git::git(&repo, &["commit", "-m", "base"]).unwrap();
        let amb = d.path().join("ambiente01");
        crate::git::git(&repo, &["worktree", "add", "--detach", &amb.to_string_lossy()]).unwrap();
        let ws = s.db.upsert_workspace(&repo.to_string_lossy()).unwrap();

        let list = worktrees_of(&s, &ws).unwrap();
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!((list[0].branch.as_deref(), list[0].adopted), (None, false));

        let m = adopt_worktree(&s, &ws, &list[0].path).unwrap();
        assert!(m.adopted && m.branch.is_none());
        assert_eq!(m.base_branch.as_deref(), Some("main"));
        assert!(worktrees_of(&s, &ws).unwrap()[0].adopted);
        assert_eq!(adopt_worktree(&s, &ws, &list[0].path).unwrap_err().code(), "already_adopted");
        assert_eq!(adopt_worktree(&s, &ws, &d.path().join("nada").to_string_lossy()).unwrap_err().code(), "bad_worktree");

        // a branch muda por fora (ambiente.sh iniciar): o estado relê e grava no banco
        crate::git::git(&amb, &["switch", "-c", "spec/x"]).unwrap();
        let st = worktree_state_of(&s, &m.id).unwrap();
        assert_eq!(st.branch.as_deref(), Some("spec/x"));
        assert_eq!(s.db.get_mission(&m.id).unwrap().branch.as_deref(), Some("spec/x"));
        // o projeto passa a dizer a base (git.base): a missão já adotada segue sem adotar de novo
        crate::git::git(&repo, &["branch", "dev"]).unwrap();
        std::fs::create_dir_all(repo.join(".regente")).unwrap();
        std::fs::write(repo.join(".regente").join("projeto.json"), r#"{"versao": 1, "git": {"base": "dev"}}"#).unwrap();
        worktree_state_of(&s, &m.id).unwrap();
        assert_eq!(s.db.get_mission(&m.id).unwrap().base_branch.as_deref(), Some("dev"));

        assert_eq!(missions::integrate_mission(&s, &m.id).unwrap_err().code(), "adopted");
        let arquivada = archive_mission(&s, m.id.clone()).unwrap();
        assert_eq!(arquivada.status, "arquivada");
        assert!(amb.join("a.txt").exists(), "arquivar não tira o worktree do projeto");
        assert!(crate::git::git(&repo, &["worktree", "list"]).unwrap().contains("ambiente01"));
        // reverter arquivo também é do projeto, não do Regente
        assert_eq!(mission_revert_file_inner(&s, &m.id, "a.txt").unwrap_err().code(), "adopted");
        // arquivada, a pasta continua e pode ser adotada de novo
        let de_novo = adopt_worktree(&s, &ws, &list[0].path).unwrap();
        assert!(de_novo.adopted && de_novo.id != m.id);
        // pasta apagada por fora (worktree "prunable"): o git ainda a lista, mas adotar recusa
        let amb2 = d.path().join("ambiente02");
        crate::git::git(&repo, &["worktree", "add", "--detach", &amb2.to_string_lossy()]).unwrap();
        // o caminho vem do git (nome longo); o temp pode ter nome curto 8.3, então casa pelo sufixo
        let p2 = worktrees_of(&s, &ws).unwrap().into_iter().map(|w| w.path).find(|p| p.ends_with("ambiente02")).unwrap();
        std::fs::remove_dir_all(&amb2).unwrap();
        assert_eq!(adopt_worktree(&s, &ws, &p2).unwrap_err().code(), "no_worktree");
    }

}

#[tauri::command]
pub fn workspace_list(s: S<'_>) -> AppResult<Vec<Workspace>> {
    s.db.list_workspaces()
}

// async: fecha os processos dos painéis antes de esquecer o workspace
#[tauri::command(async)]
pub fn workspace_remove(s: S<'_>, id: String) -> AppResult<()> {
    // fila de uma missão em andamento continuaria abrindo processos invisíveis
    for m in s.db.list_missions(&id)? {
        s.cancel_mission(&m.id);
    }
    for p in s.db.open_panes(&id)? {
        let _ = s.close_pane(&p.id);
    }
    s.db.remove_workspace(&id)
}

// ---------- panes
#[tauri::command]
pub fn pane_list(s: S<'_>, workspace_id: String) -> AppResult<Vec<PaneRow>> {
    s.list_panes(&workspace_id)
}

/// Conta que o usuário usou por último nesta pasta com a CLI `cli` (padrão do "Abrir Claude Code" e do "Novo painel").
/// Uma conta removida em Configurações volta a ser a principal (None).
#[tauri::command]
pub fn workspace_last_account(s: S<'_>, workspace_id: String, cli: String) -> AppResult<Option<String>> {
    let conta = s.db.last_account(&workspace_id, &cli)?;
    let extras = lk(&s.settings).accounts.get(&cli).cloned().unwrap_or_default();
    Ok(conta.filter(|c| extras.contains(c)))
}

#[tauri::command(async)]
pub fn pane_spawn(s: S<'_>, mut spec: PaneSpec) -> AppResult<PaneRow> {
    // a pane opened while a worktree mission is selected runs in that worktree, not in the main checkout
    if let Some(m) = spec.mission_id.as_deref().and_then(|id| s.db.get_mission(id).ok()) {
        if let (Some(wt), Ok(ws)) = (m.worktree_path.as_deref(), s.db.get_workspace(&spec.workspace_id)) {
            if Path::new(wt).is_dir() && missions::same_dir(&spec.cwd, &ws.path) {
                spec.cwd = missions::mission_cwd(Path::new(&ws.path), Path::new(wt)).to_string_lossy().into_owned();
            }
        }
    }
    s.spawn_pane(spec)
}

#[tauri::command]
pub fn pane_attach(s: S<'_>, webview: tauri::Webview, id: String, on_data: Channel<Response>) -> AppResult<u64> {
    // tagged with the webview: a reload/closed window drops its (dead) channels via `detach_owner`
    s.attach(&id, webview.label(), Box::new(move |b: &[u8]| on_data.send(Response::new(b.to_vec())).is_ok()))
}

#[tauri::command]
pub fn pane_detach(s: S<'_>, id: String, sink: u64) {
    s.panes.detach(&id, sink)
}

/// Execuções de shell de um painel do Claude (painel Execuções), da mais antiga para a mais nova.
#[tauri::command]
pub fn exec_lista(s: S<'_>, pane: String) -> Vec<crate::execucoes::Exec> {
    crate::util::lk(&s.execs).resumos(&pane)
}

/// Saída de uma execução do painel, a partir de `desde` (bytes; no máximo 256 KB por chamada). Só o painel do Claude
/// dono da execução alcança o id dela: a chave é (painel, id).
#[tauri::command]
pub fn exec_get(s: S<'_>, pane_id: String, exec_id: String, desde: u64) -> Option<crate::execucoes::Trecho> {
    crate::util::lk(&s.execs).trecho(&pane_id, &exec_id, desde)
}

/// "Fixar" de um painel promovido: não fecha sozinho depois do sucesso.
#[tauri::command]
pub fn exec_fixar(s: S<'_>, pane: String, fixar: bool) {
    crate::util::lk(&s.execs).fixar(&pane, fixar);
}

/// "Desligar execuções neste painel" (ou religar): o painel deixa de registrar os comandos dele até o app reiniciar.
#[tauri::command]
pub fn exec_desligar(s: S<'_>, pane: String, desligar: bool) -> AppResult<()> {
    s.exec_desligar(&pane, desligar)
}

#[tauri::command]
pub fn exec_desligado(s: S<'_>, pane: String) -> bool {
    s.exec_desligado(&pane)
}

/// "Mostrar execuções do painel ativo": abre (ou acha) o painel Execuções do painel do Claude.
#[tauri::command(async)]
pub fn exec_painel(s: S<'_>, pane: String) -> AppResult<String> {
    s.abrir_execucoes(&pane)
}

/// "Abrir em painel" de uma execução: promove agora, respeitando o teto.
#[tauri::command(async)]
pub fn exec_promover(s: S<'_>, pane: String, exec_id: String) -> AppResult<()> {
    s.promover_execucao(&pane, &exec_id)
}

/// "Fechar execuções concluídas" do painel: fecha os painéis promovidos concluídos e não fixados. Devolve quantos.
#[tauri::command(async)]
pub fn exec_fechar_concluidas(s: S<'_>, pane: String) -> AppResult<usize> {
    s.fechar_execucoes_concluidas(&pane)
}

/// Conversa de um painel de subagente (lida a cada segundo enquanto ele trabalha): `{items, description}`.
#[tauri::command(async)]
pub fn subagent_items(s: S<'_>, id: String) -> AppResult<serde_json::Value> {
    let row = s.db.get_pane(&id)?;
    if row.kind != "subagent" {
        return fail("not_subagent", "este painel não é de subagente");
    }
    Ok(match row.url_or_path.as_deref() {
        Some(path) => crate::transcript::subagent_view(std::path::Path::new(path)),
        None => serde_json::json!({"items": [], "description": null}),
    })
}

#[tauri::command]
pub fn pane_write(s: S<'_>, id: String, data: String) -> AppResult<()> {
    s.write_input(&id, data.as_bytes())
}

/// Troca modelo e/ou esforço de um painel de IA (P1.8). `frame`: a tela mostrada enquanto ele reinicia, se preciso.
/// `fresh`: reinicia do zero quando não há sessão para retomar (o usuário confirmou).
#[tauri::command(async)]
pub fn pane_switch_model(s: S<'_>, id: String, model: Option<String>, effort: Option<String>, frame: String, fresh: bool) -> AppResult<PaneRow> {
    s.switch_model(&id, model.as_deref(), effort.as_deref(), &frame, fresh)
}

/// Conectores MCP do Regente (entregues a todo painel do Claude, em qualquer conta). Ver `conectores`.
#[tauri::command]
pub fn conectores_list(s: S<'_>) -> Vec<crate::conectores::Conector> {
    s.conectores()
}

/// Cadastra ou troca um conector: `config` é o JSON no formato do `mcpServers` do Claude.
#[tauri::command]
pub fn conector_save(s: S<'_>, nome: String, config: String) -> AppResult<Vec<crate::conectores::Conector>> {
    let v: serde_json::Value = serde_json::from_str(&config).map_err(|e| AppError::new("bad_connector", format!("JSON inválido: {e}")))?;
    s.conector_salvar(nome.trim(), &v)
}

/// O Context7, com a chave opcional.
#[tauri::command]
pub fn conector_context7(s: S<'_>, chave: Option<String>) -> AppResult<Vec<crate::conectores::Conector>> {
    s.conector_salvar(crate::conectores::CONTEXT7, &crate::conectores::context7(chave.as_deref()))
}

#[tauri::command]
pub fn conector_toggle(s: S<'_>, nome: String, ativo: bool) -> AppResult<Vec<crate::conectores::Conector>> {
    s.conector_ativar(&nome, ativo)
}

#[tauri::command]
pub fn conector_delete(s: S<'_>, nome: String) -> AppResult<Vec<crate::conectores::Conector>> {
    s.conector_remover(&nome)
}

#[tauri::command]
pub fn conectores_importaveis(s: S<'_>) -> Vec<crate::conectores::Importavel> {
    s.conectores_importaveis()
}

#[tauri::command]
pub fn conector_import(s: S<'_>, nome: String, conta: String) -> AppResult<Vec<crate::conectores::Conector>> {
    s.conector_importar(&nome, &conta)
}

/// Continua o painel do Claude em outra conta, retomando a conversa (ver `troca_conta`).
#[tauri::command(async)]
pub fn pane_switch_account(s: S<'_>, id: String, account: String, frame: String) -> AppResult<PaneRow> {
    s.switch_account(&id, &account, &frame)
}

#[tauri::command]
pub fn pane_resize(s: S<'_>, id: String, cols: u16, rows: u16) -> AppResult<()> {
    if s.panes.is_alive(&id) { s.panes.resize(&id, cols.max(2), rows.max(2)) } else { Ok(()) }
}

#[tauri::command(async)]
pub fn pane_close(s: S<'_>, id: String) -> AppResult<()> {
    s.close_pane(&id)
}

#[tauri::command]
pub fn pane_rename(s: S<'_>, id: String, name: String) -> AppResult<()> {
    s.rename_pane(&id, &name)
}

#[tauri::command]
pub fn pane_reorder(s: S<'_>, ids: Vec<String>) -> AppResult<()> {
    s.db.set_pane_order(&ids)
}

/// Servidores MCP do Claude que o perfil `custom` pode manter (só leitura; nomes, origem e transporte).
#[tauri::command(async)]
pub fn mcp_servers_list(cli: String, cwd: String) -> AppResult<crate::mcp::user_servers::Listing> {
    if crate::remote::is_remote(&cwd) {
        return fail("unsupported", "não suportado em workspace remoto");
    }
    Ok(crate::mcp::user_servers::listing(&cli, &crate::mcp::user_servers::claude_json_path(&crate::util::home_dir()), std::path::Path::new(&cwd)))
}

/// Estado de atenção (bloqueada, pronta, motivos) de cada missão do workspace.
#[tauri::command(async)]
pub fn mission_attention(s: S<'_>, workspace_id: String) -> AppResult<Vec<crate::attention::MissionAttention>> {
    s.mission_attention(&workspace_id)
}

#[tauri::command]
pub fn pane_group_list(s: S<'_>, workspace_id: String) -> AppResult<Vec<PaneGroup>> {
    s.db.list_pane_groups(&workspace_id)
}

#[tauri::command]
pub fn pane_group_create(s: S<'_>, workspace_id: String, name: String) -> AppResult<PaneGroup> {
    s.db.create_pane_group(&workspace_id, &clean_group_name(&name)?)
}

#[tauri::command]
pub fn pane_group_update(s: S<'_>, id: String, name: Option<String>, collapsed: Option<bool>) -> AppResult<()> {
    let name = name.map(|n| clean_group_name(&n)).transpose()?;
    s.db.update_pane_group(&id, name.as_deref(), collapsed)
}

#[tauri::command]
pub fn pane_group_delete(s: S<'_>, id: String) -> AppResult<()> {
    s.db.delete_pane_group(&id)
}

#[tauri::command]
pub fn pane_set_group(s: S<'_>, id: String, group_id: Option<String>) -> AppResult<()> {
    s.db.set_pane_group(&id, group_id.as_deref())
}

#[tauri::command(async)]
pub fn pane_hibernate(s: S<'_>, id: String, frame: String) -> AppResult<()> {
    s.hibernate(&id, &frame)
}

#[tauri::command]
pub fn pane_wake(s: S<'_>, id: String) {
    s.wake(&id)
}

// ---------- cron
#[tauri::command]
pub fn cron_list(s: S<'_>, workspace_id: Option<String>) -> AppResult<Vec<CronJob>> {
    s.db.cron_jobs(workspace_id.as_deref())
}

#[tauri::command]
pub fn cron_save(s: S<'_>, job: CronJob) -> AppResult<CronJob> {
    crate::cronjobs::save(&s, job)
}

#[tauri::command]
pub fn cron_delete(s: S<'_>, id: String) -> AppResult<()> {
    s.db.delete_cron(&id)
}

// async: a run may spawn a pane (process start), which must not block the UI thread
#[tauri::command(async)]
pub fn cron_run_now(s: S<'_>, id: String) -> AppResult<()> {
    crate::cronjobs::run_now(&s, &id)
}

#[tauri::command]
pub fn cron_preview(expr: String) -> AppResult<Vec<i64>> {
    crate::cronjobs::preview(&expr, crate::util::now_ms())
}

#[tauri::command]
pub fn pane_set_resident(s: S<'_>, id: String, resident: bool) -> AppResult<()> {
    s.db.set_pane_resident(&id, resident)
}

#[tauri::command]
pub fn pane_focus(s: S<'_>, id: String) {
    s.rt().focused = Some(id);
}

/// Segundos desde a última saída do painel (None = sem processo, ou ainda sem saída). Base do aviso de cache na troca de modelo.
#[tauri::command]
pub fn pane_idle_secs(s: S<'_>, id: String) -> Option<u64> {
    s.panes.activity(&id).and_then(|(_, last)| last).map(|t| t.elapsed().as_secs())
}

/// Chip "Compactar" do medidor de contexto: compacta agora, com o foco da missão (mesmas travas do automático).
#[tauri::command]
pub fn pane_compact(s: S<'_>, id: String) -> AppResult<()> {
    crate::compactacao::acao_manual(&s, &id, false)
}

/// Chip "Recomeçar com memória": `/clear` e o brief de retomada (mesmas travas do automático).
#[tauri::command]
pub fn pane_fresh_start(s: S<'_>, id: String) -> AppResult<()> {
    crate::compactacao::acao_manual(&s, &id, true)
}

#[tauri::command]
pub fn pane_hibernate_reasons(s: S<'_>, id: String, manual: bool) -> AppResult<Vec<String>> {
    crate::hibernate::reasons_for(&s, &id, manual)
}

#[tauri::command]
pub fn pane_frame(s: S<'_>, id: String) -> AppResult<Option<String>> {
    s.db.get_frame(&id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneDetails {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: String,
    session_id: Option<String>,
    route_label: Option<String>,
    pid: Option<u32>,
    memory_mb: Option<u64>,
    skills: Vec<String>,
}

/// Monitor de recursos (RAM/CPU por painel, interface e Regente); reaproveita a leitura de até 2 s atrás.
#[tauri::command(async)]
pub fn resources_snapshot(s: S<'_>) -> AppResult<crate::resources::Snapshot> {
    let panes: Vec<(String, String, String, u32)> = s
        .panes
        .ids()
        .into_iter()
        .filter_map(|id| {
            let pid = s.panes.pid(&id)?;
            let row = s.db.get_pane(&id).ok()?;
            Some((id, row.name, row.status, pid))
        })
        .collect();
    let procs = crate::resources::procs();
    let mut snap = crate::resources::build(std::process::id(), &panes, &procs);
    snap.cpu_ready = crate::resources::cpu_ready();
    Ok(snap)
}

#[tauri::command(async)]
pub fn pane_details(s: S<'_>, id: String) -> AppResult<PaneDetails> {
    let plan = s.panes.plan(&id).ok_or_else(|| AppError::new("not_running", "painel não está rodando"))?;
    let pid = s.panes.pid(&id);
    // memória da árvore inteira do painel (cmd -> claude -> node -> servidores MCP), como no monitor de recursos
    let memory_mb = pid.and_then(|p| crate::resources::tree_mem_mb(p, &crate::resources::procs()));
    // modo sensível: valores que vêm do cofre (globais ou do workspace) somem da tela, qualquer que seja o nome
    let ws = s.db.get_pane(&id)?.workspace_id;
    let vault_vals = if s.is_sensitive(&ws) { s.vault_values_for(&ws)? } else { vec![] };
    let env = plan
        .env
        .iter()
        .map(|(k, v)| {
            let secret = ["TOKEN", "KEY", "SECRET"].iter().any(|w| k.to_uppercase().contains(w));
            (k.clone(), if secret || vault_vals.contains(v) { "***".into() } else { v.clone() })
        })
        .collect();
    let skills = s.rt().pane_skills.get(&id).cloned().unwrap_or_default();
    Ok(PaneDetails { program: plan.program, args: plan.args, env, cwd: plan.cwd, session_id: plan.session_id, route_label: plan.route_label, pid, memory_mb, skills })
}

/// Histórico completo de um painel (disco + buffer em memória já não cobre), sem ANSI, para ler na tela.
#[tauri::command(async)]
pub fn pane_history(s: S<'_>, id: String) -> AppResult<String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return fail("bad_pane", "painel inválido");
    }
    // só a cauda do disco (seek até o fim): o que passar do teto nunca é lido
    let disk = crate::spill::tail(&s.data.join("panes").join(&id), HISTORY_CAP);
    let Some(bytes) = full_history(disk, s.panes.snapshot(&id)) else {
        return fail("no_history", "este painel ainda não tem saída registrada");
    };
    Ok(crate::orchestrator::strip_ansi(&String::from_utf8_lossy(&bytes)))
}

/// Teto do histórico lido para a tela (8 MB; o resto do disco nem é lido).
const HISTORY_CAP: usize = 8 * 1024 * 1024;

/// Histórico completo: disco (cauda, já limitada) + buffer em memória (recente), cortado nos últimos 8 MB.
/// `None` só se não há nenhum dos dois. O corte é em bytes, antes de tirar o ANSI.
fn full_history(disk: Option<Vec<u8>>, recent: Vec<u8>) -> Option<Vec<u8>> {
    if disk.is_none() && recent.is_empty() {
        return None;
    }
    let mut bytes = disk.unwrap_or_default();
    bytes.extend(recent);
    if bytes.len() > HISTORY_CAP {
        bytes.drain(..bytes.len() - HISTORY_CAP);
    }
    Some(bytes)
}

#[tauri::command]
pub fn restore_list(s: S<'_>, workspace_id: String) -> AppResult<Vec<PaneRow>> {
    s.db.restorable_panes(&workspace_id)
}

#[tauri::command(async)]
pub fn restore_apply(s: S<'_>, workspace_id: String, accept: bool, pane_ids: Option<Vec<String>>) -> AppResult<Vec<PaneRow>> {
    s.restore(&workspace_id, accept, pane_ids.as_deref())
}

// ---------- missions
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewMission {
    workspace_id: String,
    name: String,
    mode: String,
    worktree: bool,
    #[serde(default)]
    slots: Vec<PaneSpec>,
    squad_id: Option<String>,
    brief: Option<String>,
    recipe_id: Option<String>,
    /// Modo agêntico: harness e conta do piloto.
    #[serde(default)]
    pilot: Option<agentic::PilotArgs>,
    /// Modo agêntico: chefe (chain) ou torre; omitido = o padrão de agentic.json.
    #[serde(default)]
    style: Option<String>,
    /// Intake answers by question index; they win over the recipe's and the brief's autofill.
    #[serde(default)]
    answers: BTreeMap<usize, String>,
    /// Perfil de trabalho da missão (id de `perfis`); omitido = do projeto/padrão.
    #[serde(default)]
    profile: Option<String>,
}

#[tauri::command(async)]
pub fn mission_create(s: S<'_>, input: NewMission) -> AppResult<Mission> {
    create_mission(&s, input)
}

/// Opens a mission in any mode.
pub fn create_mission(s: &AppState, input: NewMission) -> AppResult<Mission> {
    if input.profile.as_deref().is_some_and(|p| !crate::perfil::IDS.contains(&p)) {
        return fail("bad_perfil", "perfil desconhecido");
    }
    if input.mode == "squad" {
        let squad = input.squad_id.as_deref().and_then(|id| squads::get(&s.data, id)).map_or_else(|| fail("bad_squad", "squad desconhecida"), Ok)?;
        let brief = input.brief.unwrap_or_default();
        if brief.trim().is_empty() {
            return fail("bad_brief", "escreva o brief da missão");
        }
        let recipe = input.recipe_id.as_deref().map(|id| recipes::get(&s.data, id).map_or_else(|| fail("bad_recipe", "receita desconhecida"), Ok)).transpose()?;
        if recipe.as_ref().and_then(|r| r.squad.as_deref()).is_some_and(|sq| sq != squad.id) {
            return fail("bad_recipe", "a receita é de outra squad");
        }
        let ws = s.db.get_workspace(&input.workspace_id)?;
        let mut answers = match &recipe {
            Some(r) => recipes::intake_answers(r, &squad, &brief),
            None => squads::autofill(&squad.intake, &brief),
        };
        answers.extend(input.answers);
        let opts = missions::MissionOpts { worktree: input.worktree, recipe_id: recipe.as_ref().map(|r| r.id.clone()), profile: input.profile.clone(), parent: None };
        let (m, _) = squads::start_squad_mission(&s, &ws, &squad, &input.name, &brief, answers, providers::Permission::Auto, &opts)?;
        return Ok(m);
    }
    if input.mode == "agentico" {
        let brief = input.brief.unwrap_or_default();
        let recipe = input.recipe_id.as_deref().map(|id| recipes::get(&s.data, id).map_or_else(|| fail("bad_recipe", "receita desconhecida"), Ok)).transpose()?;
        if recipe.as_ref().is_some_and(|r| r.mode != "agentico") {
            return fail("bad_recipe", "a receita não é agêntica");
        }
        let pilot = input.pilot.or_else(|| recipe.as_ref().and_then(|r| r.pilot.clone()).map(|h| agentic::PilotArgs { cli: h.cli, model: h.model, effort: h.effort, account: None })).unwrap_or_default();
        let style = input.style.or_else(|| recipe.as_ref().and_then(|r| r.style.clone())).unwrap_or_else(|| agentic::load(&s.data).default_style);
        let ws = s.db.get_workspace(&input.workspace_id)?;
        let worktree = input.worktree || recipe.as_ref().and_then(|r| r.worktree).unwrap_or(false);
        let opts = missions::MissionOpts { worktree, recipe_id: recipe.as_ref().map(|r| r.id.clone()), profile: input.profile.clone(), parent: None };
        let (m, _) = agentic::start_agentic_mission(&s, &ws, &input.name, &pilot, &brief, &style, &opts)?;
        return Ok(m);
    }
    if input.mode != "livre" {
        return fail("mode_unavailable", "modo desconhecido");
    }
    if input.slots.is_empty() || input.slots.len() > crate::queue::MAX_BATCH {
        return fail("bad_slots", format!("escolha de 1 a {} terminais", crate::queue::MAX_BATCH));
    }
    let ws = s.db.get_workspace(&input.workspace_id)?;
    let (mut m, cwd) = missions::new_livre(&ws, &input.name, input.worktree)?;
    m.profile = input.profile.clone();
    s.db.insert_mission(&m)?;
    crate::worktree_cache::acelerar(&s, &ws, &m);
    let specs = input
        .slots
        .into_iter()
        .map(|mut sp| {
            sp.workspace_id = ws.id.clone();
            sp.mission_id = Some(m.id.clone());
            sp.cwd = cwd.clone();
            sp
        })
        .collect();
    s.enqueue_mission(&m.id, specs);
    Ok(m)
}

#[tauri::command]
pub fn agentic_get(s: S<'_>) -> AgenticConfig {
    agentic::load(&s.data)
}

#[tauri::command]
pub fn agentic_save(s: S<'_>, config: AgenticConfig) -> AppResult<()> {
    agentic::save(&s.data, &config)
}

#[tauri::command]
pub fn voice_get(s: S<'_>) -> crate::stt::VoiceConfig {
    crate::stt::load(&s.data)
}

/// Turning global dictation on checks the hotkey before saving, then registers it (another app may own it).
#[tauri::command]
pub fn voice_save(app: tauri::AppHandle, s: S<'_>, config: crate::stt::VoiceConfig) -> AppResult<()> {
    if config.global_enabled {
        crate::dictation::parse_hotkey(&config.global_hotkey)?;
        // a dictation key equal to a Shot key is refused (Shot's own check refuses the other way round)
        let shot_cfg = crate::shot::current(&app, &s.data);
        if shot_cfg.hotkeys {
            crate::shot::validate(&shot_cfg, Some(&config.global_hotkey))?;
        }
    }
    // registered before saving: a key another app owns fails here, and the previous registration comes back
    let prev = crate::stt::load(&s.data);
    if let Err(e) = crate::dictation::sync(&app, &config) {
        let _ = crate::dictation::sync(&app, &prev);
        return Err(e);
    }
    if let Err(e) = crate::stt::save(&s.data, &config) {
        let _ = crate::dictation::sync(&app, &prev);
        return Err(e);
    }
    Ok(())
}

/// The audio arrives as a raw body (no JSON array of numbers); `mime` and `purpose` come in headers.
/// Returns the bytes (size checked before the copy), the mime and the purpose.
fn audio_request(request: &tauri::ipc::Request<'_>) -> AppResult<(Vec<u8>, String, String)> {
    let body = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes,
        tauri::ipc::InvokeBody::Json(_) => return fail("bad_audio", "o áudio precisa vir como corpo binário"),
    };
    if body.len() > crate::stt::MAX_AUDIO {
        return fail("audio_too_large", "áudio acima de 25 MB: grave em trechos menores");
    }
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mime = header("mime");
    let mime = if mime.is_empty() { "audio/webm".to_string() } else { mime };
    Ok((body.clone(), mime, header("purpose")))
}

#[tauri::command]
pub async fn voice_transcribe(s: S<'_>, request: tauri::ipc::Request<'_>) -> AppResult<String> {
    let (audio, mime, purpose) = audio_request(&request)?;
    crate::stt::transcribe(&s, audio, &mime, &purpose).await
}

/// The end of a global dictation: transcribes the audio and pastes the text into the focused app.
#[tauri::command]
pub async fn voice_global_finish(app: tauri::AppHandle, s: S<'_>, request: tauri::ipc::Request<'_>) -> AppResult<String> {
    let (audio, mime, _) = audio_request(&request)?;
    crate::dictation::finish(&app, s.inner(), audio, &mime).await
}

/// The recording could not start (no microphone): back to idle.
#[tauri::command]
pub fn voice_global_cancel(app: tauri::AppHandle) {
    crate::dictation::cancel(&app);
}

#[tauri::command]
pub fn voice_history_list(s: S<'_>) -> AppResult<Vec<crate::db::VoiceEntry>> {
    s.db.voice_history(100)
}

#[tauri::command]
pub fn voice_history_clear(s: S<'_>) -> AppResult<()> {
    s.db.clear_voice_history()
}

#[tauri::command]
pub fn media_get(s: S<'_>) -> crate::media::MediaConfig {
    crate::media::load(&s.data)
}

/// Settings → Mídia: attempts today per kind, against the caps.
#[tauri::command]
pub fn media_usage(s: S<'_>) -> AppResult<Value> {
    crate::media::usage_today(&s)
}

#[tauri::command]
pub fn media_save(s: S<'_>, config: crate::media::MediaConfig) -> AppResult<()> {
    crate::media::set_config(&s, &config)
}

/// Settings → Mídia → [Testar]: the cheapest call of one provider. Returns the file or the account it reached.
#[tauri::command]
pub async fn media_test(s: S<'_>, provider: String) -> AppResult<String> {
    crate::media::test(&s, &provider).await
}

#[tauri::command]
pub fn jarvis_get(s: S<'_>) -> crate::jarvis::JarvisConfig {
    crate::jarvis::load(&s.data)
}

#[tauri::command]
pub fn jarvis_save(s: S<'_>, config: crate::jarvis::JarvisConfig) -> AppResult<()> {
    crate::jarvis::save(&s.data, &config)
}

/// The user confirmed a write on screen (terminal, browser or danger pane): same write path as a direct one.
#[tauri::command]
pub fn jarvis_write_confirmed(s: S<'_>, pane_id: String, text: String, enter: bool) -> AppResult<()> {
    let row = s.db.get_pane(&pane_id)?;
    if row.closed_at.is_some() {
        return fail("pane_closed", "o painel já foi fechado");
    }
    s.write_input(&row.id, &crate::jarvis::write_bytes(&text, enter))
}

/// Classifies a fixed request with the configured Decisor, to check the model, key and latency.
#[tauri::command]
pub async fn decisor_test(s: S<'_>) -> AppResult<Value> {
    let cfg = agentic::load(&s.data);
    if cfg.decisor.kind == "none" {
        return fail("decisor_off", "configure um modelo Decisor antes de testar");
    }
    let secret = match cfg.decisor.secret.as_deref() {
        Some(n) => Some(s.secret(n)?.ok_or_else(|| AppError::new("missing_secret", format!("configure a chave '{n}' no Cofre")))?),
        None => None,
    };
    let prices = lk(&s.prices).clone();
    let agents: Vec<(String, String)> = crate::catalog::list_agents(&s.data, &crate::util::home_dir()).into_iter().map(|a| (a.name, a.description)).collect();
    let out = agentic::decide(&cfg.decisor, secret, &prices, &cfg.types, &agents, "corrigir o botão de login no mobile").await?;
    agentic::account(&s.db, &out)?;
    let Some(task) = out.task_type.clone() else { return fail("decisor_invalid", "o Decisor não devolveu um tipo válido") };
    Ok(json!({"taskType": task, "model": out.model, "latencyMs": out.latency_ms, "costUsd": out.cost_usd, "measured": out.measured}))
}

/// Decisions of the last `since_days` days (default 7), most recent first.
#[tauri::command]
pub fn decisions_list(s: S<'_>, since_days: Option<i64>) -> AppResult<Vec<Decision>> {
    let since = now_ms() - since_days.unwrap_or(7).clamp(0, 3650) * 86_400_000;
    s.db.decisions(since, 500)
}

#[tauri::command]
pub fn squads_list(s: S<'_>) -> Vec<crate::squads::SquadDef> {
    crate::squads::list(&s.data)
}

#[tauri::command]
pub fn squad_save(s: S<'_>, squad: crate::squads::SquadDef) -> AppResult<()> {
    crate::squads::save(&s.data, &squad)
}

/// The 20 most recent panes, as the Squad Former reads them: what each one cost and how long it ran.
#[tauri::command]
pub fn squad_former_brief(s: S<'_>) -> AppResult<Value> {
    let now = now_ms();
    let rows = s.db.recent_panes(20)?;
    Ok(json!(rows
        .iter()
        .map(|p| json!({
            "name": p.name,
            "cli": p.cli,
            "model": p.model,
            "effort": p.effort,
            "role": p.role,
            "agent": p.agent,
            "costUsd": p.cost_usd,
            "durationMin": (p.closed_at.unwrap_or(now) - p.created_at).max(0) / 60_000,
        }))
        .collect::<Vec<_>>()))
}

/// Memories of a workspace, most recent first (with a mission: that mission's and the workspace-wide ones).
#[tauri::command]
pub fn memory_list(s: S<'_>, workspace_id: String, mission_id: Option<String>) -> AppResult<Vec<crate::db::Memory>> {
    s.db.memories(&workspace_id, mission_id.as_deref(), 300)
}

#[tauri::command]
pub fn memory_search(s: S<'_>, workspace_id: String, q: String) -> AppResult<Vec<crate::db::Memory>> {
    crate::embed::buscar(&s.db, &workspace_id, &q, 100)
}

#[tauri::command]
pub fn memory_update(s: S<'_>, id: i64, text: String) -> AppResult<()> {
    let text = crate::memory::checked_text(&text)?;
    if !s.db.update_memory(id, &text)? {
        return fail("not_found", "memória não encontrada");
    }
    crate::embed::agendar(crate::embed::Tarefa::Vetorizar(id));
    Ok(())
}

#[tauri::command]
pub fn memory_delete(s: S<'_>, id: i64) -> AppResult<()> {
    s.db.delete_vector(id)?;
    s.db.delete_memory(id)
}

/// Memórias de um estado ("sugerida" ou "arquivada") do workspace (ou "_global"), das mais recentes.
#[tauri::command]
pub fn memory_list_status(s: S<'_>, workspace_id: String, status: String) -> AppResult<Vec<crate::db::Memory>> {
    if !["sugerida", "arquivada"].contains(&status.as_str()) {
        return fail("bad_status", "estado: sugerida ou arquivada");
    }
    s.db.memories_in(&workspace_id, &status, 300)
}

/// Muda o estado de uma memória: "ativa" (aceitar), "sugerida" ou "arquivada".
#[tauri::command]
pub fn memory_set_status(s: S<'_>, id: i64, status: String) -> AppResult<()> {
    if !["ativa", "sugerida", "arquivada"].contains(&status.as_str()) {
        return fail("bad_status", "estado: ativa, sugerida ou arquivada");
    }
    // aceitar passa pela mesma regra de dedup da gravação (funde numa ativa igual)
    if status == "ativa" {
        return crate::memory::activate(&s.db, id);
    }
    if !s.db.set_memory_status(id, &status)? {
        return fail("not_found", "memória não encontrada");
    }
    Ok(())
}

/// Arquivos de instruções do projeto que existem na raiz do workspace (só leitura).
#[tauri::command]
pub fn instrucoes_listar(s: S<'_>, workspace_id: String) -> AppResult<Vec<crate::instrucoes::Instrucao>> {
    crate::instrucoes::listar(&s, &workspace_id)
}

/// Sugestão da IA auxiliar para um arquivo de instruções, a partir das memórias aceitas. Não grava.
#[tauri::command(async)]
pub async fn instrucoes_sugerir(s: S<'_>, workspace_id: String, file: String) -> AppResult<crate::instrucoes::Sugestao> {
    crate::instrucoes::sugerir(&s, &workspace_id, &file).await
}

/// Grava o texto aprovado no arquivo, só se ele ainda tem o hash lido (sem perder edição).
#[tauri::command]
pub fn instrucoes_aplicar(s: S<'_>, workspace_id: String, file: String, texto: String, hash_original: String) -> AppResult<()> {
    crate::instrucoes::aplicar(&s, &workspace_id, &file, &texto, &hash_original)
}

/// Preferência global do usuário (Memória → Preferências globais): vale em todos os projetos e entra ativa.
#[tauri::command]
pub fn memory_add_global(s: S<'_>, text: String) -> AppResult<i64> {
    let text = crate::memory::checked_text(&text)?;
    crate::memory::save(&s.db, &crate::db::Memory { workspace_id: crate::memory::GLOBAL.into(), kind: "fato".into(), text, created_at: now_ms(), ..Default::default() })
}

/// Fixa ou desfixa uma memória (fixadas entram primeiro no brief e nunca expiram).
#[tauri::command]
pub fn memory_set_pinned(s: S<'_>, id: i64, pinned: bool) -> AppResult<()> {
    if !s.db.set_memory_pinned(id, pinned)? {
        return fail("not_found", "memória não encontrada");
    }
    Ok(())
}

/// Memory on/off for a workspace (off = its brief is not injected into new panes).
#[tauri::command]
pub fn memory_toggle(s: S<'_>, workspace_id: String, enabled: bool) -> AppResult<()> {
    let mut cur = lk(&s.settings);
    let mut next = cur.clone();
    next.memory_disabled.retain(|w| w != &workspace_id);
    if !enabled {
        next.memory_disabled.push(workspace_id);
    }
    save_json(&s.data.join("settings.json"), &next)?;
    *cur = next;
    Ok(())
}

/// Conversa do painel em Markdown (P1.6). Claude com JSONL da sessão; senão o texto do terminal (`screen`, ou o buffer do backend).
#[tauri::command(async)]
pub fn pane_transcript(s: S<'_>, id: String, full_tools: bool, screen: Option<String>) -> AppResult<String> {
    let row = s.db.get_pane(&id)?;
    if row.cli.as_deref() == Some("claude") && !crate::remote::is_remote(&row.cwd) {
        if let Some(sid) = row.session_id.as_deref() {
            let dir = s.config_dir_for("claude", row.account.as_deref());
            if let Some(path) = crate::cost::claude::find_session_file(&dir, sid) {
                let text = String::from_utf8_lossy(&std::fs::read(&path)?).into_owned();
                return Ok(crate::transcript::claude_markdown(&text, &row.name, full_tools));
            }
        }
    }
    let text = screen.unwrap_or_else(|| crate::orchestrator::strip_ansi(&String::from_utf8_lossy(&s.panes.snapshot(&id))));
    Ok(crate::transcript::terminal_markdown(&row.name, &text))
}

/// Busca em todos os painéis abertos (P1.7): o buffer de cada um, o histórico em disco (com teto de bytes) e, no Claude, a conversa da sessão. Até 200 resultados.
#[tauri::command(async)]
pub fn search_panes(s: S<'_>, query: String) -> AppResult<crate::search::Found> {
    let query = query.trim().to_string();
    let mut found = crate::search::Found::default();
    if query.is_empty() {
        return Ok(found);
    }
    let mut disk_left = crate::search::DISK_TOTAL;
    'panes: for ws in s.db.list_workspaces()? {
        for p in s.list_panes(&ws.id)? {
            let owner = crate::search::Owner { pane_id: p.id.clone(), pane_name: p.name.clone(), workspace_id: ws.id.clone() };
            let ring = String::from_utf8_lossy(&s.panes.snapshot(&p.id)).into_owned();
            let mut sources = vec![("terminal", crate::search::scan_recent(&ring, &query, crate::search::MAX_HITS))];
            // histórico que já saiu do buffer (spill em disco), com teto por painel e por busca
            let cap = crate::search::DISK_PER_PANE.min(disk_left);
            if cap > 0 {
                let (excerpts, read) = crate::search::scan_disk(&s.data.join("panes").join(&p.id), &query, cap, crate::search::MAX_HITS);
                disk_left -= read.min(disk_left);
                if read >= cap {
                    found.disk_limited = true;
                }
                sources.push(("historico", excerpts));
            } else {
                found.disk_limited = true;
            }
            if let (Some("claude"), Some(sid), false) = (p.cli.as_deref(), p.session_id.as_deref(), crate::remote::is_remote(&p.cwd)) {
                let dir = s.config_dir_for("claude", p.account.as_deref());
                if let Some(path) = crate::cost::claude::find_session_file(&dir, sid) {
                    let text = String::from_utf8_lossy(&std::fs::read(&path)?).into_owned();
                    let md = crate::transcript::claude_markdown(&text, &p.name, true);
                    sources.push(("conversa", crate::search::scan(&md, &query, crate::search::MAX_HITS)));
                }
            }
            for (source, excerpts) in sources {
                if crate::search::extend(&mut found.hits, &owner, source, excerpts) {
                    found.truncated = true;
                    break 'panes;
                }
            }
        }
    }
    Ok(found)
}

/// Busca no histórico em disco de um só painel (botão "Histórico" da busca do terminal). Mais recentes primeiro.
#[tauri::command(async)]
pub fn search_pane_history(s: S<'_>, id: String, query: String) -> AppResult<crate::search::Found> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return fail("bad_pane", "painel inválido");
    }
    let row = s.db.get_pane(&id)?;
    let query = query.trim().to_string();
    let mut found = crate::search::Found::default();
    if query.is_empty() {
        return Ok(found);
    }
    let (excerpts, read) = crate::search::scan_disk(&s.data.join("panes").join(&id), &query, crate::search::DISK_PER_PANE, crate::search::MAX_HITS);
    found.disk_limited = read >= crate::search::DISK_PER_PANE;
    let owner = crate::search::Owner { pane_id: row.id.clone(), pane_name: row.name.clone(), workspace_id: row.workspace_id.clone() };
    found.truncated = crate::search::extend(&mut found.hits, &owner, "historico", excerpts);
    Ok(found)
}

/// Issue do GitHub para o brief da missão (P1.9). Só roda com o clique do usuário, na pasta do workspace (`gh`).
#[tauri::command(async)]
pub fn gh_issue(s: S<'_>, workspace_id: String, reference: String) -> AppResult<crate::github::Issue> {
    let ws = s.db.get_workspace(&workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return fail("remote_unsupported", "o gh só roda em workspace local");
    }
    let r = crate::github::parse_ref(&reference).ok_or_else(|| AppError::new("bad_issue", "use o número (123 ou #123) ou a URL da issue no GitHub"))?;
    crate::github::view(Path::new(&ws.path), &r)
}

// ---------- commit e PR (P2.2): alvo = worktree da missão, ou o repositório do workspace
/// Painéis que a tela mostra agora (o aviso do Windows só sai dos que estão fora dela).
#[tauri::command]
pub fn panes_visible(s: S<'_>, ids: Vec<String>) {
    s.rt().visible_panes = ids.into_iter().collect();
}

/// Arquivos que dá para commitar (staged, não staged e não rastreados).
/// Roda `f` no pool de threads bloqueantes do runtime. Comando `async` síncrono roda numa thread do próprio runtime e,
/// se demora (git status em repositório grande, disco de rede), prende as outras chamadas que dependem dela.
async fn bloqueante<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::new("join", e.to_string()))?
}

#[tauri::command]
pub async fn git_status(s: S<'_>, workspace_id: String, mission_id: Option<String>) -> AppResult<Vec<crate::gitflow::Entry>> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<Vec<crate::gitflow::Entry>> {
        let s = &*s;
        let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
        crate::gitflow::status_entries(&root)
    })
    .await
}

/// Diff de trabalho do workspace (ou da missão), capado: o compositor o cola como `@diff`.
#[tauri::command(async)]
pub fn diff_text(s: S<'_>, workspace_id: String, mission_id: Option<String>) -> AppResult<String> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    // missão com worktree: também o que a branch já tem contra a base
    let base = match mission_id.as_deref() {
        Some(id) => Some(crate::missions::live_worktree(&s.db.get_mission(id)?)?.base_branch),
        None => None,
    };
    crate::gitflow::working_diff(&root, base.as_deref())
}

/// Mensagem sugerida para os arquivos escolhidos (IA com chave no cofre; senão o modelo pelos nomes).
#[tauri::command]
pub async fn git_commit_suggest(s: S<'_>, workspace_id: String, mission_id: Option<String>, paths: Vec<String>) -> AppResult<crate::gitflow::CommitSuggestion> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    let diff = crate::gitflow::commit_diff(&root, &paths)?;
    crate::gitflow::suggest_commit(&s, &paths, &diff).await
}

/// Mensagem do modelo para abrir o commit já preenchida (só sem IA; a IA fica no clique).
#[tauri::command(async)]
pub fn git_commit_prefill(s: S<'_>, paths: Vec<String>) -> AppResult<Option<String>> {
    crate::gitflow::commit_prefill(&s, &paths)
}

/// Título e descrição do modelo para abrir o PR já preenchidos (só sem IA). `None` se a base é inválida.
#[tauri::command(async)]
pub fn git_pr_prefill(s: S<'_>, workspace_id: String, mission_id: Option<String>, base: String) -> AppResult<Option<crate::gitflow::PrSuggestion>> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    let base = base.trim();
    let branch = crate::git::current_branch(&root)?;
    if base.is_empty() || branch.is_empty() || base == branch {
        return Ok(None);
    }
    crate::gitflow::check_base(base)?;
    crate::gitflow::pr_prefill(&s, &root, base, &branch)
}

/// Commit só com os arquivos escolhidos; o resto segue como estava. Devolve o hash curto.
#[tauri::command(async)]
pub fn git_commit(s: S<'_>, workspace_id: String, mission_id: Option<String>, paths: Vec<String>, message: String) -> AppResult<String> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    crate::gitflow::commit(&root, &paths, &message)
}

/// Título e descrição sugeridos para o PR da branch contra `base`.
#[tauri::command]
pub async fn git_pr_suggest(s: S<'_>, workspace_id: String, mission_id: Option<String>, base: String) -> AppResult<crate::gitflow::PrSuggestion> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    let branch = crate::git::current_branch(&root)?;
    if base.trim().is_empty() || branch.is_empty() || base.trim() == branch {
        return fail("bad_base", "escolha uma branch base diferente da branch atual");
    }
    crate::gitflow::check_base(base.trim())?;
    crate::gitflow::suggest_pr(&s, &root, base.trim(), &branch).await
}

/// Branch base sugerida para o PR (ver `gitflow::default_base`).
#[tauri::command(async)]
pub fn git_pr_base(s: S<'_>, workspace_id: String, mission_id: Option<String>) -> AppResult<String> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    Ok(crate::gitflow::default_base(&root))
}

/// Envia a branch e abre o PR (`gh pr create`). Só roda depois do usuário confirmar o envio. Devolve a URL.
#[tauri::command(async)]
pub fn git_pr_open(s: S<'_>, workspace_id: String, mission_id: Option<String>, base: String, title: String, body: String) -> AppResult<String> {
    let root = crate::gitflow::target(&s, &workspace_id, mission_id.as_deref())?;
    let branch = crate::git::current_branch(&root)?;
    if base.trim().is_empty() || branch.is_empty() || base.trim() == branch {
        return fail("bad_base", "escolha uma branch base diferente da branch atual");
    }
    crate::gitflow::check_base(base.trim())?;
    crate::gitflow::open_pr(
        base.trim(),
        &branch,
        &title,
        &body,
        &|a: &[&str]| crate::github::run(&root, a),
        &|b: &str| crate::gitflow::push(&root, b),
    )
}

/// Grava a conversa no caminho que o usuário escolheu no diálogo de salvar. Só `.md`.
#[tauri::command(async)]
pub fn export_markdown(path: String, text: String) -> AppResult<()> {
    let p = Path::new(&path);
    if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
        return fail("bad_ext", "salve a conversa como arquivo .md");
    }
    std::fs::write(p, text)?;
    Ok(())
}

#[tauri::command]
pub fn prompts_list(s: S<'_>) -> AppResult<Vec<crate::prompts::PromptDef>> {
    crate::prompts::list(&s.data)
}

#[tauri::command]
pub fn prompts_save(s: S<'_>, prompts: Vec<crate::prompts::PromptDef>) -> AppResult<Vec<crate::prompts::PromptDef>> {
    crate::prompts::save(&s.data, prompts)
}

/// Cola o prompt já expandido no painel (sem Enter, salvo se `enter`).
#[tauri::command]
pub fn prompt_paste(s: S<'_>, id: String, text: String, enter: bool) -> AppResult<()> {
    s.write_input(&id, &crate::orchestrator::paste_bytes(&text, enter))
}

/// A pasta do workspace é um repositório git? (o assistente de missão liga o worktree por padrão nesse caso). Remoto: não.
#[tauri::command]
pub async fn git_is_repo(s: S<'_>, workspace_id: String) -> AppResult<bool> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<bool> {
        let s = &*s;
        let ws = s.db.get_workspace(&workspace_id)?;
        if crate::remote::is_remote(&ws.path) {
            return Ok(false);
        }
        Ok(crate::git::is_repo(Path::new(&ws.path)))
    })
    .await
}

/// Cria o repositório Git na pasta do workspace (botão "Iniciar Git" do rodapé). Já sendo repositório, não faz nada.
#[tauri::command(async)]
pub fn git_init(s: S<'_>, workspace_id: String) -> AppResult<()> {
    let ws = s.db.get_workspace(&workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return remote_unsupported("iniciar o Git");
    }
    let root = Path::new(&ws.path);
    if !crate::git::is_repo(root) {
        crate::git::git(root, &["init", "-q"])?;
    }
    Ok(())
}

/// Branches do workspace para o seletor do rodapé (`fetch`: busca o remoto antes). Rede: fora da thread da janela.
#[tauri::command]
pub async fn git_branches(s: S<'_>, workspace_id: String, fetch: bool) -> AppResult<crate::gitflow::Branches> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<crate::gitflow::Branches> {
        let s = &*s;
        crate::gitflow::branches(&crate::gitflow::target(&s, &workspace_id, None)?, fetch)
    })
    .await
}

/// Branch, upstream e ↑↓ do repositório do workspace num processo git só (o rodapé relê isto; o seletor usa `git_branches`).
#[tauri::command]
pub async fn git_sync_state(s: S<'_>, workspace_id: String) -> AppResult<crate::gitflow::Branches> {
    let s = s.inner().clone();
    bloqueante(move || crate::gitflow::sync_state(&crate::gitflow::target(&s, &workspace_id, None)?)).await
}

/// Troca a branch do workspace (`create`: cria a partir da atual). Devolve a branch atual.
#[tauri::command(async)]
pub fn git_switch(s: S<'_>, workspace_id: String, branch: String, create: bool) -> AppResult<String> {
    crate::gitflow::switch(&crate::gitflow::target(&s, &workspace_id, None)?, &branch, create)
}

/// Puxa a branch do workspace (só avanço rápido). Devolve o resumo.
#[tauri::command(async)]
pub fn git_pull(s: S<'_>, workspace_id: String) -> AppResult<String> {
    crate::gitflow::pull(&crate::gitflow::target(&s, &workspace_id, None)?)
}

/// Sincroniza a branch do workspace com o remoto (rebase + push; sem remoto, publica). Devolve o resumo.
#[tauri::command(async)]
pub fn git_sync(s: S<'_>, workspace_id: String) -> AppResult<String> {
    crate::gitflow::sync(&crate::gitflow::target(&s, &workspace_id, None)?)
}

/// Branch atual do workspace para a variável {{branch}}; vazio fora de repositório git ou em workspace remoto.
#[tauri::command(async)]
pub fn workspace_branch(s: S<'_>, workspace_id: String) -> AppResult<String> {
    let ws = s.db.get_workspace(&workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return Ok(String::new());
    }
    Ok(crate::git::branch_fast(Path::new(&ws.path)).unwrap_or_default())
}

/// Branch atual do workspace, ou do worktree da missão, lida do arquivo HEAD (sem processo git): o rodapé pergunta a
/// cada 2 s e ao ganhar foco, e uma mudança dispara a releitura completa. None = fora de git ou remoto; "" = destacado.
#[tauri::command(async)]
pub fn git_head(s: S<'_>, workspace_id: String, mission_id: Option<String>) -> AppResult<Option<String>> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let dir = match mission_id.map(|id| s.db.get_mission(&id)).transpose()?.and_then(|m| m.worktree_path) {
        Some(wt) => wt,
        None => ws.path,
    };
    if crate::remote::is_remote(&dir) {
        return Ok(None);
    }
    let p = Path::new(&dir);
    Ok(crate::git::head_branch(p).or_else(|| crate::git::is_repo(p).then(|| crate::git::current_branch(p).unwrap_or_default())))
}

#[tauri::command]
pub fn recipes_list(s: S<'_>) -> AppResult<Vec<RecipeDef>> {
    Ok(recipes::list(&s.data))
}

#[tauri::command]
pub fn recipe_save(s: S<'_>, recipe: RecipeDef) -> AppResult<()> {
    recipes::save(&s.data, &recipe)
}

#[tauri::command]
pub fn recipe_delete(s: S<'_>, id: String) -> AppResult<()> {
    recipes::remove(&s.data, &id)
}

/// The user's decision on a gate (the UI's only path: no MCP tool decides a gate).
#[tauri::command]
pub fn gate_answer(s: S<'_>, id: String, decision: String, note: Option<String>) -> AppResult<squads::GateReply> {
    let gate = s.db.get_gate(&id).ok();
    let reply = squads::answer_gate(&s, &id, &decision, note.as_deref())?;
    // memória: a decisão vira sugestão (RM-32); o gate já foi decidido, então a falha de leitura só perde a sugestão
    if let Some(g) = gate.and_then(|g| s.db.get_pane(&g.pane_id).ok().map(|p| (g, p))) {
        let (g, p) = g;
        // só decisão com nota vira sugestão: "aprovar" sozinho (ex.: gate de plano) não ensina nada e só enche a fila
        if let Some(n) = note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            let nota = format!(" — {n}");
            crate::memory::suggest(&s, &p.workspace_id, g.mission_id.as_deref(), "decisao", &format!("{}: {decision}{nota}", g.title));
        }
    }
    crate::planexec::on_gate(&s, &id, &decision, note.as_deref());
    Ok(reply)
}

/// Plano da missão (`plano.json`) e as tarefas, para a aba de tarefas.
#[tauri::command]
pub fn plan_get(s: S<'_>, mission_id: String) -> AppResult<Value> {
    crate::planexec::plan_get(&s, &mission_id)
}

/// "Retomar plano": tarefas bloqueadas ou sem executor voltam a pendente e o despacho segue.
#[tauri::command(async)]
pub fn plan_resume(s: S<'_>, mission_id: String) -> AppResult<usize> {
    crate::planexec::retomar(&s, &mission_id)
}

/// Undecided gates, to rebuild the gate modals after a restart.
#[tauri::command]
pub fn gates_open(s: S<'_>, workspace_id: Option<String>) -> AppResult<Vec<Gate>> {
    s.db.open_gates(workspace_id.as_deref())
}

#[tauri::command]
pub fn tasks_list(s: S<'_>, mission_id: String) -> AppResult<Vec<crate::db::TaskItem>> {
    s.db.tasks(&mission_id)
}

/// Entregas da missão, das mais recentes (a seção "Entregas" da missão).
#[tauri::command]
pub fn handoff_list(s: S<'_>, mission_id: String) -> AppResult<Vec<crate::db::Handoff>> {
    s.db.handoffs(Some(&mission_id), None, 50)
}

/// Unseen handoffs per mission of the workspace: [{missionId, count}].
#[tauri::command]
pub fn unseen_handoffs(s: S<'_>, workspace_id: String) -> AppResult<Vec<Value>> {
    Ok(s.db.unseen_handoffs(&workspace_id)?.into_iter().map(|(id, n)| json!({ "missionId": id, "count": n })).collect())
}

#[tauri::command]
pub fn mission_mark_seen(s: S<'_>, id: String) -> AppResult<()> {
    s.db.mark_handoffs_seen(&id)
}

#[tauri::command]
pub fn mission_list(s: S<'_>, workspace_id: String) -> AppResult<Vec<Mission>> {
    s.db.list_missions(&workspace_id)
}

/// Worktree do repositório do workspace (bloco A); `adopted` = já é missão aberta.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub adopted: bool,
}

/// Worktrees do repositório, menos o principal, marcando os que já são missão aberta. Fora de git ou remoto: vazio.
pub fn worktrees_of(s: &AppState, ws: &Workspace) -> AppResult<Vec<WorktreeInfo>> {
    let root = Path::new(&ws.path);
    if crate::remote::is_remote(&ws.path) || !crate::git::is_repo(root) {
        return Ok(vec![]);
    }
    let abertos: Vec<String> = s.db.list_missions(&ws.id)?.into_iter().filter(|m| m.status != "arquivada").filter_map(|m| m.worktree_path).collect();
    Ok(crate::worktrees::list(root)?
        .into_iter()
        .map(|w| WorktreeInfo { adopted: abertos.iter().any(|p| missions::same_dir(p, &w.path)), path: w.path, head: w.head, branch: w.branch })
        .collect())
}

#[tauri::command(async)]
pub fn worktrees_list(s: S<'_>, workspace_id: String) -> AppResult<Vec<WorktreeInfo>> {
    let ws = s.db.get_workspace(&workspace_id)?;
    worktrees_of(&s, &ws)
}

/// Adota um worktree do projeto como missão: nada é criado nem removido no git. A base vem de `projeto.json` (`git.base`)
/// ou de `gitflow::default_base`. Recusa caminho fora da lista e worktree que já é missão.
pub fn adopt_worktree(s: &AppState, ws: &Workspace, path: &str) -> AppResult<Mission> {
    let Some(w) = worktrees_of(s, ws)?.into_iter().find(|w| missions::same_dir(&w.path, path)) else {
        return fail("bad_worktree", "esse caminho não é um worktree deste repositório");
    };
    if w.adopted {
        return fail("already_adopted", "esse worktree já é uma missão aberta");
    }
    // pasta apagada por fora: o git ainda a lista (prunable), mas não há o que adotar
    if !Path::new(&w.path).is_dir() {
        return fail("no_worktree", "a pasta desse worktree não existe mais (rode git worktree prune)");
    }
    let root = Path::new(&ws.path);
    let base = crate::projeto::git_base(root).unwrap_or_else(|| crate::gitflow::default_base(root));
    let m = missions::new_adotada(ws, &w.path, &base)?;
    s.db.insert_mission(&m)?;
    s.emit("mission-updated", json!({"missionId": m.id}));
    Ok(m)
}

#[tauri::command(async)]
pub fn mission_adopt(s: S<'_>, workspace_id: String, path: String) -> AppResult<Mission> {
    let ws = s.db.get_workspace(&workspace_id)?;
    adopt_worktree(&s, &ws, &path)
}

/// Missão adotada segue o `git.base` do projeto: mudar a base do projeto corrige as missões já adotadas.
fn base_do_projeto(s: &AppState, m: &mut Mission) -> AppResult<()> {
    if !m.adopted {
        return Ok(());
    }
    let ws = s.db.get_workspace(&m.workspace_id)?;
    if let Some(b) = crate::projeto::git_base(Path::new(&ws.path)).filter(|b| m.base_branch.as_deref() != Some(b.as_str())) {
        s.db.set_mission_base(&m.id, &b)?;
        m.base_branch = Some(b);
        s.emit("mission-updated", json!({"missionId": m.id}));
    }
    Ok(())
}

/// Estado vivo do worktree da missão. Adotada cuja branch mudou por fora (`ambiente.sh`): grava a nova no banco.
pub fn worktree_state_of(s: &AppState, id: &str) -> AppResult<missions::WorktreeState> {
    let mut m = s.db.get_mission(id)?;
    base_do_projeto(s, &mut m)?;
    let wt = missions::live_worktree(&m)?;
    let st = missions::worktree_state(&wt.path, &wt.base_branch)?;
    if m.adopted && m.branch != st.branch {
        s.db.set_mission_branch(&m.id, st.branch.as_deref())?;
        s.emit("mission-updated", json!({"missionId": m.id}));
    }
    Ok(st)
}

#[tauri::command]
pub async fn mission_worktree_state(s: S<'_>, id: String) -> AppResult<missions::WorktreeState> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<missions::WorktreeState> {
        let s = &*s;
        worktree_state_of(&s, &id)
    })
    .await
}

/// The UI only changes name, pinned and the manual statuses.
#[tauri::command]
pub fn mission_update(s: S<'_>, mission: Mission) -> AppResult<Mission> {
    let mut cur = s.db.get_mission(&mission.id)?;
    if cur.status == "arquivada" {
        return fail("archived", "missão arquivada não pode ser alterada");
    }
    if !mission.name.trim().is_empty() {
        cur.name = mission.name.trim().to_string();
    }
    cur.pinned = mission.pinned;
    if ["ativa", "pronta", "concluida"].contains(&mission.status.as_str()) {
        cur.status = mission.status;
        cur.status_reason = None;
    }
    s.db.update_mission(&cur)?;
    s.emit("mission-updated", serde_json::json!({"missionId": cur.id}));
    Ok(cur)
}

#[tauri::command(async)]
pub fn mission_integrate(s: S<'_>, id: String) -> AppResult<Mission> {
    missions::integrate_mission(&s, &id)
}

// async: espera até 5 s os processos morrerem e roda git — fora da thread principal
/// `manter_pasta`: o usuário já viu o `worktree_locked` e quer arquivar mesmo assim (a pasta sai depois, no boot).
#[tauri::command(async)]
pub fn mission_archive(s: S<'_>, id: String, manter_pasta: Option<bool>) -> AppResult<Mission> {
    archive_mission_opts(&s, id, manter_pasta.unwrap_or(false))
}

/// Arquiva uma missão (fecha os painéis, tira o worktree). Também usado por "Comparar abordagens" (A1c).
pub fn archive_mission(s: &AppState, id: String) -> AppResult<Mission> {
    archive_mission_opts(s, id, false)
}

/// Pastas de worktree que ficaram para trás no arquivamento (`manter_pasta`, arquivos em uso): tenta tirar de novo.
/// O WIP já virou commit na primeira tentativa; aqui só sai a pasta. Erro fica no log e a próxima abertura tenta outra vez.
pub fn retry_archived_worktrees(s: &AppState) {
    let Ok(wss) = s.db.list_workspaces() else { return };
    for ws in wss.iter().filter(|w| !crate::remote::is_remote(&w.path)) {
        let Ok(ms) = s.db.list_missions(&ws.id) else { continue };
        for m in ms.iter().filter(|m| m.status == "arquivada" && !m.adopted) {
            let Some(p) = m.worktree_path.as_deref().filter(|p| Path::new(p).exists()) else { continue };
            match missions::remove_worktree(Path::new(&ws.path), Path::new(p)) {
                Ok(()) => tracing::info!("pasta da missão arquivada {} removida", m.name),
                Err(e) => tracing::warn!("pasta da missão arquivada {} segue em uso: {e}", m.name),
            }
        }
    }
}

fn archive_mission_opts(s: &AppState, id: String, manter_pasta: bool) -> AppResult<Mission> {
    let mut m = s.db.get_mission(&id)?;
    // o despacho do plano não abre executores enquanto a missão fecha (a guarda sai de escopo no fim)
    let _arquivando = crate::planexec::Arquivando::new(&id);
    crate::verificacao::cancelar(&id);
    // trabalho que não chegou à base, antes de o worktree sair (memória: risco)
    // adotada: a branch é do projeto (pode nem existir); o cálculo de pendentes não se aplica
    let pendentes = match (s.db.get_workspace(&m.workspace_id), missions::worktree_of(&m)) {
        (Ok(ws), Ok(wt)) if !m.adopted => missions::files_pending(Path::new(&ws.path), &wt),
        _ => 0,
    };
    s.cancel_mission(&id); // antes de fechar: a fila não pode abrir painéis novos durante o arquivamento
    let panes: Vec<String> = s.db.mission_panes(&id)?.into_iter().filter(|p| p.closed_at.is_none()).map(|p| p.id).collect();
    for p in &panes {
        let _ = s.close_pane(p);
    }
    let t0 = Instant::now();
    while panes.iter().any(|p| s.panes.is_alive(p)) && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100)); // Windows locks files of live processes
    }
    // adotada: o worktree é do projeto e fica como está (arquivar só fecha os painéis)
    if let (Some(p), false) = (&m.worktree_path, m.adopted) {
        if Path::new(p).exists() {
            let ws = s.db.get_workspace(&m.workspace_id)?;
            // worktree_locked vai para a UI e a missão NÃO é marcada como arquivada, a menos que o usuário tenha
            // escolhido arquivar mesmo assim: aí a pasta fica e `retry_archived_worktrees` tenta de novo no boot
            match missions::remove_worktree(Path::new(&ws.path), Path::new(p)) {
                Err(e) if manter_pasta && e.code() == "worktree_locked" => tracing::warn!("missão {} arquivada com a pasta em uso: {e}", m.name),
                r => r?,
            }
        }
    }
    crate::fases::fechar_missao(&s, &id);
    m.status = "arquivada".into();
    m.archived_at = Some(now_ms());
    s.db.update_mission(&m)?;
    if pendentes > 0 {
        crate::memory::suggest(&s, &m.workspace_id, Some(&m.id), "risco", &format!("Missão {} arquivada com {pendentes} arquivos não integrados", m.name));
    }
    s.emit("mission-archived", serde_json::json!({"missionId": m.id}));
    Ok(m)
}

/// Arquivos que a missão muda contra a base do worktree (revisão antes de integrar, P2.1).
#[tauri::command]
pub async fn mission_diff_files(s: S<'_>, id: String) -> AppResult<Vec<crate::review::ChangedFile>> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<Vec<crate::review::ChangedFile>> {
        let s = &*s;
        let mut m = s.db.get_mission(&id)?;
        base_do_projeto(&s, &mut m)?;
        let wt = missions::live_worktree(&m)?;
        crate::review::changed_files(&wt.path, &wt.base_branch)
    })
    .await
}

/// As duas versões de um arquivo da missão (base e worktree) para o Monaco Diff.
#[tauri::command(async)]
pub fn mission_file_versions(s: S<'_>, id: String, path: String) -> AppResult<crate::review::FileVersions> {
    let mut m = s.db.get_mission(&id)?;
    base_do_projeto(&s, &mut m)?;
    let wt = missions::live_worktree(&m)?;
    crate::review::file_versions(&wt.path, &wt.base_branch, &path)
}

/// Reverte um arquivo da missão para a base (o usuário confirmou no diálogo). Adotada: recusa (`adopted`).
pub fn mission_revert_file_inner(s: &AppState, id: &str, path: &str) -> AppResult<()> {
    let m = s.db.get_mission(id)?;
    // adotada: a branch é do projeto; o Regente não commita nela (reverta pelo fluxo do projeto, no painel)
    if m.adopted {
        return fail("adopted", "missão adotada: reverta pelo fluxo do projeto, no painel");
    }
    let wt = missions::live_worktree(&m)?;
    crate::review::revert_file(&wt.path, &wt.base_branch, path)
}

#[tauri::command(async)]
pub fn mission_revert_file(s: S<'_>, id: String, path: String) -> AppResult<()> {
    mission_revert_file_inner(&s, &id, &path)
}

#[tauri::command]
pub fn spawn_cancel(s: S<'_>, mission_id: String, index: usize) -> bool {
    s.cancel_slot(&mission_id, index)
}

#[tauri::command]
pub fn mission_totals(s: S<'_>, workspace_id: String) -> AppResult<Vec<Value>> {
    Ok(s.db.mission_totals(&workspace_id)?.into_iter().map(|(id, tokens, cache, cost)| json!({ "missionId": id, "tokens": tokens, "cacheTokens": cache, "costUsd": cost })).collect())
}

/// Missões recentes de todos os workspaces, do uso mais novo ao mais antigo (Home → "Continuar de onde parei").
#[tauri::command]
pub fn recent_missions(s: S<'_>, limit: Option<i64>) -> AppResult<Vec<crate::db::RecentMission>> {
    s.db.recent_missions(limit.unwrap_or(10).clamp(1, 50))
}

// ---------- providers / accounts / prices / settings
#[tauri::command]
pub fn providers_get(s: S<'_>) -> ProvidersConfig {
    lk(&s.providers).clone()
}

#[tauri::command]
pub fn providers_save(s: S<'_>, mut config: ProvidersConfig) -> AppResult<()> {
    // a lista de padrões vistos é do servidor: a tela pode não mandá-la, e não pode fazer um padrão apagado voltar
    let seen = lk(&s.providers).seen_defaults.clone();
    for k in seen {
        if !config.seen_defaults.contains(&k) {
            config.seen_defaults.push(k);
        }
    }
    save_json(&s.data.join("providers.json"), &config)?;
    *lk(&s.providers) = config;
    s.forget_cli_paths();
    Ok(())
}

#[tauri::command(async)]
pub fn providers_detect(s: S<'_>) -> Vec<Detected> {
    s.forget_cli_paths();
    let cfg = lk(&s.providers).clone();
    providers::detect(&cfg)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliVersion {
    pub current: Option<String>,
    pub latest: Option<String>,
    pub newer: bool,
    pub update_cmd: Option<String>,
    pub note: Option<String>,
}

/// Versão instalada (`<cli> --version`) comparada com a publicada no npm (`npm view <pacote> version`).
#[tauri::command(async)]
pub fn cli_version_check(s: S<'_>, id: String) -> AppResult<CliVersion> {
    let cli = lk(&s.providers).clis.iter().find(|c| c.id == id).cloned().ok_or_else(|| AppError::new("unknown_cli", "IA desconhecida"))?;
    let current = s.cli_path(&cli.command).and_then(|p| crate::cli_version::capture(&p, &["--version"], Duration::from_secs(15))).and_then(|o| crate::cli_version::display(&o));
    let latest = match (&cli.npm_package, s.cli_path("npm")) {
        (Some(pkg), Some(npm)) if crate::cli_version::valid_npm_package(pkg) => crate::cli_version::capture(&npm, &["view", pkg, "version"], Duration::from_secs(30)).and_then(|o| crate::cli_version::display(&o)),
        _ => None,
    };
    let newer = match (&latest, &current) {
        (Some(l), Some(c)) => crate::cli_version::is_newer(l, c),
        _ => false,
    };
    let note = if current.is_none() {
        Some("não encontrado no PATH")
    } else if cli.npm_package.is_none() {
        Some("sem fonte pública de versão para esta IA")
    } else if latest.is_none() {
        Some("o npm não respondeu")
    } else {
        None
    };
    Ok(CliVersion { current, latest, newer, update_cmd: cli.update_cmd.or(cli.install), note: note.map(String::from) })
}

/// Sessões anteriores do Claude Code e do Codex ligadas à pasta do workspace (vazio em workspace remoto).
#[tauri::command(async)]
pub fn sessions_list(s: S<'_>, workspace_id: String) -> AppResult<Vec<crate::sessions::SessionInfo>> {
    let ws = s.db.get_workspace(&workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return Ok(vec![]);
    }
    let (claude, codex) = crate::sessions::default_homes(&crate::util::home_dir());
    Ok(crate::sessions::for_workspace(&claude, &codex, &ws.path))
}

/// Abre um painel de agente retomando uma sessão anterior (`--resume <id>` ou o equivalente do Codex).
#[tauri::command]
pub fn session_resume(s: S<'_>, workspace_id: String, cli: String, session_id: String) -> AppResult<PaneRow> {
    resume_session(&s, workspace_id, cli, session_id)
}

/// The shared path of the desktop and the phone (which resumes only closed conversations of its own panes):
/// a local workspace, a valid session id, a CLI that resumes, and a pane in `pend`.
pub fn resume_session(s: &AppState, workspace_id: String, cli: String, session_id: String) -> AppResult<PaneRow> {
    let ws = s.db.get_workspace(&workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return fail("remote_workspace", "sessões anteriores só são listadas em pasta local");
    }
    if !crate::sessions::valid_session_id(&session_id) {
        return fail("bad_session", "id de sessão inválido");
    }
    let has_resume = lk(&s.providers).clis.iter().any(|c| c.id == cli && !c.session_resume.is_empty());
    if !has_resume {
        return fail("no_resume", format!("{cli} não retoma sessão"));
    }
    // a mesma conversa não abre duas vezes
    if s.db.session_is_open(&session_id)? {
        return fail("already_open", "esta conversa já está aberta num painel");
    }
    // conta, modelo e rota vêm da linha da sessão (a mais nova), para retomar com o mesmo que estava
    let from = s.db.latest_session_pane(&workspace_id, &session_id)?;
    s.spawn_pane(resume_spec(&workspace_id, &ws.path, cli, session_id, from.as_ref()))
}

/// Pura: o `PaneSpec` de uma retomada. `from` é a linha que rodou a sessão (dá conta, modelo e rota).
pub fn resume_spec(workspace_id: &str, cwd: &str, cli: String, session_id: String, from: Option<&PaneRow>) -> PaneSpec {
    PaneSpec {
        workspace_id: workspace_id.into(),
        kind: PaneKind::Agent,
        cli: Some(cli),
        cwd: cwd.into(),
        resume_session: Some(session_id),
        account: from.and_then(|r| r.account.clone()),
        model: from.and_then(|r| r.model.clone()),
        route: from.and_then(|r| r.route.clone()),
        ..Default::default()
    }
}

/// "Me sugere": reescreve o rascunho do briefing em seções (IA auxiliar ou do Decisor). Não grava nada.
#[tauri::command(async)]
pub async fn brief_suggest(s: S<'_>, text: String) -> AppResult<crate::briefing::BriefSuggestion> {
    crate::briefing::suggest(&s, &text).await
}

#[tauri::command]
pub async fn route_test(s: S<'_>, route_id: String) -> AppResult<String> {
    let route = lk(&s.providers).routes.iter().find(|r| r.id == route_id).cloned().ok_or_else(|| AppError::new("unknown_route", "rota desconhecida"))?;
    let secret = match &route.secret {
        Some(n) => Some(s.secret(n)?.ok_or_else(|| AppError::new("missing_secret", format!("configure a chave '{n}' no Cofre")))?),
        None => None,
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none()) // a chave nunca segue um redirect
        .build().map_err(|e| AppError::new("route_failed", e.to_string()))?;
    let mut req = client.get(format!("{}/v1/models", route.base_url.trim_end_matches('/'))).header("anthropic-version", "2023-06-01");
    if let Some(k) = &secret {
        req = req.header("x-api-key", k).bearer_auth(k);
    }
    let t0 = Instant::now();
    let resp = req.send().await.map_err(|e| AppError::new("route_failed", e.to_string()))?;
    let ms = t0.elapsed().as_millis();
    if resp.status().is_success() {
        Ok(format!("Conectado, responde em {ms} ms"))
    } else {
        fail("route_failed", format!("HTTP {} em {ms} ms", resp.status().as_u16()))
    }
}

/// `cli` joins a path: only CLIs that support extra accounts (`accountEnv`).
fn account_cli(s: &AppState, cli: &str) -> AppResult<()> {
    if lk(&s.providers).clis.iter().any(|c| c.id == cli && c.account_env.is_some()) {
        Ok(())
    } else {
        fail("bad_cli", "esta IA não aceita contas extras")
    }
}

#[tauri::command]
pub fn account_add(s: S<'_>, cli: String, name: String) -> AppResult<Settings> {
    account_cli(&s, &cli)?;
    let name = name.trim().to_lowercase();
    if !providers::valid_account(&name) {
        return fail("bad_account", "use letras, números, - ou _ (e não 'principal' nem nomes reservados do Windows)");
    }
    std::fs::create_dir_all(s.data.join("accounts").join(&cli).join(&name))?;
    let mut st = lk(&s.settings);
    let list = st.accounts.entry(cli).or_default();
    if !list.contains(&name) {
        list.push(name);
    }
    save_json(&s.data.join("settings.json"), &*st)?;
    Ok(st.clone())
}

#[tauri::command]
pub fn account_remove(s: S<'_>, cli: String, name: String) -> AppResult<Settings> {
    account_cli(&s, &cli)?;
    let mut st = lk(&s.settings);
    if let Some(list) = st.accounts.get_mut(&cli) {
        list.retain(|n| n != &name);
    }
    save_json(&s.data.join("settings.json"), &*st)?;
    Ok(st.clone())
}

#[tauri::command]
pub fn prices_get(s: S<'_>) -> Prices {
    lk(&s.prices).clone()
}

#[tauri::command]
pub fn prices_save(s: S<'_>, prices: Prices) -> AppResult<()> {
    save_json(&s.data.join("prices.json"), &prices)?;
    *lk(&s.prices) = prices;
    Ok(())
}

#[tauri::command]
pub fn settings_get(s: S<'_>) -> Settings {
    lk(&s.settings).clone()
}

#[tauri::command]
pub fn settings_set(s: S<'_>, settings: Settings) -> AppResult<()> {
    // segura o lock durante a escrita para não intercalar com account_add/account_remove
    let mut cur = lk(&s.settings);
    let antes = cur.subagent_panes;
    let mut next = settings;
    next.accounts = cur.accounts.clone(); // contas só mudam por account_add/account_remove
    next.perfis = cur.perfis.clone(); // perfis só mudam por perfis_save, perfil_restore e ajustes (o formulário não os reescreve)
    next.auto_ajuste = cur.auto_ajuste.clone(); // o piloto (modo, aprendido, recusas) só muda pelos comandos autoajuste_*
    // marca de migração do bloco B: a interface nunca a edita; payload sem a chave viraria false e remigraria no boot
    next.inline_v1 = true;
    // proxy só de conta que existe, com URL validada (uma conta removida perde o proxy)
    let mut proxies = std::collections::BTreeMap::new();
    for (key, raw) in std::mem::take(&mut next.account_proxy) {
        let known = key.split_once('/').is_some_and(|(cli, acc)| cur.accounts.get(cli).is_some_and(|v| v.iter().any(|a| a == acc)));
        if known {
            let url = crate::settings::clean_proxy(&raw)?;
            if !url.is_empty() {
                proxies.insert(key, url);
            }
        }
    }
    next.account_proxy = proxies;
    next.spawn_concurrency = next.spawn_concurrency.clamp(1, 16);
    next.favorites = crate::settings::clean_favorites(std::mem::take(&mut next.favorites));
    next.terminal_font_size = next.terminal_font_size.clamp(9, 24);
    next.scrollback = crate::settings::clamp_scrollback(next.scrollback);
    next.subagent_close_secs = next.subagent_close_secs.min(3600);
    next.execucoes_prefs.promover_apos_s = next.execucoes_prefs.promover_apos_s.clamp(1, 600);
    next.execucoes_prefs.fechar_ok_s = next.execucoes_prefs.fechar_ok_s.min(3600);
    next.execucoes_prefs.max_promovidos = next.execucoes_prefs.max_promovidos.clamp(1, 20);
    next.execucoes_prefs.desligado_clis.retain(|c| c.len() <= 40);
    next.execucoes_prefs.desligado_clis.truncate(16);
    next.quickstart_dismissed.retain(|id| id.len() <= 40);
    next.quickstart_dismissed.truncate(16);
    next.auto_hibernate.idle_minutes = next.auto_hibernate.idle_minutes.clamp(1, 1440);
    next.auto_hibernate.resident_floor = next.auto_hibernate.resident_floor.min(16);
    crate::settings::clean_shortcuts(&mut next.shortcuts);
    next.budget.daily_usd = crate::budget::valid_limit(next.budget.daily_usd);
    next.budget.mission_usd = crate::budget::valid_limit(next.budget.mission_usd);
    // ▼ trilha A
    // ▲ trilha A
    // ▼ trilha B
    next.memoria.clamp();
    // a busca por significado só liga/desliga por embed_set_ativo (ela exige a instalação)
    next.busca_semantica = cur.busca_semantica.clone();
    next.voz_local.clamp();
    // ▲ trilha B
    // ▼ trilha C
    // a confiança nos comandos do projeto só muda por projeto_trust (como as contas)
    next.projeto.confianca = cur.projeto.confianca.clone();
    next.projeto.clamp();
    next.ausencia.clamp();
    // ▲ trilha C
    save_json(&s.data.join("settings.json"), &next)?;
    let mudou = antes != next.subagent_panes;
    *cur = next;
    // o lock de ajustes sai antes: reconciliar fecha painéis, que leem as Configurações
    drop(cur);
    if mudou {
        s.reconciliar_subagentes();
    }
    Ok(())
}

/// Presets, edições, padrão e ajustes por workspace (Configurações → Perfil de trabalho).
#[tauri::command]
pub fn perfis_get(s: S<'_>) -> Value {
    let prefs = lk(&s.settings).perfis.clone();
    json!({"presets": crate::perfil::presets(), "padrao": prefs.padrao, "editados": prefs.editados, "patches": prefs.patches})
}

/// Grava as preferências de perfil. Um preset editado precisa ser um `Perfil` completo; senão, erro com a mensagem.
#[tauri::command]
pub fn perfis_save(s: S<'_>, perfis: crate::perfil::PerfisPrefs) -> AppResult<()> {
    if !crate::perfil::IDS.contains(&perfis.padrao.as_str()) {
        return fail("bad_perfil", "perfil padrão desconhecido");
    }
    let mut next = perfis;
    // editados guarda só a diferença contra o preset (merge-patch); o formulário manda o preset inteiro
    for (id, v) in next.editados.iter_mut() {
        let p = crate::perfil::validate_editado(id, v)?;
        *v = crate::perfil::to_patch(id, &p)?;
    }
    next.clamp();
    let mut cur = lk(&s.settings);
    // os patches por workspace não vêm do formulário: só os ajustes (perfil_patch_remove, desfazer e o T1) mudam
    next.patches = cur.perfis.patches.clone();
    cur.perfis = next;
    save_json(&s.data.join("settings.json"), &*cur)?;
    s.emit("perfis-changed", json!({}));
    Ok(())
}

/// Remove o ajuste de perfil de um workspace (vai para o feed, desfazível).
#[tauri::command]
pub fn perfil_patch_remove(s: S<'_>, workspace_id: String) -> AppResult<()> {
    crate::ajustes::remover_patch(&s, &workspace_id)
}

/// Regras de leitura negada (`permissions.deny`) das pastas pesadas, para todas as stacks (lista somente leitura).
#[tauri::command]
pub fn perfil_read_denies() -> Vec<String> {
    crate::statusline::read_denies(&["node", "rust", "python"].map(String::from))
}

/// Recibo do perfil de um painel: `NOME=valor` das variáveis e os argumentos aplicados na última abertura. Sem segredos.
#[tauri::command]
pub fn pane_perfil(s: S<'_>, id: String) -> Vec<String> {
    s.rt().pane_perfil.get(&id).cloned().unwrap_or_default()
}

/// Perfil efetivo do workspace (e da missão): `{perfil, origem, ajustado}`.
#[tauri::command]
pub fn perfil_efetivo(s: S<'_>, workspace_id: String, mission_id: Option<String>) -> AppResult<Value> {
    let (perfil, origem, ajustado) = crate::perfil::efetivo(&s, &workspace_id, mission_id.as_deref())?;
    Ok(json!({"perfil": perfil, "origem": origem, "ajustado": ajustado}))
}

/// Perfil que uma missão nova teria neste workspace com o perfil `profile` (None = o padrão): o assistente mostra e
/// edita os papéis dele antes de começar.
#[tauri::command]
pub fn perfil_preview(s: S<'_>, workspace_id: String, profile: Option<String>) -> AppResult<crate::perfil::Perfil> {
    let prefs = lk(&s.settings).perfis.clone();
    Ok(crate::perfil::resolve(&prefs, &workspace_id, profile.as_deref()).0)
}

/// Perfil da missão (None = segue o projeto/padrão).
#[tauri::command]
pub fn mission_set_profile(s: S<'_>, id: String, profile: Option<String>) -> AppResult<()> {
    if profile.as_deref().is_some_and(|p| !crate::perfil::IDS.contains(&p)) {
        return fail("bad_perfil", "perfil desconhecido");
    }
    s.db.set_mission_profile(&id, profile.as_deref())
}

/// Volta um preset ao padrão (apaga a edição do usuário).
#[tauri::command]
pub fn perfil_restore(s: S<'_>, id: String) -> AppResult<()> {
    if !crate::perfil::IDS.contains(&id.as_str()) {
        return fail("bad_perfil", "perfil desconhecido");
    }
    let mut cur = lk(&s.settings);
    cur.perfis.editados.remove(&id);
    save_json(&s.data.join("settings.json"), &*cur)?;
    s.emit("perfis-changed", json!({}));
    Ok(())
}

/// Onde os tokens foram desperdiçados (RM-21). Atualiza a partir do último offset antes de agregar.
#[tauri::command(async)]
pub fn waste_summary(s: S<'_>, since_ms: i64, workspace_id: Option<String>) -> AppResult<Vec<crate::db::WasteRow>> {
    crate::cost::waste::refresh(&s)?;
    s.db.waste_rows(since_ms, workspace_id.as_deref())
}

/// Índice de código do workspace (RM-24): contagens do último passe e se o perfil o liga. Sem índice em workspace sensível ou remoto.
#[tauri::command(async)]
pub fn code_index_stats(s: S<'_>, workspace_id: String) -> AppResult<Value> {
    let ws = s.db.get_workspace(&workspace_id)?;
    let sensivel = s.is_sensitive(&workspace_id);
    let ligado = crate::perfil::efetivo_de(&s, &workspace_id, None).0.indice_codigo;
    let bloqueado = sensivel || crate::remote::parse_ws_path(&ws.path).is_some();
    // só conta o que já está em cache: não constrói nem atualiza o índice a partir de uma consulta de estatística
    let (arquivos, simbolos, atualizado): (usize, usize, Option<i64>) = match (cfg!(feature = "codeindex"), ligado && !bloqueado) {
        #[cfg(feature = "codeindex")]
        (true, true) if !crate::codeindex::toca_sensivel(&s, Path::new(&ws.path)) => crate::codeindex::em_cache(Path::new(&ws.path), |idx| {
            let (a, n) = crate::codeindex::estatisticas(idx);
            (a, n, Some(idx.atualizado_ms))
        })
        .unwrap_or((0, 0, None)),
        _ => (0, 0, None),
    };
    Ok(json!({"ligado": ligado, "sensivel": sensivel, "arquivos": arquivos, "simbolos": simbolos, "atualizadoMs": atualizado}))
}

/// Liga ou desliga o índice de código neste workspace: grava só essa chave no patch do workspace.
#[tauri::command]
pub fn code_index_set(s: S<'_>, workspace_id: String, ligado: bool) -> AppResult<()> {
    s.db.get_workspace(&workspace_id)?;
    let antes = lk(&s.settings).perfis.patches.get(&workspace_id).cloned();
    let mut patch = antes.clone().unwrap_or_else(|| json!({}));
    let Some(obj) = patch.as_object_mut() else { return fail("bad_patch", "patch do workspace inválido") };
    obj.insert("indiceCodigo".into(), json!(ligado));
    let editados = lk(&s.settings).perfis.editados.clone();
    if !crate::perfil::patch_valido(&patch, &editados) {
        return fail("bad_patch", "patch do workspace inválido");
    }
    // grava só se o patch ainda é o lido (sem pisar numa mudança concorrente) e registra o ajuste, desfazível
    if !crate::perfil::set_patch_if(&s, &workspace_id, antes.as_ref(), Some(patch.clone()))? {
        return fail("stale", "o perfil mudou ao mesmo tempo; tente de novo");
    }
    crate::ajustes::registrar(&s, crate::ajustes::Novo {
        workspace_id: Some(workspace_id.clone()), mission_id: None, pane_id: None, kind: "perfil_patch", acao: "aplicado",
        motivo: format!("índice de código {} pelo usuário", if ligado { "ligado" } else { "desligado" }),
        antes, depois: Some(patch), estimativa: None, desfazivel: true,
    })?;
    s.emit("perfis-changed", json!({}));
    Ok(())
}

/// Painel de economia (RM-23): grupos, acerto de cache, contexto médio e economia estimada.
#[tauri::command(async)]
pub fn economy_report(s: S<'_>, since_ms: i64, workspace_id: Option<String>) -> AppResult<crate::economia::EconomyReport> {
    crate::economia::report(&s.db, since_ms, workspace_id.as_deref())
}

/// Guarda de qualidade do workspace (RM-23; também consumida pelo T1).
#[tauri::command(async)]
pub fn quality_guard(s: S<'_>, workspace_id: String) -> AppResult<crate::economia::QualityGuard> {
    crate::economia::quality(&s, &workspace_id)
}

/// Higiene das instruções (RM-22): só lê os arquivos do workspace e o MEMORY.md do projeto. Remoto: vazio.
#[tauri::command(async)]
pub fn hygiene_scan(s: S<'_>, workspace_id: String) -> AppResult<crate::hygiene::Report> {
    let ws = s.db.get_workspace(&workspace_id)?;
    if crate::remote::is_remote(&ws.path) {
        return Ok(crate::hygiene::Report::default());
    }
    // a memória do projeto fica no diretório da conta que o workspace está usando (a do painel mais novo)
    let conta = s.db.active_claude_account(&workspace_id)?;
    let memoria = crate::hygiene::memoria_do_projeto(&s.config_dir_for("claude", conta.as_deref()), Path::new(&ws.path));
    Ok(crate::hygiene::scan(Path::new(&ws.path), Some(&memoria)))
}

/// Feed "Ajustes automáticos" (até 200, mais recentes primeiro). Sem workspace = todos.
#[tauri::command]
pub fn ajustes_list(s: S<'_>, workspace_id: Option<String>) -> AppResult<Vec<crate::db::Ajuste>> {
    crate::ajustes::listar(&s.db, workspace_id.as_deref(), 200)
}

#[tauri::command]
pub fn ajustes_undo(s: S<'_>, id: String) -> AppResult<()> {
    crate::ajustes::desfazer(&s, &id)
}

/// Aplica uma proposta do piloto automático (ação "Aplicar" no feed).
#[tauri::command]
pub fn ajustes_aplicar(s: S<'_>, id: String) -> AppResult<()> {
    crate::autoajuste::aplicar_proposta(&s, &id)
}

/// "Ignorar" numa proposta do piloto: a linha vira `pulado` e a mudança não volta por 30 dias.
#[tauri::command]
pub fn ajustes_ignorar(s: S<'_>, id: String) -> AppResult<()> {
    crate::autoajuste::ignorar(&s, &id)
}

/// Zera o que o piloto aprendeu (K, fator e razão de resumo). O feed fica como está.
#[tauri::command]
pub fn autoajuste_zerar_aprendizado(s: S<'_>) -> AppResult<()> {
    crate::autoajuste::zerar_aprendizado(&s)
}

/// Modo do piloto e o que ele aprendeu (Configurações → Perfil e a faixa do feed).
#[tauri::command]
pub fn autoajuste_get(s: S<'_>) -> crate::autoajuste::AutoAjustePrefs {
    lk(&s.settings).auto_ajuste.clone()
}

/// Grava o modo do piloto: aplicar | propor | desligado. Outro valor é recusado.
#[tauri::command]
pub fn autoajuste_set_modo(s: S<'_>, modo: String) -> AppResult<()> {
    if !matches!(modo.as_str(), "aplicar" | "propor" | "desligado") {
        return fail("bad_modo", "modo do piloto desconhecido");
    }
    let mut cur = lk(&s.settings);
    // voltar a aplicar zera as recusas: o usuário decidiu de novo
    if modo == "aplicar" {
        cur.auto_ajuste.recusados.clear();
    }
    cur.auto_ajuste.modo = modo;
    save_json(&s.data.join("settings.json"), &*cur)?;
    drop(cur);
    s.emit("perfis-changed", json!({}));
    Ok(())
}

/// Roda o piloto agora (a tela de Uso chama ao abrir). Não gera nada em `desligado`.
#[tauri::command(async)]
pub fn autoajuste_rodar(s: S<'_>) -> AppResult<()> {
    crate::autoajuste::tick(&s)
}

/// Stack de fonte herdada do settings.json do Windows Terminal (botão em Configurações → Geral).
#[tauri::command(async)]
pub fn terminal_font_from_wt() -> AppResult<String> {
    match crate::settings::wt_face_from_disk() {
        Some(face) => Ok(crate::settings::font_stack(Some(&face))),
        None => fail("not_found", "não achei uma fonte configurada no Windows Terminal"),
    }
}

// ---------- vault (values never return to the UI)
#[tauri::command(async)]
pub fn vault_names(s: S<'_>) -> AppResult<Vec<String>> {
    s.vault()?.as_ref().expect("opened").names()
}

#[tauri::command(async)]
pub fn vault_set(s: S<'_>, name: String, value: String) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return fail("bad_name", "dê um nome à chave");
    }
    crate::vault::check_name(name)?;
    s.vault()?.as_ref().expect("opened").set(name, &value)
}

/// Liga ou desliga o modo sensível de um workspace (o mesmo padrão da memória do workspace).
/// Ao ligar, devolve os painéis abertos cuja rota usa segredo do Cofre: continuam rodando até reiniciar, e a UI avisa.
#[tauri::command]
pub fn workspace_sensitive_set(s: S<'_>, workspace_id: String, on: bool) -> AppResult<Vec<String>> {
    let mut cur = lk(&s.settings);
    let mut next = cur.clone();
    next.sensitive_workspaces.retain(|w| w != &workspace_id);
    if on {
        next.sensitive_workspaces.push(workspace_id.clone());
    }
    save_json(&s.data.join("settings.json"), &next)?;
    *cur = next;
    drop(cur);
    // sensível não guarda vetor de memória (M1c): apaga os que já existem
    if on {
        s.db.delete_vectors_of_workspace(&workspace_id)?;
    }
    if !on {
        return Ok(vec![]);
    }
    let routes = lk(&s.providers).routes.clone();
    let uses_secret = |route: &Option<String>| route.as_deref().is_some_and(|r| routes.iter().any(|x| x.id == r && x.secret.is_some()));
    Ok(s.db.open_panes(&workspace_id)?.into_iter().filter(|p| uses_secret(&p.route)).map(|p| p.id).collect())
}

#[tauri::command(async)]
pub fn vault_delete(s: S<'_>, name: String) -> AppResult<()> {
    s.vault()?.as_ref().expect("opened").delete(name.trim())
}

// ---------- usage
#[tauri::command]
pub fn usage_rows(s: S<'_>, since: Option<String>) -> AppResult<Vec<UsageRow>> {
    s.db.usage_rows(since.as_deref())
}

#[tauri::command]
pub fn usage_totals(s: S<'_>) -> AppResult<Value> {
    let (tokens, cost) = s.db.totals()?;
    Ok(json!({ "tokens": tokens, "costUsd": cost }))
}

// ---------- files (always inside an open workspace)
fn guard(s: &AppState, p: &str) -> AppResult<PathBuf> {
    let roots: Vec<PathBuf> = s.db.list_workspaces()?.into_iter().map(|w| PathBuf::from(w.path)).collect();
    let p = PathBuf::from(p);
    files::ensure_within(&roots, &p)?;
    Ok(p)
}

/// Caminho `ssh://` dentro de um workspace remoto aberto.
fn remote_guard(s: &AppState, p: &str) -> AppResult<(crate::remote::Remote, String)> {
    let roots: Vec<String> = s.db.list_workspaces()?.into_iter().map(|w| w.path).collect();
    crate::remote::within(&roots, p)
}

fn remote_unsupported(what: &str) -> AppResult<()> {
    fail("remote_unsupported", format!("{what} ainda não funciona em workspace remoto"))
}

#[tauri::command]
pub async fn fs_list(s: S<'_>, path: String, ignorados: Option<bool>) -> AppResult<Vec<FsEntry>> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<Vec<FsEntry>> {
        let s = &*s;
        if crate::remote::is_remote(&path) {
            let (r, root) = remote_guard(&s, &path)?;
            return crate::remote::remote_list(&r, &root);
        }
        files::list_dir_opts(&guard(&s, &path)?, ignorados.unwrap_or(false))
    })
    .await
}

/// Índice de arquivos do Ctrl+P: uma chamada por workspace (cache em files::index).
#[tauri::command]
pub async fn fs_index(s: S<'_>, root: String) -> AppResult<files::Index> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<files::Index> {
        let s = &*s;
        files::index(&guard(&s, &root)?)
    })
    .await
}

/// Marcas do Git da árvore de Arquivos (a árvore consulta a cada 3 s quando visível). Workspace remoto: vazio.
#[tauri::command]
pub async fn fs_git_marcas(s: S<'_>, root: String) -> AppResult<Vec<files::GitMarca>> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<Vec<files::GitMarca>> {
        let s = &*s;
        if crate::remote::is_remote(&root) {
            return Ok(vec![]);
        }
        files::git_marcas(&guard(&s, &root)?)
    })
    .await
}

/// Texto do arquivo no último commit, para a margem do Git no editor. Workspace remoto: None.
#[tauri::command(async)]
pub fn fs_git_base(s: S<'_>, path: String) -> AppResult<Option<String>> {
    if crate::remote::is_remote(&path) {
        return Ok(None);
    }
    Ok(files::git_base(&guard(&s, &path)?))
}

#[tauri::command]
pub async fn fs_read(s: S<'_>, path: String) -> AppResult<FileText> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<FileText> {
        let s = &*s;
        if crate::remote::is_remote(&path) {
            let (r, root) = remote_guard(&s, &path)?;
            return crate::remote::remote_read(&r, &root);
        }
        files::read_text(&guard(&s, &path)?)
    })
    .await
}

/// Um caminho citado no terminal, achado em disco (`files::resolver_link`): onde está, se é pasta e se fica dentro de um
/// workspace aberto (fora dele o editor abre só leitura). None: não existe em nenhum dos lugares procurados.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkResolvido {
    path: String,
    dir: bool,
    dentro: bool,
}

#[tauri::command]
pub async fn fs_resolver_link(s: S<'_>, texto: String, cwd: String, workspace_id: String) -> AppResult<Option<LinkResolvido>> {
    // leitura de disco e git fora das threads do runtime (`bloqueante`): uma chamada lenta não prende as outras
    let s = s.inner().clone();
    bloqueante(move || -> AppResult<Option<LinkResolvido>> {
        let s = &*s;
        let ws = s.db.get_workspace(&workspace_id)?;
        if crate::remote::is_remote(&ws.path) || crate::remote::is_remote(&cwd) {
            return Ok(None);
        }
        let Some(p) = files::resolver_link(&texto, Path::new(&cwd), Path::new(&ws.path), &crate::util::home_dir()) else { return Ok(None) };
        let roots: Vec<PathBuf> = s.db.list_workspaces()?.into_iter().map(|w| PathBuf::from(w.path)).collect();
        let dentro = files::ensure_within(&roots, &p).is_ok();
        Ok(Some(LinkResolvido { dir: p.is_dir(), dentro, path: p.to_string_lossy().into_owned() }))
    })
    .await
}

/// Texto de um arquivo fora dos workspaces, só para ler (o clique num link do terminal que aponta para fora).
#[tauri::command(async)]
pub fn fs_read_externo(path: String) -> AppResult<FileText> {
    files::read_text_externo(Path::new(&path))
}

#[tauri::command(async)]
pub fn fs_write(s: S<'_>, path: String, text: String, expected_mtime: Option<i64>) -> AppResult<i64> {
    if crate::remote::is_remote(&path) {
        // a checagem do mtime e a troca atômica são um único comando no servidor
        let (r, root) = remote_guard(&s, &path)?;
        return crate::remote::remote_write(&r, &root, &text, expected_mtime);
    }
    let p = guard(&s, &path)?;
    let out = files::write_text(&p, &text, expected_mtime);
    files::invalidate_within(&p);
    out
}

#[tauri::command(async)]
pub fn fs_create(s: S<'_>, path: String, is_dir: bool) -> AppResult<()> {
    if crate::remote::is_remote(&path) {
        remote_guard(&s, &path)?;
        return remote_unsupported("criar arquivos");
    }
    let p = guard(&s, &path)?;
    files::create(&p, is_dir)?;
    files::invalidate_within(&p);
    Ok(())
}

/// Apagar/renomear exige um caminho ESTRITAMENTE dentro de um workspace (nunca a própria raiz).
fn guard_strict(s: &AppState, p: &str) -> AppResult<PathBuf> {
    let roots: Vec<PathBuf> = s.db.list_workspaces()?.into_iter().map(|w| PathBuf::from(w.path)).collect();
    let p = PathBuf::from(p);
    files::ensure_strictly_within(&roots, &p)?;
    Ok(p)
}

#[tauri::command(async)]
pub fn fs_rename(s: S<'_>, from: String, to: String) -> AppResult<()> {
    if crate::remote::is_remote(&from) || crate::remote::is_remote(&to) {
        return remote_unsupported("renomear arquivos");
    }
    let (from, to) = (guard_strict(&s, &from)?, guard_strict(&s, &to)?);
    files::rename(&from, &to)?;
    files::invalidate_within(&from);
    files::invalidate_within(&to);
    Ok(())
}

/// A árvore de arquivos passou a mostrar `root`: mudanças lá dentro saem como `fs-changed` e a troca de branch como
/// `git-head` (ver `fswatch`). Contado por raiz; `fs_unwatch` desfaz.
#[tauri::command]
pub fn fs_watch(s: S<'_>, root: String) -> AppResult<()> {
    if !crate::remote::is_remote(&root) {
        guard(&s, &root)?;
        crate::fswatch::watch(&root);
    }
    Ok(())
}

#[tauri::command]
pub fn fs_unwatch(root: String) {
    crate::fswatch::unwatch(&root);
}

/// Copia `src` para dentro da pasta `dir` (Duplicar, soltar do Explorer) com nome livre; devolve o caminho novo.
/// O destino tem de estar num workspace aberto; a origem pode vir de fora (arquivo solto do Explorer, gesto do usuário).
#[tauri::command(async)]
pub fn fs_copy(s: S<'_>, src: String, dir: String) -> AppResult<String> {
    if crate::remote::is_remote(&src) || crate::remote::is_remote(&dir) {
        return remote_unsupported("copiar arquivos").map(|_| String::new());
    }
    let dir = guard(&s, &dir)?;
    if !dir.is_dir() {
        return fail("not_a_dir", "o destino não é uma pasta");
    }
    let src = PathBuf::from(src);
    if !src.is_absolute() {
        return fail("bad_path", "caminho de origem relativo");
    }
    let novo = files::copy_into(&src, &dir)?;
    files::invalidate_within(&dir);
    Ok(novo.to_string_lossy().into_owned())
}

#[tauri::command(async)]
pub fn fs_delete(s: S<'_>, path: String) -> AppResult<()> {
    if crate::remote::is_remote(&path) {
        return remote_unsupported("apagar arquivos");
    }
    let p = guard_strict(&s, &path)?;
    files::delete(&p)?;
    files::invalidate_within(&p);
    Ok(())
}

/// "ok" | "grande" (>32 MB) | "fora" (outside the workspaces) | "invalida" (not an image)
#[tauri::command(async)]
pub fn image_check(s: S<'_>, path: String) -> AppResult<String> {
    if crate::remote::is_remote(&path) {
        return fail("remote_unsupported", "imagens de workspace remoto ainda não funcionam");
    }
    let p = match guard(&s, &path) {
        Ok(p) => p,
        Err(e) if e.code() == "outside_workspace" => return Ok("fora".into()),
        Err(e) => return Err(e),
    };
    let meta = std::fs::metadata(&p)?;
    if meta.is_dir() {
        return Ok("invalida".into());
    }
    if meta.len() > files::MAX_IMAGE {
        return Ok("grande".into());
    }
    let mut head = [0u8; 256];
    let n = std::fs::File::open(&p)?.read(&mut head)?;
    Ok(if files::looks_like_image(&head[..n]) { "ok" } else { "invalida" }.into())
}

// ---------- diagnostics
#[tauri::command(async)]
pub fn diag_generate(s: S<'_>) -> AppResult<String> {
    Ok(crate::diag::make_bundle(&s.data, &s.data.join("diagnosticos"), env!("CARGO_PKG_VERSION"))?.to_string_lossy().to_string())
}

// ---------- catálogo e marketplace
#[tauri::command]
pub fn catalog_agents(s: S<'_>) -> Vec<crate::catalog::AgentDef> {
    crate::catalog::list_agents(&s.data, &crate::util::home_dir())
}

/// Skill do Regente: estado nas pastas de skills do Claude (sem escrever nada).
#[tauri::command(async)]
pub fn base_skill_status() -> crate::base_skill::BaseSkillStatus {
    crate::base_skill::status(&crate::util::home_dir())
}

/// Instala as skills de papel na pasta do Claude (a UI confirma antes e mostra o caminho).
#[tauri::command(async)]
pub fn base_skill_install(s: S<'_>) -> AppResult<crate::base_skill::InstallReport> {
    crate::base_skill::install(&s.data, &crate::util::home_dir())
}

/// Remove as skills instaladas pelo Regente (vão para a Lixeira).
#[tauri::command(async)]
pub fn base_skill_remove() -> AppResult<Vec<String>> {
    crate::base_skill::remove(&crate::util::home_dir())
}

#[tauri::command]
pub fn catalog_skills(s: S<'_>) -> Vec<crate::catalog::SkillDef> {
    crate::catalog::list_skills(&s.data, &crate::util::home_dir())
}

/// SKILL.md of a catalog skill. Only the directories the catalog lists are read.
#[tauri::command]
pub fn catalog_skill_text(s: S<'_>, dir: String) -> AppResult<String> {
    if !crate::catalog::list_skills(&s.data, &crate::util::home_dir()).iter().any(|k| k.dir == dir) {
        return fail("bad_skill", "skill desconhecida");
    }
    Ok(std::fs::read_to_string(Path::new(&dir).join("SKILL.md"))?)
}

#[tauri::command]
pub fn catalog_save_agent(s: S<'_>, agent: crate::catalog::AgentDef) -> AppResult<PathBuf> {
    crate::catalog::save_agent(&s.data, &agent)
}

#[tauri::command]
pub async fn marketplace_search(s: S<'_>, q: String, kind: Option<String>, force: Option<bool>) -> AppResult<Vec<crate::marketplace::Item>> {
    let items = crate::marketplace::refresh(&s.data, &crate::marketplace::client()?, force.unwrap_or(false)).await?;
    Ok(crate::marketplace::search(&items, &q, kind.as_deref()))
}

#[tauri::command]
pub async fn marketplace_install(s: S<'_>, item: crate::marketplace::Item, target: String) -> AppResult<PathBuf> {
    crate::marketplace::install(&item, &target, &s.data, &crate::util::home_dir(), &crate::marketplace::client()?).await
}

#[derive(Serialize)]
pub struct InstalledIn {
    claude: bool,
    codex: bool,
    antigravity: bool,
    regente: bool,
}

#[tauri::command]
pub fn marketplace_installed(s: S<'_>, item: crate::marketplace::Item) -> InstalledIn {
    let home = crate::util::home_dir();
    let at = |t: &str| crate::marketplace::installed(&item, t, &s.data, &home);
    InstalledIn { claude: at("claude"), codex: at("codex"), antigravity: at("antigravity"), regente: at("regente") }
}

#[tauri::command]
pub fn marketplace_uninstall(s: S<'_>, name: String, kind: String, target: String) -> AppResult<()> {
    crate::marketplace::uninstall(&name, &kind, &target, &s.data, &crate::util::home_dir())
}

/// Settings → Shot: the config and the captures folder.
#[tauri::command]
pub fn shot_get(s: S<'_>) -> serde_json::Value {
    serde_json::json!({"config": crate::shot::load(&s.data), "dir": crate::shot::shots_dir(), "ffmpeg": crate::shot::ffmpeg_path().is_some()})
}

/// Saves the Shot config. The hotkeys are registered first; a refused or taken key leaves the previous config and
/// hotkeys in place and returns the error.
#[tauri::command]
pub fn shot_save(app: tauri::AppHandle, s: S<'_>, config: crate::shot::ShotConfig) -> AppResult<()> {
    crate::shot::apply(&app, &s.data, &config)?;
    // the MCP checks read this in memory: the only way to change it is this command
    lk(&s.guarded).agent_capture = config.agent_capture;
    Ok(())
}

/// Captures the monitor under the cursor (the button and the Ctrl+Shift+3 path). Returns the file.
#[tauri::command(async)]
pub fn shot_full(app: tauri::AppHandle) -> AppResult<String> {
    Ok(crate::shot::capture_full(&app)?.to_string_lossy().to_string())
}

/// Freezes the monitor and opens the region overlay.
#[tauri::command(async)]
pub fn shot_region_begin(app: tauri::AppHandle) -> AppResult<()> {
    crate::shot::begin(&app, crate::shot::Mode::Region)
}

/// The overlay's pick, in image pixels. Saves the crop and closes the overlay.
#[tauri::command(async)]
pub fn shot_region_commit(app: tauri::AppHandle, x: u32, y: u32, w: u32, h: u32) -> AppResult<Option<String>> {
    Ok(crate::shot::region_commit(&app, x, y, w, h)?.map(|p| p.to_string_lossy().to_string()))
}

/// Esc in the overlay: drops the frozen frame and closes the overlay.
#[tauri::command(async)]
pub fn shot_region_cancel(app: tauri::AppHandle) {
    crate::shot::region_cancel(&app)
}

/// The frozen frame as PNG bytes (the overlay's background).
#[tauri::command]
pub fn shot_overlay_image(app: tauri::AppHandle) -> AppResult<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(crate::shot::overlay_png(&app)?))
}

/// The captures, newest first.
#[tauri::command]
pub fn shot_list() -> Vec<crate::shot::ShotInfo> {
    crate::shot::list(&crate::shot::shots_dir())
}

/// Editor autosave: the rendered PNG (base64) and the document JSON, written beside the capture.
#[tauri::command(async)]
pub fn shot_save_edit(path: String, png: String, layers: String) -> AppResult<()> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let bytes = B64.decode(png).map_err(|_| AppError::new("bad_png", "imagem inválida"))?;
    crate::shot::save_edit(&crate::shot::shots_dir(), Path::new(&path), &bytes, &layers)
}

/// The editor's start state for a capture.
#[tauri::command]
pub fn shot_edit_get(path: String) -> AppResult<crate::shot::EditState> {
    crate::shot::edit_get(&crate::shot::shots_dir(), Path::new(&path))
}

/// Moves a capture (or a frames folder) to the Recycle Bin.
#[tauri::command(async)]
pub fn shot_delete(path: String) -> AppResult<()> {
    crate::shot::delete(&crate::shot::shots_dir(), Path::new(&path))
}

/// The image to draw on (the original once edited), as raw PNG bytes: the editor loads it from memory, so its canvas stays readable.
#[tauri::command(async)]
pub fn shot_read(path: String) -> AppResult<Response> {
    let st = crate::shot::edit_get(&crate::shot::shots_dir(), Path::new(&path))?;
    Ok(Response::new(std::fs::read(&st.base)?))
}

/// Gravar região: the overlay picks the region, then the recording starts. Without ffmpeg it is refused.
#[tauri::command(async)]
pub fn shot_record_begin(app: tauri::AppHandle) -> AppResult<()> {
    crate::shot::begin(&app, crate::shot::Mode::Record)
}

/// Parar: ends the recording and returns the video. The app opens the trimmer on the `shot-recorded` event.
#[tauri::command(async)]
pub fn shot_record_stop(app: tauri::AppHandle) -> AppResult<String> {
    Ok(crate::shot::stop_recording(&app)?.to_string_lossy().to_string())
}

/// Trimmer: frames of `start..end` seconds at `fps` (1 or 2), without duplicates, plus the contact sheet.
#[tauri::command(async)]
pub fn shot_extract(path: String, start: f64, end: f64, fps: u32) -> AppResult<crate::shot::Extracted> {
    crate::shot::extract(&crate::shot::shots_dir(), Path::new(&path), start, end, fps)
}

/// Forces a Redline poll now (the window's Atualizar), and tells every window and the tray.
#[tauri::command]
pub async fn redline_now(app: tauri::AppHandle, s: S<'_>) -> AppResult<crate::redline::Payload> {
    Ok(crate::redline::refresh(&s, &app).await)
}

/// Opens (or focuses) the Redline window: the top-bar chip uses it. Async, so creating the window never runs on the
/// main thread that is handling the event.
#[tauri::command]
pub async fn redline_open(app: tauri::AppHandle) {
    crate::dictation::open_redline(&app)
}

/// The last readings, without reading anything.
#[tauri::command]
pub fn redline_current(s: S<'_>) -> crate::redline::Payload {
    crate::redline::current(&s)
}

/// The history of the windows over the last `days` (1 to 30), oldest first.
#[tauri::command]
pub fn redline_history(s: S<'_>, days: u32) -> AppResult<Vec<crate::redline::LimitPoint>> {
    let days = days.clamp(1, crate::redline::MAX_DAYS) as i64;
    s.db.limits_since(crate::util::now_ms() - days * 86_400_000)
}

/// The per-account settings and the accounts they apply to.
#[tauri::command]
pub fn redline_prefs(s: S<'_>) -> Value {
    let accounts: Vec<String> = crate::redline::known_accounts(&s).into_iter().map(|(p, a)| format!("{p}/{a}")).collect();
    json!({"prefs": crate::redline::load_prefs(&s.data), "accounts": accounts})
}

/// Saves the `ativa` and `no topo` flags. Only known accounts can be set.
#[tauri::command]
pub fn redline_set_prefs(app: tauri::AppHandle, s: S<'_>, prefs: crate::redline::Prefs) -> AppResult<()> {
    let known: Vec<String> = crate::redline::known_accounts(&s).into_iter().map(|(p, a)| format!("{p}/{a}")).collect();
    if let Some(k) = prefs.accounts.keys().find(|k| !known.contains(k)) {
        return fail("bad_account", format!("conta desconhecida: {k}"));
    }
    crate::redline::save_prefs(&s.data, &prefs)?;
    // the top bar and the tray follow the new flags at once
    let p = crate::redline::current(&s);
    s.emit("limits", json!(p));
    crate::dictation::set_limits(&app, &p);
    Ok(())
}
