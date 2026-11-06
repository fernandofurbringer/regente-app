use crate::error::{fail, AppResult};
use crate::util::{new_id, now_ms};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub struct Db(pub Mutex<Connection>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub path: String,
    pub name: String,
    pub last_opened_at: i64,
    pub pinned: bool,
}

/// Missão na Home ("Continuar de onde parei"): nome, workspace e o que está pendente nela.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecentMission {
    pub id: String,
    pub workspace_id: String,
    pub workspace_name: String,
    pub workspace_path: String,
    pub name: String,
    pub status: String,
    /// Painéis abertos (não fechados) da missão.
    pub live_panes: i64,
    /// Custo da missão, incluindo painéis já fechados.
    pub cost_usd: f64,
    /// Entregas ainda não vistas.
    pub unseen: i64,
    /// Gates sem decisão.
    pub open_gates: i64,
    /// Último uso, em unix ms.
    pub last_activity: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Mission {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub mode: String,   // livre | squad | agentico
    pub status: String, // ativa | pronta | bloqueada | concluida | arquivada
    pub status_reason: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub base_branch: Option<String>,
    pub pinned: bool,
    pub created_at: i64,
    pub archived_at: Option<i64>,
    pub squad_id: Option<String>,
    pub recipe_id: Option<String>,
    pub brief: Option<String>,
    pub style: Option<String>,
    /// Perfil de trabalho da missão (id de `perfis`); None = segue o projeto/padrão.
    pub profile: Option<String>,
    /// Estado de aprovação do plano (X1a): fonte de verdade; `aprovacao.json` é só espelho.
    pub plan_json: Option<String>,
    /// Verificação da missão (`VerifyCfg` em JSON: comando, toggles, limites); None = padrões.
    pub verify_json: Option<String>,
    /// Missão adotada (bloco A): o worktree é do projeto; o Regente nunca o cria nem o remove.
    #[serde(default)]
    pub adopted: bool,
}

/// Uma variante de "Comparar abordagens" (A1c, RM-42).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompareVariant {
    pub group_id: String,
    pub mission_id: String,
    pub label: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PaneRow {
    pub id: String,
    pub workspace_id: String,
    pub mission_id: Option<String>,
    pub kind: String, // agent | terminal | browser | image
    pub name: String,
    pub ord: i64,
    pub cli: Option<String>,
    pub shell: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub permission: String, // danger | auto | pend | plan
    pub effort: Option<String>,
    pub route: Option<String>,
    pub session_id: Option<String>,
    pub cwd: String,
    pub url_or_path: Option<String>,
    pub status: String,
    pub resident: bool,
    pub created_at: i64,
    pub closed_at: Option<i64>,
    pub tok_in: i64,
    pub tok_out: i64,
    pub tok_cache_r: i64,
    pub tok_cache_w: i64,
    pub cost_usd: f64,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub agent: Option<String>,
    pub task_type: Option<String>,
    pub mcp_profile: String, // full | minimal (`""` = not set: insert_pane writes "full")
    pub allowed_skills: Option<String>, // JSON array
    pub name_manual: bool,
    /// The system prompt as the spawn received it (before the memory brief and the skills were appended).
    pub append_system: Option<String>,
    pub disallowed_tools: Option<String>, // JSON array
    /// Grupo de painéis do workspace (`pane_groups`); None = sem grupo.
    pub group_id: Option<String>,
    /// Servidores MCP escolhidos no perfil `custom` (JSON array de nomes); None fora dele.
    pub mcp_servers: Option<String>,
    /// Modo de memória do painel: auto (None), standalone ou off.
    pub memory_mode: Option<String>,
    /// Subagente do Claude (`kind` subagent): o id do hook SubagentStart.
    pub agent_id: Option<String>,
    /// Perfil de trabalho que o painel abriu (id de `perfis`); fixo até o painel fechar.
    pub profile: Option<String>,
    /// Gasto das sessões anteriores do painel (já incluído em tok_*/cost_usd). A sessão atual é o resto.
    pub base_tok_in: i64,
    pub base_tok_out: i64,
    pub base_tok_cache_r: i64,
    pub base_tok_cache_w: i64,
    pub base_cost_usd: f64,
}

/// Um tipo de desperdício agregado (tela Uso). `exemplo`: a evidência mais recente (None em workspace sensível).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WasteRow {
    pub kind: String,
    pub count: i64,
    pub panes: i64,
    pub tokens_est: i64,
    pub last_at: i64,
    pub exemplo: Option<String>,
    /// Achados cujo painel não existe mais (ainda contam; a tela mostra "painel removido").
    pub removidos: i64,
}

/// Estado de leitura de um JSONL: até onde foi lido, tamanho na última passada e hash do começo (detecta rotação).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WasteOff {
    pub pos: u64,
    pub len: u64,
    pub head: u64,
}

/// Achado a gravar (o arquivo vai junto na transação).
pub struct WasteIns<'a> {
    pub pane: &'a str,
    pub workspace: &'a str,
    pub session: Option<&'a str>,
    pub kind: &'static str,
    pub tokens: u64,
    pub evidence: Option<&'a str>,
    pub at: i64,
}

/// Um painel para o painel de economia (custos e tokens da linha, contadores de `pane_metrics`).
#[derive(Debug, Clone, PartialEq)]
pub struct EcoPane {
    pub profile: Option<String>,
    pub task_type: Option<String>,
    pub model: Option<String>,
    pub mission_id: Option<String>,
    pub cost_usd: f64,
    pub tok_in: i64,
    pub tok_out: i64,
    pub cache_r: i64,
    pub cache_w: i64,
    pub turns: i64,
    pub compactions: i64,
    pub fresh_starts: i64,
    pub escalations: i64,
    pub first_turn_tokens: Option<i64>,
}

/// Métricas por painel (`pane_metrics`): tabela lateral, fora de `PaneRow`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PaneMetrics {
    pub turns: i64,
    /// Entrada (input + cache) da primeira resposta do Claude: o prefixo fixo do painel.
    pub first_turn_tokens: Option<i64>,
    pub ctx_peak: i64,
    pub compactions: i64,
    pub fresh_starts: i64,
    pub escalations: i64,
    pub updated_at: i64,
}

/// Campos de `pane_metrics` aceitos por `metric_bump`/`metric_set`: o nome vai para o SQL, então é lista fechada.
const METRIC_FIELDS: [&str; 6] = ["turns", "first_turn_tokens", "ctx_peak", "compactions", "fresh_starts", "escalations"];

fn metric_field(field: &str) -> AppResult<&'static str> {
    METRIC_FIELDS.iter().copied().find(|f| *f == field).map_or_else(|| fail("bad_metric", format!("métrica desconhecida: {field}")), Ok)
}

/// Linha do feed "Ajustes automáticos": o que o Regente mudou sozinho, por quê, e o efeito (estimado e medido).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ajuste {
    pub id: String,
    pub created_at: i64,
    pub workspace_id: Option<String>,
    pub mission_id: Option<String>,
    pub pane_id: Option<String>,
    /// perfil_patch | compactacao | recomeco | escalonamento | indice_codigo | testes_relacionados | limites_saida
    pub kind: String,
    /// aplicado | proposto | pulado
    pub acao: String,
    pub motivo: String,
    pub antes_json: Option<String>,
    pub depois_json: Option<String>,
    pub estimativa_json: Option<String>,
    pub efeito_json: Option<String>,
    pub desfazivel: bool,
    pub desfeito_at: Option<i64>,
}

fn ajuste_from(r: &Row) -> rusqlite::Result<Ajuste> {
    Ok(Ajuste {
        id: r.get(0)?, created_at: r.get(1)?, workspace_id: r.get(2)?, mission_id: r.get(3)?, pane_id: r.get(4)?,
        kind: r.get(5)?, acao: r.get(6)?, motivo: r.get(7)?, antes_json: r.get(8)?, depois_json: r.get(9)?,
        estimativa_json: r.get(10)?, efeito_json: r.get(11)?, desfazivel: r.get(12)?, desfeito_at: r.get(13)?,
    })
}

/// Grupo nomeado de painéis do workspace: recolhível na grade; o painel guarda o id do grupo.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaneGroup {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub collapsed: bool,
    pub ord: i64,
}

/// Nome de grupo: aparado, 1 a 40 caracteres.
pub fn clean_group_name(name: &str) -> AppResult<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 40 {
        return fail("bad_name", "o nome do grupo tem de 1 a 40 caracteres");
    }
    Ok(n.to_string())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub day: String,
    pub cli: String,
    pub model: String,
    pub effort: String,
    pub tok_in: i64,
    pub tok_out: i64,
    pub cost_usd: f64,
    pub terminals_opened: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub id: String,
    pub mission_id: Option<String>,
    pub from_pane: String,
    pub to_pane: Option<String>,
    pub summary: String,
    pub data_json: Option<String>,
    pub files_json: Option<String>,
    pub created_at: i64,
    pub seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskItem {
    pub id: String,
    pub mission_id: String,
    pub title: String,
    pub status: String,
    pub owner_pane: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEntry {
    pub id: i64,
    pub ts: i64,
    /// chat | code (the global dictation mode when it ran)
    pub mode: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: i64, // assigned by insert_memory
    pub workspace_id: String,
    pub mission_id: Option<String>,
    pub kind: String,
    pub text: String,
    pub author_pane: Option<String>,
    pub created_at: i64,
    /// "ativa" (padrão, vai ao brief), "sugerida" (espera revisão) ou "arquivada" (expirou). Vazio = "ativa" na gravação.
    pub status: String,
    /// Fixada: entra no brief antes do resto e nunca expira.
    pub pinned: bool,
    /// Quantas vezes entrou num brief ou foi aceita numa fusão.
    pub uses: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    pub id: String,
    pub mission_id: Option<String>,
    pub pane_id: String,
    pub title: String,
    pub summary: String,
    pub options_json: Option<String>,
    pub decision: Option<String>,
    pub note: Option<String>,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    pub mission_id: Option<String>,
    pub pane_id: Option<String>,
    pub request: String,
    pub task_type: Option<String>,
    pub cli: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub agent: Option<String>,
    pub decided_by: String,
    pub cost_usd: f64,
    pub receipt: String,
    pub created_at: i64,
}

/// A paired phone. Its token is never stored, only the SHA-256 of it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MobileDevice {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    /// Paired by code: waits for the desktop's approval.
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub expr: String,
    pub prompt: String,
    pub target: String,
    pub spawn_json: Option<String>,
    pub enabled: bool,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub last_error: Option<String>,
    /// The pane the last spawn run opened: the next run writes into it while it is alive.
    pub last_pane: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HelperSession {
    pub task_type: String,
    pub mission_id: String,
    pub pane_id: String,
    pub session_id: Option<String>,
    pub cli: Option<String>,
    pub closed_at: i64,
}

const SCHEMA_V1: &str = r#"
CREATE TABLE workspaces(id TEXT PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
  last_opened_at INTEGER NOT NULL, pinned INTEGER NOT NULL DEFAULT 0);
CREATE TABLE missions(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, name TEXT NOT NULL,
  mode TEXT NOT NULL, status TEXT NOT NULL, status_reason TEXT, worktree_path TEXT, branch TEXT,
  base_branch TEXT, pinned INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, archived_at INTEGER);
CREATE TABLE panes(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, mission_id TEXT, kind TEXT NOT NULL,
  name TEXT NOT NULL, ord INTEGER NOT NULL, cli TEXT, shell TEXT, model TEXT, account TEXT,
  permission TEXT NOT NULL, effort TEXT, route TEXT, session_id TEXT, cwd TEXT NOT NULL, url_or_path TEXT,
  status TEXT NOT NULL, resident INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, closed_at INTEGER,
  tok_in INTEGER NOT NULL DEFAULT 0, tok_out INTEGER NOT NULL DEFAULT 0, tok_cache_r INTEGER NOT NULL DEFAULT 0,
  tok_cache_w INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL DEFAULT 0,
  frozen_frame TEXT, restorable INTEGER NOT NULL DEFAULT 0);
CREATE INDEX panes_ws ON panes(workspace_id);
CREATE TABLE usage_daily(day TEXT NOT NULL, cli TEXT NOT NULL, model TEXT NOT NULL, effort TEXT NOT NULL,
  tok_in INTEGER NOT NULL DEFAULT 0, tok_out INTEGER NOT NULL DEFAULT 0, cost_usd REAL NOT NULL DEFAULT 0,
  terminals_opened INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(day, cli, model, effort));
"#;

const SCHEMA_V2: &str = r#"
ALTER TABLE panes ADD COLUMN parent_id TEXT;
ALTER TABLE panes ADD COLUMN role TEXT;
ALTER TABLE panes ADD COLUMN agent TEXT;
ALTER TABLE panes ADD COLUMN task_type TEXT;
ALTER TABLE panes ADD COLUMN mcp_profile TEXT NOT NULL DEFAULT 'full';
ALTER TABLE panes ADD COLUMN allowed_skills TEXT;
ALTER TABLE panes ADD COLUMN name_manual INTEGER NOT NULL DEFAULT 0;
ALTER TABLE missions ADD COLUMN squad_id TEXT;
ALTER TABLE missions ADD COLUMN recipe_id TEXT;
ALTER TABLE missions ADD COLUMN brief TEXT;
ALTER TABLE missions ADD COLUMN style TEXT;
CREATE TABLE handoffs(id TEXT PRIMARY KEY, mission_id TEXT, from_pane TEXT NOT NULL, to_pane TEXT,
  summary TEXT NOT NULL, data_json TEXT, files_json TEXT, created_at INTEGER NOT NULL, seen INTEGER NOT NULL DEFAULT 0);
CREATE TABLE tasks(id TEXT PRIMARY KEY, mission_id TEXT NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL,
  owner_pane TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE memories(id INTEGER PRIMARY KEY AUTOINCREMENT, workspace_id TEXT NOT NULL, mission_id TEXT,
  kind TEXT NOT NULL, text TEXT NOT NULL, author_pane TEXT, created_at INTEGER NOT NULL);
CREATE VIRTUAL TABLE memories_fts USING fts5(text, content='memories', content_rowid='id');
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN INSERT INTO memories_fts(rowid, text) VALUES (new.id, new.text); END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN INSERT INTO memories_fts(memories_fts, rowid, text) VALUES ('delete', old.id, old.text); END;
CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO memories_fts(rowid, text) VALUES (new.id, new.text); END;
CREATE TABLE gates(id TEXT PRIMARY KEY, mission_id TEXT, pane_id TEXT NOT NULL, title TEXT NOT NULL, summary TEXT NOT NULL,
  options_json TEXT, decision TEXT, note TEXT, created_at INTEGER NOT NULL, decided_at INTEGER);
CREATE TABLE decisions(id TEXT PRIMARY KEY, mission_id TEXT, pane_id TEXT, request TEXT NOT NULL, task_type TEXT,
  cli TEXT, model TEXT, effort TEXT, agent TEXT, decided_by TEXT NOT NULL, cost_usd REAL NOT NULL DEFAULT 0,
  receipt TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE cron_jobs(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, name TEXT NOT NULL, expr TEXT NOT NULL,
  prompt TEXT NOT NULL, target TEXT NOT NULL, spawn_json TEXT, enabled INTEGER NOT NULL DEFAULT 1,
  last_run INTEGER, next_run INTEGER, last_error TEXT);
CREATE TABLE helper_sessions(task_type TEXT NOT NULL, mission_id TEXT NOT NULL, pane_id TEXT NOT NULL,
  session_id TEXT, cli TEXT, closed_at INTEGER NOT NULL, PRIMARY KEY(task_type, mission_id));
"#;

/// v3: what a respawn needs to rebuild the same CLI args (the system prompt and the denied tools, as received),
/// and the last pane a spawn job opened.
const SCHEMA_V3: &str = r#"
ALTER TABLE panes ADD COLUMN append_system TEXT;
ALTER TABLE panes ADD COLUMN disallowed_tools TEXT;
ALTER TABLE cron_jobs ADD COLUMN last_pane TEXT;
"#;

/// Global dictations (ditado global): the text pasted into other apps, for the history in Settings → Voz.
const SCHEMA_V4: &str = r#"
CREATE TABLE voice_history(id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, mode TEXT NOT NULL, text TEXT NOT NULL);
"#;

/// Attempts of paid media calls over MCP (the spending caps count these, failed ones included).
const SCHEMA_V5: &str = r#"
CREATE TABLE media_usage(id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, mission_id TEXT, created_at INTEGER NOT NULL);
CREATE INDEX media_usage_kind ON media_usage(kind, created_at);
"#;

const SCHEMA_V6: &str = r#"
CREATE TABLE limits_history(ts INTEGER NOT NULL, provider TEXT NOT NULL, account TEXT NOT NULL, window TEXT NOT NULL, used_percent REAL NOT NULL, resets_at INTEGER);
CREATE INDEX limits_history_key ON limits_history(provider, account, window, ts);
"#;

const SCHEMA_V7: &str = r#"
CREATE TABLE mobile_devices(id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL, created_at INTEGER NOT NULL, last_seen INTEGER, revoked INTEGER NOT NULL DEFAULT 0);
"#;

const SCHEMA_V8: &str = r#"
ALTER TABLE mobile_devices ADD COLUMN pending INTEGER NOT NULL DEFAULT 0;
"#;

/// Índices das consultas por missão (totais, orçamento, entregas e gates).
const SCHEMA_V9: &str = r#"
CREATE INDEX IF NOT EXISTS idx_panes_mission ON panes(mission_id);
CREATE INDEX IF NOT EXISTS idx_handoffs_mission ON handoffs(mission_id);
CREATE INDEX IF NOT EXISTS idx_gates_mission ON gates(mission_id);
"#;

/// Grupos de painéis por workspace (nome e recolhido); `panes.group_id` aponta para cá.
const SCHEMA_V10: &str = r#"
CREATE TABLE pane_groups(id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, name TEXT NOT NULL, collapsed INTEGER NOT NULL DEFAULT 0, ord INTEGER NOT NULL DEFAULT 0);
CREATE INDEX pane_groups_ws ON pane_groups(workspace_id);
ALTER TABLE panes ADD COLUMN group_id TEXT;
"#;

/// Perfil de MCP `custom`: nomes dos servidores mantidos (além do Regente), para a retomada reconfigurar igual.
const SCHEMA_V11: &str = r#"
ALTER TABLE panes ADD COLUMN mcp_servers TEXT;
"#;

/// Modo de memória do painel.
const SCHEMA_V12: &str = r#"
ALTER TABLE panes ADD COLUMN memory_mode TEXT;
"#;

/// Uma linha de `mission_phase_events` (fase da missão com intervalo e tokens/custo medidos).
#[derive(Debug, Clone, PartialEq)]
pub struct FaseRow {
    pub id: i64,
    pub mission_id: String,
    pub pane_id: Option<String>,
    pub task_id: Option<String>,
    pub phase: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub tok_in: i64,
    pub tok_out: i64,
    pub cost_usd: f64,
}

/// Métricas por fase da missão (planejamento, execução, verificação, correções): intervalo e tokens/custo do painel
/// no intervalo. `ended_at` nulo = fase em curso.
const SCHEMA_V20: &str = r#"
CREATE TABLE IF NOT EXISTS mission_phase_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mission_id TEXT NOT NULL,
    pane_id TEXT,
    task_id TEXT,
    phase TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    tok_in INTEGER NOT NULL DEFAULT 0,
    tok_out INTEGER NOT NULL DEFAULT 0,
    cost_usd REAL NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS mpe_mission ON mission_phase_events(mission_id);
"#;

/// Gasto acumulado das sessões anteriores do painel: uma nova sessão (resume falhou) não apaga o que já foi gasto.
const SCHEMA_V14: &str = r#"
ALTER TABLE panes ADD COLUMN base_tok_in INTEGER NOT NULL DEFAULT 0;
ALTER TABLE panes ADD COLUMN base_tok_out INTEGER NOT NULL DEFAULT 0;
ALTER TABLE panes ADD COLUMN base_tok_cache_r INTEGER NOT NULL DEFAULT 0;
ALTER TABLE panes ADD COLUMN base_tok_cache_w INTEGER NOT NULL DEFAULT 0;
ALTER TABLE panes ADD COLUMN base_cost_usd REAL NOT NULL DEFAULT 0;
"#;

/// `agent_id` do subagente do Claude (painel `subagent`): liga o painel aos hooks SubagentStart/Stop.
const SCHEMA_V15: &str = r#"
ALTER TABLE panes ADD COLUMN agent_id TEXT;
"#;

/// Perfil de trabalho (painel e missão), métricas por painel e feed "Ajustes automáticos".
/// Desperdício de tokens (RM-21): achados por painel e offset de leitura por arquivo JSONL.
/// Estrutural e idempotente: roda em todo `migrate()`, então não depende de `user_version`.
const SCHEMA_V17: &str = "
CREATE TABLE IF NOT EXISTS waste_findings(id INTEGER PRIMARY KEY AUTOINCREMENT, pane_id TEXT NOT NULL, workspace_id TEXT, session_id TEXT, file TEXT, kind TEXT NOT NULL, tokens_est INTEGER NOT NULL, evidence_json TEXT, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS wf_kind ON waste_findings(kind, created_at);
CREATE INDEX IF NOT EXISTS wf_file ON waste_findings(file);
CREATE TABLE IF NOT EXISTS waste_offsets(file TEXT PRIMARY KEY, offset INTEGER NOT NULL, file_len INTEGER NOT NULL DEFAULT 0, head_hash INTEGER NOT NULL DEFAULT 0);
";

const SCHEMA_V16: &str = r#"
ALTER TABLE panes ADD COLUMN profile TEXT;
ALTER TABLE missions ADD COLUMN profile TEXT;
CREATE TABLE IF NOT EXISTS pane_metrics (
    pane_id TEXT PRIMARY KEY,
    turns INTEGER NOT NULL DEFAULT 0,
    first_turn_tokens INTEGER,
    ctx_peak INTEGER NOT NULL DEFAULT 0,
    compactions INTEGER NOT NULL DEFAULT 0,
    fresh_starts INTEGER NOT NULL DEFAULT 0,
    escalations INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ajustes (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    workspace_id TEXT,
    mission_id TEXT,
    pane_id TEXT,
    kind TEXT NOT NULL,
    acao TEXT NOT NULL,
    motivo TEXT NOT NULL,
    antes_json TEXT,
    depois_json TEXT,
    estimativa_json TEXT,
    efeito_json TEXT,
    desfazivel INTEGER NOT NULL DEFAULT 0,
    desfeito_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_ajustes_ws ON ajustes(workspace_id, created_at);
"#;

/// Memória avançada (M1a): estado (ativa, sugerida, arquivada), fixada, usos e último uso. As colunas vêm por
/// `MEMORY_COLS_V19` (checagem por coluna), para o banco que já tiver alguma delas não quebrar.
const SCHEMA_V19: &str = r#"
CREATE INDEX IF NOT EXISTS idx_memories_ws_status ON memories(workspace_id, status);
DROP TRIGGER IF EXISTS memories_au;
CREATE TRIGGER memories_au AFTER UPDATE OF text ON memories BEGIN
  INSERT INTO memories_fts(memories_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO memories_fts(rowid, text) VALUES (new.id, new.text); END;
"#;

/// Busca semântica (M1c, RM-38): um vetor f32 (little-endian) por memória. Estrutural e idempotente: roda em todo `migrate()`.
/// O gatilho apaga o vetor junto com a memória (qualquer `DELETE`, inclusive o de workspace).
const SCHEMA_V21: &str = r#"
CREATE TABLE IF NOT EXISTS memory_vectors(memory_id INTEGER PRIMARY KEY, model TEXT NOT NULL, dim INTEGER NOT NULL, vec BLOB NOT NULL);
CREATE TRIGGER IF NOT EXISTS memories_ad_vetor AFTER DELETE ON memories BEGIN DELETE FROM memory_vectors WHERE memory_id = old.id; END;
"#;

/// Colunas da v21 (checagem por coluna, como a v19): o motivo do arquivamento por dedup.
const MEMORY_COLS_V21: &[(&str, &str)] = &[("status_motivo", "TEXT")];

/// Comparar abordagens (A1c, RM-42): variantes de um mesmo pedido, uma missão cada. Estrutural e idempotente.
const SCHEMA_V23: &str = r#"
CREATE TABLE IF NOT EXISTS compare_variants(group_id TEXT NOT NULL, mission_id TEXT PRIMARY KEY, label TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS idx_compare_group ON compare_variants(group_id);
"#;

const MEMORY_COLS_V19: &[(&str, &str)] = &[
    ("status", "TEXT NOT NULL DEFAULT 'ativa'"),
    ("pinned", "INTEGER NOT NULL DEFAULT 0"),
    ("uses", "INTEGER NOT NULL DEFAULT 0"),
    ("last_used_at", "INTEGER"),
];

/// Entregas por painel de origem: a atenção das missões consulta `has_handoff_from` a cada tick.
const SCHEMA_V13: &str = r#"
CREATE INDEX IF NOT EXISTS idx_handoffs_from ON handoffs(from_pane);
"#;

const PANE_COLS: &str = "id,workspace_id, mission_id, kind, name, ord, cli, shell, model, account, permission, effort, route, session_id, cwd, url_or_path, status, resident, created_at, closed_at, tok_in, tok_out, tok_cache_r, tok_cache_w, cost_usd, parent_id, role, agent, task_type, mcp_profile, allowed_skills, name_manual, append_system, disallowed_tools, base_tok_in, base_tok_out, base_tok_cache_r, base_tok_cache_w, base_cost_usd, agent_id, profile";
const MISSION_COLS: &str = "id, workspace_id, name, mode, status, status_reason, worktree_path, branch, base_branch, pinned, created_at, archived_at, squad_id, recipe_id, brief, style, profile, plan_json, verify_json, adopted";

fn pane_from(r: &Row) -> rusqlite::Result<PaneRow> {
    Ok(PaneRow {
        id: r.get(0)?, workspace_id: r.get(1)?, mission_id: r.get(2)?, kind: r.get(3)?, name: r.get(4)?,
        ord: r.get(5)?, cli: r.get(6)?, shell: r.get(7)?, model: r.get(8)?, account: r.get(9)?,
        permission: r.get(10)?, effort: r.get(11)?, route: r.get(12)?, session_id: r.get(13)?, cwd: r.get(14)?,
        url_or_path: r.get(15)?, status: r.get(16)?, resident: r.get(17)?, created_at: r.get(18)?,
        closed_at: r.get(19)?, tok_in: r.get(20)?, tok_out: r.get(21)?, tok_cache_r: r.get(22)?,
        tok_cache_w: r.get(23)?, cost_usd: r.get(24)?, parent_id: r.get(25)?, role: r.get(26)?, agent: r.get(27)?,
        task_type: r.get(28)?, mcp_profile: r.get(29)?, allowed_skills: r.get(30)?, name_manual: r.get(31)?,
        append_system: r.get(32)?, disallowed_tools: r.get(33)?,
        base_tok_in: r.get(34)?, base_tok_out: r.get(35)?, base_tok_cache_r: r.get(36)?, base_tok_cache_w: r.get(37)?, base_cost_usd: r.get(38)?,
        agent_id: r.get(39)?, profile: r.get(40)?,
        group_id: r.get(41)?, mcp_servers: r.get(42)?, memory_mode: r.get(43)?,
    })
}

fn mission_from(r: &Row) -> rusqlite::Result<Mission> {
    Ok(Mission {
        id: r.get(0)?, workspace_id: r.get(1)?, name: r.get(2)?, mode: r.get(3)?, status: r.get(4)?,
        status_reason: r.get(5)?, worktree_path: r.get(6)?, branch: r.get(7)?, base_branch: r.get(8)?,
        pinned: r.get(9)?, created_at: r.get(10)?, archived_at: r.get(11)?, squad_id: r.get(12)?,
        recipe_id: r.get(13)?, brief: r.get(14)?, style: r.get(15)?, profile: r.get(16)?, plan_json: r.get(17)?, verify_json: r.get(18)?, adopted: r.get(19)?,
    })
}

fn handoff_from(r: &Row) -> rusqlite::Result<Handoff> {
    Ok(Handoff {
        id: r.get(0)?, mission_id: r.get(1)?, from_pane: r.get(2)?, to_pane: r.get(3)?, summary: r.get(4)?,
        data_json: r.get(5)?, files_json: r.get(6)?, created_at: r.get(7)?, seen: r.get(8)?,
    })
}

fn task_from(r: &Row) -> rusqlite::Result<TaskItem> {
    Ok(TaskItem { id: r.get(0)?, mission_id: r.get(1)?, title: r.get(2)?, status: r.get(3)?, owner_pane: r.get(4)?, created_at: r.get(5)?, updated_at: r.get(6)? })
}

/// Colunas de `memories` na ordem de `memory_from` (sem alias; com `m.` nos joins).
const MEM_COLS: &str = "id, workspace_id, mission_id, kind, text, author_pane, created_at, status, pinned, uses, last_used_at";

fn memory_from(r: &Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: r.get(0)?, workspace_id: r.get(1)?, mission_id: r.get(2)?, kind: r.get(3)?, text: r.get(4)?, author_pane: r.get(5)?,
        created_at: r.get(6)?, status: r.get(7)?, pinned: r.get(8)?, uses: r.get(9)?, last_used_at: r.get(10)?,
    })
}

fn has_column(c: &Connection, table: &str, col: &str) -> AppResult<bool> {
    Ok(c.query_row("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2", params![table, col], |r| r.get::<_, i64>(0))? > 0)
}

fn gate_from(r: &Row) -> rusqlite::Result<Gate> {
    Ok(Gate {
        id: r.get(0)?, mission_id: r.get(1)?, pane_id: r.get(2)?, title: r.get(3)?, summary: r.get(4)?,
        options_json: r.get(5)?, decision: r.get(6)?, note: r.get(7)?, created_at: r.get(8)?, decided_at: r.get(9)?,
    })
}

fn decision_from(r: &Row) -> rusqlite::Result<Decision> {
    Ok(Decision {
        id: r.get(0)?, mission_id: r.get(1)?, pane_id: r.get(2)?, request: r.get(3)?, task_type: r.get(4)?, cli: r.get(5)?,
        model: r.get(6)?, effort: r.get(7)?, agent: r.get(8)?, decided_by: r.get(9)?, cost_usd: r.get(10)?, receipt: r.get(11)?,
        created_at: r.get(12)?,
    })
}

fn cron_from(r: &Row) -> rusqlite::Result<CronJob> {
    Ok(CronJob {
        id: r.get(0)?, workspace_id: r.get(1)?, name: r.get(2)?, expr: r.get(3)?, prompt: r.get(4)?, target: r.get(5)?,
        spawn_json: r.get(6)?, enabled: r.get(7)?, last_run: r.get(8)?, next_run: r.get(9)?, last_error: r.get(10)?,
        last_pane: r.get(11)?,
    })
}

fn ws_from(r: &Row) -> rusqlite::Result<Workspace> {
    Ok(Workspace { id: r.get(0)?, path: r.get(1)?, name: r.get(2)?, last_opened_at: r.get(3)?, pinned: r.get(4)? })
}

/// Absolute path, backslashes, no trailing separator (except a drive root like `C:\\`).
pub fn normalize_ws_path(p: &str) -> String {
    // remoto: não é caminho local (path::absolute o mangleria)
    if p.starts_with("ssh://") {
        return p.trim_end_matches('/').to_string();
    }
    let abs = std::path::absolute(p).map(|x| x.to_string_lossy().to_string()).unwrap_or_else(|_| p.to_string());
    let mut s = abs.replace('/', "\\");
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let c = Connection::open(path)?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "NORMAL")?;
        Self::migrate(&c)?;
        Ok(Self(Mutex::new(c)))
    }

    pub fn open_in_memory() -> AppResult<Self> {
        let c = Connection::open_in_memory()?;
        Self::migrate(&c)?;
        Ok(Self(Mutex::new(c)))
    }

    /// A tabela existe no banco (sqlite_master).
    fn tem_tabela(c: &Connection, nome: &str) -> AppResult<bool> {
        Ok(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [nome], |r| r.get::<_, i64>(0))? > 0)
    }

    /// A coluna existe na tabela (PRAGMA table_info).
    fn tem_coluna(c: &Connection, tabela: &str, coluna: &str) -> AppResult<bool> {
        let mut st = c.prepare(&format!("PRAGMA table_info({tabela})"))?;
        let nomes: Vec<String> = st.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
        Ok(nomes.iter().any(|n| n == coluna))
    }

    fn migrate(c: &Connection) -> AppResult<()> {
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v < 1 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V1)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        if v < 2 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V2)?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        if v < 3 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V3)?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        if v < 4 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V4)?;
            tx.pragma_update(None, "user_version", 4)?;
            tx.commit()?;
        }
        if v < 5 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V5)?;
            tx.pragma_update(None, "user_version", 5)?;
            tx.commit()?;
        }
        if v < 6 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V6)?;
            tx.pragma_update(None, "user_version", 6)?;
            tx.commit()?;
        }
        if v < 7 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V7)?;
            tx.pragma_update(None, "user_version", 7)?;
            tx.commit()?;
        }
        if v < 8 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V8)?;
            tx.pragma_update(None, "user_version", 8)?;
            tx.commit()?;
        }
        if v < 9 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V9)?;
            tx.pragma_update(None, "user_version", 9)?;
            tx.commit()?;
        }
        if v < 10 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V10)?;
            tx.pragma_update(None, "user_version", 10)?;
            tx.commit()?;
        }
        if v < 11 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V11)?;
            tx.pragma_update(None, "user_version", 11)?;
            tx.commit()?;
        }
        if v < 12 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V12)?;
            tx.pragma_update(None, "user_version", 12)?;
            tx.commit()?;
        }
        if v < 13 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V13)?;
            tx.pragma_update(None, "user_version", 13)?;
            tx.commit()?;
        }
        if v < 14 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V14)?;
            tx.pragma_update(None, "user_version", 14)?;
            tx.commit()?;
        }
        if v < 15 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V15)?;
            tx.pragma_update(None, "user_version", 15)?;
            tx.commit()?;
        }
        if v < 16 {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V16)?;
            tx.pragma_update(None, "user_version", 16)?;
            tx.commit()?;
        }
        // v17 roda sempre (IF NOT EXISTS): recria as tabelas de desperdício mesmo em banco já marcado como 19+
        {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V17)?;
            if v < 17 {
                tx.pragma_update(None, "user_version", 17)?;
            }
            tx.commit()?;
        }
        // v18 é estrutural: roda também quando o user_version já passou de 18 (outro ramo entregou a v19 antes);
        // cada coluna é conferida por si, então um banco de dev com v19 sem as colunas recebe as duas.
        if v < 18 || !Self::tem_coluna(c, "missions", "plan_json")? || !Self::tem_coluna(c, "missions", "verify_json")? {
            let tx = c.unchecked_transaction()?;
            if !Self::tem_coluna(&tx, "missions", "plan_json")? {
                tx.execute_batch("ALTER TABLE missions ADD COLUMN plan_json TEXT;")?;
            }
            if !Self::tem_coluna(&tx, "missions", "verify_json")? {
                tx.execute_batch("ALTER TABLE missions ADD COLUMN verify_json TEXT;")?;
            }
            if v < 18 {
                tx.pragma_update(None, "user_version", 18)?;
            }
            tx.commit()?;
        }
        // ▼ migrações: acrescente em ordem numérica
        // ▼ trilha B
        // roda também quando um banco numerado além de 19 (outra trilha) ainda não tem as colunas: é idempotente
        let mut falta_coluna = false;
        for (col, _) in MEMORY_COLS_V19 {
            falta_coluna |= !has_column(c, "memories", col)?;
        }
        if v < 19 || falta_coluna {
            let tx = c.unchecked_transaction()?;
            // ADD COLUMN não é idempotente: só cria a coluna que falta (ordem de migrações entre trilhas)
            for (col, ty) in MEMORY_COLS_V19 {
                if !has_column(&tx, "memories", col)? {
                    tx.execute_batch(&format!("ALTER TABLE memories ADD COLUMN {col} {ty};"))?;
                }
            }
            tx.execute_batch(SCHEMA_V19)?;
            // nunca rebaixa: um banco já em versão maior fica nela
            tx.pragma_update(None, "user_version", v.max(19))?;
            tx.commit()?;
        }
        // ▲ trilha B
        // v20 é estrutural como a v18: a tabela de fases roda quando falta, mesmo com user_version já acima de 20.
        if v < 20 || !Self::tem_tabela(c, "mission_phase_events")? {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V20)?;
            if v < 20 {
                tx.pragma_update(None, "user_version", 20)?;
            }
            tx.commit()?;
        }
        // v21 roda sempre (IF NOT EXISTS), como a v17: não depende de user_version; nunca baixa a versão
        {
            let tx = c.unchecked_transaction()?;
            for (col, ty) in MEMORY_COLS_V21 {
                if !has_column(&tx, "memories", col)? {
                    tx.execute_batch(&format!("ALTER TABLE memories ADD COLUMN {col} {ty};"))?;
                }
            }
            tx.execute_batch(SCHEMA_V21)?;
            if v < 21 {
                tx.pragma_update(None, "user_version", 21)?;
            }
            tx.commit()?;
        }
        // v23 roda sempre (IF NOT EXISTS), como a v21: a tabela de variantes; o user_version nunca desce
        {
            let tx = c.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_V23)?;
            tx.pragma_update(None, "user_version", v.max(23))?;
            tx.commit()?;
        }
        // v24: `adopted` nas missões (bloco A). Guardada por has_column: roda quando falta; o user_version nunca desce.
        if !has_column(c, "missions", "adopted")? {
            c.execute_batch("ALTER TABLE missions ADD COLUMN adopted INTEGER NOT NULL DEFAULT 0;")?;
        }
        if v < 24 {
            c.pragma_update(None, "user_version", 24)?;
        }
        // índice por data do feed de ajustes (idempotente: vale também para bancos já na v16)
        c.execute_batch("CREATE INDEX IF NOT EXISTS idx_ajustes_at ON ajustes(created_at);")?;
        Ok(())
    }

    // ---------- comparar abordagens (A1c)
    pub fn insert_compare_variant(&self, group_id: &str, mission_id: &str, label: &str) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO compare_variants(group_id, mission_id, label, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![group_id, mission_id, label, now_ms()],
        )?;
        Ok(())
    }

    pub fn compare_variants(&self, group_id: &str) -> AppResult<Vec<CompareVariant>> {
        let c = self.c();
        let mut st = c.prepare("SELECT group_id, mission_id, label, created_at FROM compare_variants WHERE group_id = ?1 ORDER BY created_at, rowid")?;
        let rows = st.query_map([group_id], |r| Ok(CompareVariant { group_id: r.get(0)?, mission_id: r.get(1)?, label: r.get(2)?, created_at: r.get(3)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Tira a variante do grupo (rollback de uma comparação que falhou no meio).
    pub fn delete_compare_variant(&self, mission_id: &str) -> AppResult<()> {
        self.c().execute("DELETE FROM compare_variants WHERE mission_id = ?1", [mission_id])?;
        Ok(())
    }

    pub fn compare_group_of(&self, mission_id: &str) -> AppResult<Option<String>> {
        Ok(self.c().query_row("SELECT group_id FROM compare_variants WHERE mission_id = ?1", [mission_id], |r| r.get(0)).optional()?)
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ---------- workspaces
    pub fn upsert_workspace(&self, path: &str) -> AppResult<Workspace> {
        let path = normalize_ws_path(path);
        let path = path.as_str();
        let name = Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.into());
        let c = self.c();
        c.execute(
            "INSERT INTO workspaces(id, path, name, last_opened_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET last_opened_at = excluded.last_opened_at",
            params![new_id(), path, name, now_ms()],
        )?;
        Ok(c.query_row("SELECT id, path, name, last_opened_at, pinned FROM workspaces WHERE path = ?1", [path], ws_from)?)
    }

    pub fn list_workspaces(&self) -> AppResult<Vec<Workspace>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, path, name, last_opened_at, pinned FROM workspaces ORDER BY pinned DESC, last_opened_at DESC")?;
        let rows = st.query_map([], ws_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_workspace(&self, id: &str) -> AppResult<Workspace> {
        let r = self.c().query_row("SELECT id, path, name, last_opened_at, pinned FROM workspaces WHERE id = ?1", [id], ws_from).optional()?;
        r.map_or_else(|| fail("not_found", "workspace não encontrado"), Ok)
    }

    pub fn remove_workspace(&self, id: &str) -> AppResult<()> {
        let c = self.c();
        c.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        c.execute("DELETE FROM memory_vectors WHERE memory_id IN (SELECT id FROM memories WHERE workspace_id = ?1)", [id])?;
        c.execute("UPDATE panes SET closed_at = ?2, restorable = 0, frozen_frame = NULL WHERE workspace_id = ?1 AND closed_at IS NULL", params![id, now_ms()])?;
        Ok(())
    }

    // ---------- panes
    pub fn next_ord(&self, workspace_id: &str) -> AppResult<i64> {
        Ok(self.c().query_row("SELECT COALESCE(MAX(ord), -1) + 1 FROM panes WHERE workspace_id = ?1 AND closed_at IS NULL", [workspace_id], |r| r.get(0))?)
    }

    pub fn insert_pane(&self, p: &PaneRow) -> AppResult<()> {
        let mcp = if p.mcp_profile.is_empty() { "full" } else { p.mcp_profile.as_str() };
        self.c().execute(
            &format!("INSERT INTO panes({PANE_COLS}, mcp_servers, memory_mode) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43)"),
            params![p.id, p.workspace_id, p.mission_id, p.kind, p.name, p.ord, p.cli, p.shell, p.model, p.account,
                p.permission, p.effort, p.route, p.session_id, p.cwd, p.url_or_path, p.status, p.resident,
                p.created_at, p.closed_at, p.tok_in, p.tok_out, p.tok_cache_r, p.tok_cache_w, p.cost_usd,
                p.parent_id, p.role, p.agent, p.task_type, mcp, p.allowed_skills, p.name_manual,
                p.append_system, p.disallowed_tools, p.base_tok_in, p.base_tok_out, p.base_tok_cache_r, p.base_tok_cache_w, p.base_cost_usd, p.agent_id, p.profile, p.mcp_servers, p.memory_mode],
        )?;
        Ok(())
    }

    /// Open (not closed) panes whose parent is `parent_id`.
    pub fn panes_by_parent(&self, parent_id: &str) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE parent_id = ?1 AND closed_at IS NULL ORDER BY ord, created_at"))?;
        let rows = st.query_map([parent_id], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Subagentes abertos ainda trabalhando: (pai, transcrito, criado em). O status vem do `announce` do runtime.
    pub fn subagentes_trabalhando(&self) -> AppResult<Vec<(String, Option<String>, i64)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT parent_id, url_or_path, created_at FROM panes WHERE kind = 'subagent' AND status = 'trabalhando' AND closed_at IS NULL AND parent_id IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Most recent panes first, closed ones included.
    pub fn recent_panes(&self, limit: i64) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes ORDER BY created_at DESC, ord LIMIT ?1"))?;
        let rows = st.query_map([limit], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_pane(&self, id: &str) -> AppResult<PaneRow> {
        let r = self.c().query_row(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE id = ?1"), [id], pane_from).optional()?;
        r.map_or_else(|| fail("not_found", "painel não encontrado"), Ok)
    }

    /// Ids of the workspaces that have at least one open pane (the attention tick skips the others).
    pub fn workspaces_with_open_panes(&self) -> AppResult<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare("SELECT DISTINCT workspace_id FROM panes WHERE closed_at IS NULL AND restorable = 0")?;
        let rows = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn open_panes(&self, workspace_id: &str) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE workspace_id = ?1 AND closed_at IS NULL AND restorable = 0 ORDER BY ord, created_at"))?;
        let rows = st.query_map([workspace_id], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Closed agent panes of a workspace that have a session to resume, newest first (at most 50).
    /// Uma linha por sessão (a mais nova) e nada de sessão que já está aberta num painel.
    pub fn resumable_panes(&self, workspace_id: &str) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes
             WHERE workspace_id = ?1 AND kind = 'agent' AND closed_at IS NOT NULL AND session_id IS NOT NULL
               AND session_id NOT IN (SELECT session_id FROM panes WHERE closed_at IS NULL AND session_id IS NOT NULL)
               AND NOT EXISTS (SELECT 1 FROM panes q WHERE q.session_id = panes.session_id AND q.closed_at IS NOT NULL AND q.closed_at > panes.closed_at)
             ORDER BY closed_at DESC LIMIT 50"
        ))?;
        let rows = st.query_map([workspace_id], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Whether some open pane is already running this session.
    pub fn session_is_open(&self, session_id: &str) -> AppResult<bool> {
        Ok(self.c().query_row("SELECT EXISTS(SELECT 1 FROM panes WHERE session_id = ?1 AND closed_at IS NULL)", [session_id], |r| r.get(0))?)
    }

    /// The newest pane of a workspace that ran this session (open or closed): where a resume takes its account, model and route.
    pub fn latest_session_pane(&self, workspace_id: &str, session_id: &str) -> AppResult<Option<PaneRow>> {
        let c = self.c();
        let r = c
            .query_row(
                &format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE workspace_id = ?1 AND session_id = ?2 ORDER BY COALESCE(closed_at, created_at) DESC LIMIT 1"),
                params![workspace_id, session_id],
                pane_from,
            )
            .optional()?;
        Ok(r)
    }

    pub fn mission_panes(&self, mission_id: &str) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE mission_id = ?1 ORDER BY ord, created_at"))?;
        let rows = st.query_map([mission_id], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_pane_status(&self, id: &str, status: &str) -> AppResult<()> {
        self.c().execute("UPDATE panes SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    pub fn close_pane(&self, id: &str) -> AppResult<()> {
        self.c().execute("UPDATE panes SET closed_at = ?2, restorable = 0, frozen_frame = NULL WHERE id = ?1", params![id, now_ms()])?;
        Ok(())
    }

    /// Automatic names (`manual = false`) never overwrite a name the user set. `true` if a row changed.
    pub fn set_pane_name(&self, id: &str, name: &str, manual: bool) -> AppResult<bool> {
        let n = self.c().execute(
            "UPDATE panes SET name = ?2, name_manual = ?3 WHERE id = ?1 AND (?3 = 1 OR name_manual = 0)",
            params![id, name, manual],
        )?;
        Ok(n > 0)
    }

    pub fn set_pane_session(&self, id: &str, session: &str) -> AppResult<()> {
        self.c().execute("UPDATE panes SET session_id = ?2 WHERE id = ?1", params![id, session])?;
        Ok(())
    }

    /// Modelo e esforço que o painel usa (a próxima retomada também).
    pub fn set_pane_model(&self, id: &str, model: Option<&str>, effort: Option<&str>) -> AppResult<()> {
        self.c().execute("UPDATE panes SET model = ?2, effort = ?3 WHERE id = ?1", params![id, model, effort])?;
        Ok(())
    }

    /// Conta do CLI que o painel usa (`None` = principal); vale na próxima retomada.
    pub fn set_pane_account(&self, id: &str, account: Option<&str>) -> AppResult<()> {
        self.c().execute("UPDATE panes SET account = ?2 WHERE id = ?1", params![id, account])?;
        Ok(())
    }

    pub fn clear_pane_session(&self, id: &str) -> AppResult<()> {
        self.c().execute("UPDATE panes SET session_id = NULL WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_pane_resident(&self, id: &str, resident: bool) -> AppResult<()> {
        self.c().execute("UPDATE panes SET resident = ?2 WHERE id = ?1", params![id, resident])?;
        Ok(())
    }

    pub fn set_pane_order(&self, ids: &[String]) -> AppResult<()> {
        let c = self.c();
        let tx = c.unchecked_transaction()?;
        for (i, id) in ids.iter().enumerate() {
            tx.execute("UPDATE panes SET ord = ?2 WHERE id = ?1", params![id, i as i64])?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---------- grupos de painéis
    pub fn list_pane_groups(&self, ws: &str) -> AppResult<Vec<PaneGroup>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, workspace_id, name, collapsed, ord FROM pane_groups WHERE workspace_id = ?1 ORDER BY ord, rowid")?;
        let rows = st.query_map([ws], |r| Ok(PaneGroup { id: r.get(0)?, workspace_id: r.get(1)?, name: r.get(2)?, collapsed: r.get(3)?, ord: r.get(4)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn create_pane_group(&self, ws: &str, name: &str) -> AppResult<PaneGroup> {
        let c = self.c();
        let ord: i64 = c.query_row("SELECT COALESCE(MAX(ord), -1) + 1 FROM pane_groups WHERE workspace_id = ?1", [ws], |r| r.get(0))?;
        let g = PaneGroup { id: new_id(), workspace_id: ws.into(), name: name.into(), collapsed: false, ord };
        c.execute("INSERT INTO pane_groups(id, workspace_id, name, collapsed, ord) VALUES (?1, ?2, ?3, 0, ?4)", params![g.id, g.workspace_id, g.name, ord])?;
        Ok(g)
    }

    pub fn update_pane_group(&self, id: &str, name: Option<&str>, collapsed: Option<bool>) -> AppResult<()> {
        let n = self.c().execute(
            "UPDATE pane_groups SET name = COALESCE(?2, name), collapsed = COALESCE(?3, collapsed) WHERE id = ?1",
            params![id, name, collapsed],
        )?;
        if n == 0 {
            return fail("not_found", "grupo não encontrado");
        }
        Ok(())
    }

    /// Apagar o grupo solta os painéis dele (eles continuam abertos, sem grupo).
    pub fn delete_pane_group(&self, id: &str) -> AppResult<()> {
        let c = self.c();
        let tx = c.unchecked_transaction()?;
        tx.execute("UPDATE panes SET group_id = NULL WHERE group_id = ?1", params![id])?;
        tx.execute("DELETE FROM pane_groups WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Põe o painel num grupo do mesmo workspace (ou tira, com None).
    pub fn set_pane_group(&self, pane: &str, group: Option<&str>) -> AppResult<()> {
        let n = self.c().execute(
            "UPDATE panes SET group_id = ?2 WHERE id = ?1
               AND (?2 IS NULL OR EXISTS (SELECT 1 FROM pane_groups g WHERE g.id = ?2 AND g.workspace_id = panes.workspace_id))",
            params![pane, group],
        )?;
        if n == 0 {
            return fail("bad_group", "painel ou grupo inválido para este workspace");
        }
        Ok(())
    }

    pub fn set_pane_usage(&self, id: &str, tok_in: i64, tok_out: i64, cache_r: i64, cache_w: i64, cost: f64) -> AppResult<()> {
        self.c().execute(
            "UPDATE panes SET tok_in = ?2, tok_out = ?3, tok_cache_r = ?4, tok_cache_w = ?5, cost_usd = ?6 WHERE id = ?1",
            params![id, tok_in, tok_out, cache_r, cache_w, cost],
        )?;
        Ok(())
    }

    /// Nova sessão do painel: o gasto até aqui vira base (continua em missão e orçamento); a sessão nova conta a partir de zero.
    pub fn rebase_pane_usage(&self, id: &str) -> AppResult<()> {
        self.c().execute(
            "UPDATE panes SET base_tok_in = tok_in, base_tok_out = tok_out, base_tok_cache_r = tok_cache_r, base_tok_cache_w = tok_cache_w, base_cost_usd = cost_usd WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn set_frame(&self, id: &str, frame: Option<&str>) -> AppResult<()> {
        self.c().execute("UPDATE panes SET frozen_frame = ?2 WHERE id = ?1", params![id, frame])?;
        Ok(())
    }

    pub fn get_frame(&self, id: &str) -> AppResult<Option<String>> {
        Ok(self.c().query_row("SELECT frozen_frame FROM panes WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten())
    }

    /// When the app closes: every open agent/terminal pane that was live or hibernated becomes restorable.
    pub fn mark_all_restorable(&self) -> AppResult<()> {
        self.c().execute("UPDATE panes SET restorable = 1 WHERE closed_at IS NULL AND kind IN ('agent','terminal') AND status IN ('iniciando','trabalhando','ocioso','acordando','hibernado','pergunta')", [])?;
        Ok(())
    }

    /// At boot: process panes that were closing, failed or finished are not restorable, so they are closed.
    pub fn close_dead_panes(&self) -> AppResult<()> {
        self.c().execute("UPDATE panes SET closed_at = ?1, frozen_frame = NULL WHERE closed_at IS NULL AND restorable = 0 AND kind IN ('agent','terminal') AND status IN ('fechando','erro','concluido')", params![now_ms()])?;
        // subagentes e painéis de execução não são processos: nunca voltam no boot (ver runtime::subagent_hook e exec_hook)
        self.c().execute("UPDATE panes SET closed_at = ?1 WHERE closed_at IS NULL AND kind IN ('subagent', 'exec', 'execucoes')", params![now_ms()])?;
        Ok(())
    }

    /// Painel de subagente aberto deste pai com este `agent_id` (um por hook repetido).
    /// Painel filho aberto de um tipo (ex.: o painel Execuções de um painel do Claude).
    pub fn pane_filho_de_kind(&self, parent_id: &str, kind: &str) -> AppResult<Option<String>> {
        Ok(self.c().query_row("SELECT id FROM panes WHERE parent_id = ?1 AND kind = ?2 AND closed_at IS NULL", params![parent_id, kind], |r| r.get(0)).optional()?)
    }

    pub fn subagent_pane(&self, parent_id: &str, agent_id: &str) -> AppResult<Option<String>> {
        Ok(self.c().query_row("SELECT id FROM panes WHERE parent_id = ?1 AND agent_id = ?2 AND closed_at IS NULL", params![parent_id, agent_id], |r| r.get(0)).optional()?)
    }

    /// Linhas de subagente ainda abertas (todas as do app), para reconciliar ao trocar o modo nas Configurações.
    pub fn open_subagent_panes(&self) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE kind = 'subagent' AND closed_at IS NULL"))?;
        let rows = st.query_map([], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_pane_url(&self, id: &str, url_or_path: &str) -> AppResult<()> {
        self.c().execute("UPDATE panes SET url_or_path = ?2 WHERE id = ?1", params![id, url_or_path])?;
        Ok(())
    }

    pub fn restorable_panes(&self, workspace_id: &str) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE workspace_id = ?1 AND restorable = 1 AND closed_at IS NULL ORDER BY ord, created_at"))?;
        let rows = st.query_map([workspace_id], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Um painel restaurável vira painel normal (o restore o respawna logo depois).
    pub fn clear_restorable(&self, id: &str) -> AppResult<()> {
        self.c().execute("UPDATE panes SET restorable = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// `keep = true` turns the panes back into normal panes (so they are re-spawned);
    /// `keep = false` closes them.
    pub fn resolve_restorable(&self, workspace_id: &str, keep: bool) -> AppResult<()> {
        let c = self.c();
        if keep {
            c.execute("UPDATE panes SET restorable = 0 WHERE workspace_id = ?1 AND restorable = 1", [workspace_id])?;
        } else {
            c.execute("UPDATE panes SET restorable = 0, closed_at = ?2, frozen_frame = NULL WHERE workspace_id = ?1 AND restorable = 1", params![workspace_id, now_ms()])?;
        }
        Ok(())
    }

    // ---------- desperdício (v17)
    /// Painéis Claude com sessão (o JSONL fica no config dir da conta).
    pub fn claude_panes(&self) -> AppResult<Vec<PaneRow>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {PANE_COLS}, group_id, mcp_servers, memory_mode FROM panes WHERE cli = 'claude' AND session_id IS NOT NULL"))?;
        let rows = st.query_map([], pane_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Estado de leitura de um JSONL (None = nunca lido).
    pub fn waste_state(&self, file: &str) -> AppResult<Option<WasteOff>> {
        let r = self
            .c()
            .query_row("SELECT offset, file_len, head_hash FROM waste_offsets WHERE file = ?1", [file], |r| {
                Ok(WasteOff { pos: r.get::<_, i64>(0)?.max(0) as u64, len: r.get::<_, i64>(1)?.max(0) as u64, head: r.get::<_, i64>(2)? as u64 })
            })
            .optional()?;
        Ok(r)
    }

    /// Uma passada inteira numa transação: achados novos (`reset` apaga os do arquivo antes, quando ele foi rotacionado) e o offset.
    /// `evidence` None = workspace sensível: só o contador fica.
    pub fn waste_commit(&self, file: &str, reset: bool, rows: &[WasteIns], off: WasteOff) -> AppResult<()> {
        let c = self.c();
        let tx = c.unchecked_transaction()?;
        if reset {
            tx.execute("DELETE FROM waste_findings WHERE file = ?1", [file])?;
        }
        for r in rows {
            tx.execute(
                "INSERT INTO waste_findings(pane_id, workspace_id, session_id, file, kind, tokens_est, evidence_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![r.pane, r.workspace, r.session, file, r.kind, r.tokens as i64, r.evidence, r.at],
            )?;
        }
        tx.execute(
            "INSERT INTO waste_offsets(file, offset, file_len, head_hash) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(file) DO UPDATE SET offset = excluded.offset, file_len = excluded.file_len, head_hash = excluded.head_hash",
            params![file, off.pos as i64, off.len as i64, off.head as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Arquivos com offset guardado (para podar os que sumiram do disco).
    pub fn waste_files(&self) -> AppResult<Vec<String>> {
        let c = self.c();
        let mut st = c.prepare("SELECT file FROM waste_offsets")?;
        let rows = st.query_map([], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
        Ok(rows)
    }

    pub fn waste_forget(&self, file: &str) -> AppResult<()> {
        self.c().execute("DELETE FROM waste_offsets WHERE file = ?1", [file])?;
        Ok(())
    }

    /// Workspace que virou sensível: a evidência já guardada some (só os contadores ficam).
    pub fn waste_clear_evidence(&self, workspace_id: &str) -> AppResult<()> {
        self.c().execute("UPDATE waste_findings SET evidence_json = NULL WHERE workspace_id = ?1 AND evidence_json IS NOT NULL", [workspace_id])?;
        Ok(())
    }

    /// Conta do painel Claude mais novo do workspace: de lá a memória do projeto é lida (None = principal).
    pub fn active_claude_account(&self, workspace_id: &str) -> AppResult<Option<String>> {
        self.last_account(workspace_id, "claude")
    }

    /// Conta do painel mais novo do workspace com a CLI `cli`, aberto ou fechado (None = principal). Segue as trocas
    /// de conta (`set_pane_account`): é a conta que o usuário usou por último naquela pasta.
    pub fn last_account(&self, workspace_id: &str, cli: &str) -> AppResult<Option<String>> {
        let r: Option<Option<String>> = self
            .c()
            .query_row("SELECT account FROM panes WHERE workspace_id = ?1 AND cli = ?2 ORDER BY created_at DESC LIMIT 1", [workspace_id, cli], |r| r.get(0))
            .optional()?;
        Ok(r.flatten())
    }

    /// Agregado por tipo de achado desde `since` (ms), opcionalmente de um workspace.
    pub fn waste_rows(&self, since: i64, ws: Option<&str>) -> AppResult<Vec<WasteRow>> {
        let c = self.c();
        // o workspace vem do próprio achado (e não do painel), para que achados de painéis removidos continuem contando
        let mut st = c.prepare(
            "SELECT w.kind, COUNT(*), COUNT(DISTINCT w.pane_id), SUM(w.tokens_est), MAX(w.created_at), SUM(CASE WHEN p.id IS NULL THEN 1 ELSE 0 END)
             FROM waste_findings w LEFT JOIN panes p ON p.id = w.pane_id
             WHERE w.created_at >= ?1 AND (?2 IS NULL OR w.workspace_id = ?2) GROUP BY w.kind ORDER BY SUM(w.tokens_est) DESC",
        )?;
        let mut rows: Vec<WasteRow> = st
            .query_map(params![since, ws], |r| {
                Ok(WasteRow { kind: r.get(0)?, count: r.get(1)?, panes: r.get(2)?, tokens_est: r.get(3)?, last_at: r.get(4)?, exemplo: None, removidos: r.get(5)? })
            })?
            .collect::<Result<_, _>>()?;
        for r in rows.iter_mut() {
            let json: Option<String> = c
                .query_row(
                    "SELECT w.evidence_json FROM waste_findings w
                     WHERE w.kind = ?1 AND w.created_at >= ?2 AND (?3 IS NULL OR w.workspace_id = ?3) AND w.evidence_json IS NOT NULL ORDER BY w.created_at DESC LIMIT 1",
                    params![r.kind, since, ws],
                    |x| x.get(0),
                )
                .optional()?;
            r.exemplo = json.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok()).and_then(|v| v.into_iter().next());
        }
        Ok(rows)
    }

    // ---------- economia e qualidade (RM-23): leituras para o agregador de `economia.rs`
    /// Painéis desde `since` (ms), com as métricas do painel (zeros quando ainda não há).
    pub fn economy_panes(&self, since: i64, ws: Option<&str>) -> AppResult<Vec<EcoPane>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT p.profile, p.task_type, p.model, p.mission_id, p.cost_usd, p.tok_in, p.tok_out, p.tok_cache_r, p.tok_cache_w,
                    COALESCE(m.turns, 0), COALESCE(m.compactions, 0), COALESCE(m.fresh_starts, 0), COALESCE(m.escalations, 0), m.first_turn_tokens
             FROM panes p LEFT JOIN pane_metrics m ON m.pane_id = p.id
             WHERE p.created_at >= ?1 AND (?2 IS NULL OR p.workspace_id = ?2)",
        )?;
        let rows = st
            .query_map(params![since, ws], |r| {
                Ok(EcoPane {
                    profile: r.get(0)?, task_type: r.get(1)?, model: r.get(2)?, mission_id: r.get(3)?, cost_usd: r.get(4)?,
                    tok_in: r.get(5)?, tok_out: r.get(6)?, cache_r: r.get(7)?, cache_w: r.get(8)?,
                    turns: r.get(9)?, compactions: r.get(10)?, fresh_starts: r.get(11)?, escalations: r.get(12)?, first_turn_tokens: r.get(13)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Entregas do workspace em `[de, ate)` (ms): (instante, dados JSON).
    pub fn quality_handoffs(&self, ws: &str, de: i64, ate: i64) -> AppResult<Vec<(i64, Option<String>)>> {
        let c = self.c();
        // LEFT JOIN: entregas de painel removido contam pelo workspace da missão
        let mut st = c.prepare(
            "SELECT h.created_at, h.data_json FROM handoffs h LEFT JOIN panes p ON p.id = h.from_pane LEFT JOIN missions m ON m.id = h.mission_id
             WHERE COALESCE(p.workspace_id, m.workspace_id) = ?1 AND h.created_at >= ?2 AND h.created_at < ?3",
        )?;
        let rows = st.query_map(params![ws, de, ate], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Painéis do workspace criados em `[de, ate)` (ms): (turnos, escalonamentos).
    pub fn quality_panes(&self, ws: &str, de: i64, ate: i64) -> AppResult<Vec<(i64, i64)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT COALESCE(m.turns, 0), COALESCE(m.escalations, 0) FROM panes p LEFT JOIN pane_metrics m ON m.pane_id = p.id
             WHERE p.workspace_id = ?1 AND p.created_at >= ?2 AND p.created_at < ?3",
        )?;
        let rows = st.query_map(params![ws, de, ate], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- perfil de trabalho, métricas e ajustes (v16)
    pub fn set_mission_profile(&self, id: &str, profile: Option<&str>) -> AppResult<()> {
        if self.c().execute("UPDATE missions SET profile = ?2 WHERE id = ?1", params![id, profile])? == 0 {
            return fail("not_found", "missão não encontrada");
        }
        Ok(())
    }

    /// Abre uma fase da missão (`mission_phase_events`). Devolve o id da linha.
    pub fn fase_abrir(&self, missao: &str, pane: Option<&str>, task: Option<&str>, fase: &str, inicio: i64) -> AppResult<i64> {
        let c = self.c();
        c.execute("INSERT INTO mission_phase_events(mission_id, pane_id, task_id, phase, started_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![missao, pane, task, fase, inicio])?;
        Ok(c.last_insert_rowid())
    }

    /// A fase aberta mais recente de (missão, tarefa, fase). A tarefa `None` casa com as fases sem tarefa.
    pub fn fase_aberta(&self, missao: &str, task: Option<&str>, fase: &str) -> AppResult<Option<i64>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id FROM mission_phase_events WHERE mission_id = ?1 AND task_id IS ?2 AND phase = ?3 AND ended_at IS NULL ORDER BY id DESC LIMIT 1")?;
        Ok(st.query_row(params![missao, task, fase], |r| r.get(0)).optional()?)
    }

    /// Fecha a fase aberta com o fim e os tokens/custo medidos no intervalo. Só fecha se ainda estiver aberta.
    pub fn fase_fechar(&self, id: i64, fim: i64, tok_in: i64, tok_out: i64, custo: f64) -> AppResult<()> {
        self.c().execute("UPDATE mission_phase_events SET ended_at = ?2, tok_in = ?3, tok_out = ?4, cost_usd = ?5 WHERE id = ?1 AND ended_at IS NULL", params![id, fim, tok_in, tok_out, custo])?;
        Ok(())
    }

    /// Boot: fase que estava aberta quando o app parou fecha no próprio início (sem tokens: a base morreu com o processo).
    pub fn fechar_fases_orfas(&self) -> AppResult<usize> {
        Ok(self.c().execute("UPDATE mission_phase_events SET ended_at = started_at WHERE ended_at IS NULL", [])?)
    }

    /// Boot: fases de missões que já não existem saem do banco.
    pub fn prune_fases_orfas(&self) -> AppResult<usize> {
        Ok(self.c().execute("DELETE FROM mission_phase_events WHERE mission_id NOT IN (SELECT id FROM missions)", [])?)
    }

    /// Todas as fases da missão, da mais antiga para a mais nova.
    pub fn fases_da_missao(&self, missao: &str) -> AppResult<Vec<FaseRow>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, mission_id, pane_id, task_id, phase, started_at, ended_at, tok_in, tok_out, cost_usd FROM mission_phase_events WHERE mission_id = ?1 ORDER BY id")?;
        let rows = st
            .query_map([missao], |r| {
                Ok(FaseRow { id: r.get(0)?, mission_id: r.get(1)?, pane_id: r.get(2)?, task_id: r.get(3)?, phase: r.get(4)?, started_at: r.get(5)?, ended_at: r.get(6)?, tok_in: r.get(7)?, tok_out: r.get(8)?, cost_usd: r.get(9)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Soma 1 ao contador `field` (lista fechada em `METRIC_FIELDS`).
    pub fn metric_bump(&self, pane: &str, field: &str) -> AppResult<()> {
        let field = metric_field(field)?;
        self.c().execute(
            &format!("INSERT INTO pane_metrics(pane_id, updated_at, {field}) VALUES (?1, ?2, 1) ON CONFLICT(pane_id) DO UPDATE SET {field} = COALESCE({field}, 0) + 1, updated_at = excluded.updated_at"),
            params![pane, now_ms()],
        )?;
        Ok(())
    }

    /// Grava o valor de `field` (lista fechada em `METRIC_FIELDS`). Uma escrita só, e a linha só muda se o valor mudou.
    /// Os valores são por sessão do CLI: zeram quando o CLI recomeça (`/clear`), como o próprio CLI.
    pub fn metric_set(&self, pane: &str, field: &str, value: i64) -> AppResult<()> {
        let field = metric_field(field)?;
        self.c().execute(
            &format!("INSERT INTO pane_metrics(pane_id, updated_at, {field}) VALUES (?1, ?2, ?3) ON CONFLICT(pane_id) DO UPDATE SET {field} = excluded.{field}, updated_at = excluded.updated_at WHERE pane_metrics.{field} IS NOT excluded.{field}"),
            params![pane, now_ms(), value],
        )?;
        Ok(())
    }

    /// Apaga as métricas de painéis fechados há mais de `before` (ms desde a época). Chamado no boot.
    pub fn prune_metrics(&self, before: i64) -> AppResult<usize> {
        Ok(self.c().execute("DELETE FROM pane_metrics WHERE pane_id IN (SELECT id FROM panes WHERE closed_at IS NOT NULL AND closed_at < ?1)", [before])?)
    }

    /// Mantém só os `keep` ajustes mais recentes e apaga os anteriores a `before` (ms). Chamado a cada registro.
    pub fn prune_ajustes(&self, keep: i64, before: i64) -> AppResult<()> {
        self.c().execute("DELETE FROM ajustes WHERE created_at < ?1", [before])?;
        self.c().execute("DELETE FROM ajustes WHERE id NOT IN (SELECT id FROM ajustes ORDER BY created_at DESC LIMIT ?1)", [keep])?;
        Ok(())
    }

    /// Filhos (delegações) fechados depois de `desde` (ms): a fronteira de tarefa do piloto.
    pub fn filhos_fechados_desde(&self, parent: &str, desde: i64) -> AppResult<i64> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM panes WHERE parent_id = ?1 AND closed_at IS NOT NULL AND closed_at > ?2", params![parent, desde], |r| r.get(0))?)
    }

    /// Gates do painel decididos depois de `desde` (ms): a fronteira de fase do maestro.
    pub fn gates_decididos_desde(&self, pane: &str, desde: i64) -> AppResult<i64> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM gates WHERE pane_id = ?1 AND decided_at > ?2", params![pane, desde], |r| r.get(0))?)
    }

    /// Grava o efeito medido de um ajuste (JSON já pronto, limitado pelo chamador).
    pub fn set_ajuste_efeito(&self, id: &str, efeito_json: &str) -> AppResult<()> {
        self.c().execute("UPDATE ajustes SET efeito_json = ?2 WHERE id = ?1", params![id, efeito_json])?;
        Ok(())
    }

    /// Turnos totais dos painéis já fechados do mesmo papel e tipo de tarefa (histórico da política de compactação).
    pub fn turnos_fechados(&self, papel: Option<&str>, tipo: Option<&str>) -> AppResult<Vec<i64>> {
        let c = self.c();
        let mut st = c.prepare("SELECT m.turns FROM pane_metrics m JOIN panes p ON p.id = m.pane_id WHERE p.closed_at IS NOT NULL AND p.role IS ?1 AND p.task_type IS ?2 AND m.turns > 0")?;
        let rows = st.query_map(params![papel, tipo], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
        Ok(rows)
    }

    pub fn pane_metrics(&self, pane: &str) -> AppResult<Option<PaneMetrics>> {
        Ok(self
            .c()
            .query_row(
                "SELECT turns, first_turn_tokens, ctx_peak, compactions, fresh_starts, escalations, updated_at FROM pane_metrics WHERE pane_id = ?1",
                [pane],
                |r| Ok(PaneMetrics { turns: r.get(0)?, first_turn_tokens: r.get(1)?, ctx_peak: r.get(2)?, compactions: r.get(3)?, fresh_starts: r.get(4)?, escalations: r.get(5)?, updated_at: r.get(6)? }),
            )
            .optional()?)
    }

    pub fn insert_ajuste(&self, a: &Ajuste) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO ajustes(id, created_at, workspace_id, mission_id, pane_id, kind, acao, motivo, antes_json, depois_json, estimativa_json, efeito_json, desfazivel, desfeito_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![a.id, a.created_at, a.workspace_id, a.mission_id, a.pane_id, a.kind, a.acao, a.motivo, a.antes_json, a.depois_json, a.estimativa_json, a.efeito_json, a.desfazivel, a.desfeito_at],
        )?;
        Ok(())
    }

    /// Ajustes mais recentes primeiro; `workspace_id` None = todos.
    pub fn list_ajustes(&self, workspace_id: Option<&str>, limit: i64) -> AppResult<Vec<Ajuste>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, created_at, workspace_id, mission_id, pane_id, kind, acao, motivo, antes_json, depois_json, estimativa_json, efeito_json, desfazivel, desfeito_at FROM ajustes WHERE (?1 IS NULL OR workspace_id = ?1) ORDER BY created_at DESC LIMIT ?2")?;
        let rows = st.query_map(params![workspace_id, limit], ajuste_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_ajuste(&self, id: &str) -> AppResult<Ajuste> {
        let r = self.c().query_row("SELECT id, created_at, workspace_id, mission_id, pane_id, kind, acao, motivo, antes_json, depois_json, estimativa_json, efeito_json, desfazivel, desfeito_at FROM ajustes WHERE id = ?1", [id], ajuste_from).optional()?;
        r.map_or_else(|| fail("not_found", "ajuste não encontrado"), Ok)
    }

    /// Quantas compactações aplicadas têm rowid maior que `rowid` (checagem barata antes de ler o aprendizado).
    pub fn contar_compactacoes_depois(&self, rowid: i64) -> AppResult<i64> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM ajustes WHERE rowid > ?1 AND kind = 'compactacao' AND acao = 'aplicado'", [rowid], |r| r.get(0))?)
    }

    /// Compactações aplicadas com rowid maior que `rowid`, em ordem de rowid, com o rowid de cada uma.
    pub fn compactacoes_depois(&self, rowid: i64, limit: i64) -> AppResult<Vec<(i64, Ajuste)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, created_at, workspace_id, mission_id, pane_id, kind, acao, motivo, antes_json, depois_json, estimativa_json, efeito_json, desfazivel, desfeito_at, rowid FROM ajustes WHERE rowid > ?1 AND kind = 'compactacao' AND acao = 'aplicado' ORDER BY rowid LIMIT ?2")?;
        let rows = st.query_map(params![rowid, limit], |r| Ok((r.get(14)?, ajuste_from(r)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_ajuste_undone(&self, id: &str, at: i64) -> AppResult<()> {
        self.c().execute("UPDATE ajustes SET desfeito_at = ?2 WHERE id = ?1", params![id, at])?;
        Ok(())
    }

    /// Muda a ação de um ajuste (proposta aplicada vira `aplicado`) e se ele passa a ser desfazível.
    pub fn set_ajuste_acao(&self, id: &str, acao: &str, desfazivel: bool) -> AppResult<()> {
        self.c().execute("UPDATE ajustes SET acao = ?2, desfazivel = ?3 WHERE id = ?1", params![id, acao, desfazivel])?;
        Ok(())
    }

    // ---------- missions
    pub fn insert_mission(&self, m: &Mission) -> AppResult<()> {
        self.c().execute(
            &format!("INSERT INTO missions({MISSION_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)"),
            params![m.id, m.workspace_id, m.name, m.mode, m.status, m.status_reason, m.worktree_path, m.branch,
                m.base_branch, m.pinned, m.created_at, m.archived_at, m.squad_id, m.recipe_id, m.brief, m.style, m.profile, m.plan_json, m.verify_json, m.adopted],
        )?;
        Ok(())
    }

    /// Grava a configuração de verificação da missão (`verify_json`).
    pub fn set_mission_verify(&self, id: &str, json: &str) -> AppResult<()> {
        self.c().execute("UPDATE missions SET verify_json = ?2 WHERE id = ?1", params![id, json])?;
        Ok(())
    }

    /// Grava o estado de aprovação do plano (`plan_json`), a fonte que `planexec` consulta.
    pub fn set_mission_plan(&self, id: &str, json: &str) -> AppResult<()> {
        self.c().execute("UPDATE missions SET plan_json = ?2 WHERE id = ?1", params![id, json])?;
        Ok(())
    }

    pub fn update_mission(&self, m: &Mission) -> AppResult<()> {
        self.c().execute(
            "UPDATE missions SET name=?2, status=?3, status_reason=?4, worktree_path=?5, pinned=?6, archived_at=?7, brief=?8, style=?9 WHERE id=?1",
            params![m.id, m.name, m.status, m.status_reason, m.worktree_path, m.pinned, m.archived_at, m.brief, m.style],
        )?;
        Ok(())
    }

    /// Branch atual de uma missão adotada (muda por fora, pelo `ambiente.sh`); None = HEAD destacado.
    pub fn set_mission_branch(&self, id: &str, branch: Option<&str>) -> AppResult<()> {
        self.c().execute("UPDATE missions SET branch = ?2 WHERE id = ?1", params![id, branch])?;
        Ok(())
    }

    /// Base de uma missão adotada (segue o `git.base` do projeto).
    pub fn set_mission_base(&self, id: &str, base: &str) -> AppResult<()> {
        self.c().execute("UPDATE missions SET base_branch = ?2 WHERE id = ?1", params![id, base])?;
        Ok(())
    }

    pub fn get_mission(&self, id: &str) -> AppResult<Mission> {
        let r = self.c().query_row(&format!("SELECT {MISSION_COLS} FROM missions WHERE id = ?1"), [id], mission_from).optional()?;
        r.map_or_else(|| fail("not_found", "missão não encontrada"), Ok)
    }

    pub fn list_missions(&self, workspace_id: &str) -> AppResult<Vec<Mission>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {MISSION_COLS} FROM missions WHERE workspace_id = ?1 ORDER BY pinned DESC, created_at DESC"))?;
        let rows = st.query_map([workspace_id], mission_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Por missão, incluindo painéis já fechados: (id, entrada+saída, cache lido+escrito, custo).
    /// O cache não entra em "tokens": é re-cobrado a cada turno e infla o número sem ser consumo novo.
    pub fn mission_totals(&self, workspace_id: &str) -> AppResult<Vec<(String, i64, i64, f64)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT mission_id, SUM(tok_in + tok_out), SUM(tok_cache_r + tok_cache_w), SUM(cost_usd)
             FROM panes WHERE workspace_id = ?1 AND mission_id IS NOT NULL GROUP BY mission_id",
        )?;
        let rows = st.query_map([workspace_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (id, nome, custo) das missões com painéis no workspace, em uma consulta (orçamento por missão).
    pub fn mission_spend(&self, workspace_id: &str) -> AppResult<Vec<(String, String, f64)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT p.mission_id, m.name, SUM(p.cost_usd)
             FROM panes p JOIN missions m ON m.id = p.mission_id
             WHERE p.workspace_id = ?1 GROUP BY p.mission_id",
        )?;
        let rows = st.query_map([workspace_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Custo de uma missão, incluindo painéis já fechados (o orçamento da missão compara com isto).
    pub fn mission_cost(&self, mission_id: &str) -> AppResult<f64> {
        Ok(self.c().query_row("SELECT COALESCE(SUM(cost_usd), 0) FROM panes WHERE mission_id = ?1", [mission_id], |r| r.get(0))?)
    }

    /// Missões recentes de todos os workspaces (Home → "Continuar de onde parei"), do uso mais novo ao mais antigo.
    /// Uso = o mais novo entre a criação, a abertura ou o fechamento de um painel e uma entrega.
    pub fn recent_missions(&self, limit: i64) -> AppResult<Vec<RecentMission>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.id, m.workspace_id, w.name, w.path, m.name, m.status,
                    (SELECT COUNT(*) FROM panes p WHERE p.mission_id = m.id AND p.closed_at IS NULL),
                    (SELECT COALESCE(SUM(p.cost_usd), 0) FROM panes p WHERE p.mission_id = m.id),
                    (SELECT COUNT(*) FROM handoffs h WHERE h.mission_id = m.id AND h.seen = 0),
                    (SELECT COUNT(*) FROM gates g WHERE g.mission_id = m.id AND g.decision IS NULL),
                    MAX(m.created_at,
                        COALESCE((SELECT MAX(COALESCE(p.closed_at, p.created_at)) FROM panes p WHERE p.mission_id = m.id), 0),
                        COALESCE((SELECT MAX(h.created_at) FROM handoffs h WHERE h.mission_id = m.id), 0)) AS last
             FROM missions m JOIN workspaces w ON w.id = m.workspace_id
             WHERE m.archived_at IS NULL
             ORDER BY last DESC LIMIT ?1",
        )?;
        let rows = st
            .query_map([limit], |r| {
                Ok(RecentMission {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    workspace_name: r.get(2)?,
                    workspace_path: r.get(3)?,
                    name: r.get(4)?,
                    status: r.get(5)?,
                    live_panes: r.get(6)?,
                    cost_usd: r.get(7)?,
                    unseen: r.get(8)?,
                    open_gates: r.get(9)?,
                    last_activity: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- handoffs
    pub fn insert_handoff(&self, h: &Handoff) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO handoffs(id, mission_id, from_pane, to_pane, summary, data_json, files_json, created_at, seen)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![h.id, h.mission_id, h.from_pane, h.to_pane, h.summary, h.data_json, h.files_json, h.created_at, h.seen],
        )?;
        Ok(())
    }

    /// Most recent first. `None` filters are ignored.
    pub fn handoffs(&self, mission_id: Option<&str>, to_pane: Option<&str>, limit: i64) -> AppResult<Vec<Handoff>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, mission_id, from_pane, to_pane, summary, data_json, files_json, created_at, seen FROM handoffs
             WHERE (?1 IS NULL OR mission_id = ?1) AND (?2 IS NULL OR to_pane = ?2)
             ORDER BY created_at DESC, rowid DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![mission_id, to_pane, limit], handoff_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Whether the pane has delivered at least one handoff.
    pub fn has_handoff_from(&self, pane_id: &str) -> AppResult<bool> {
        Ok(self.c().query_row("SELECT EXISTS(SELECT 1 FROM handoffs WHERE from_pane = ?1)", [pane_id], |r| r.get(0))?)
    }

    pub fn get_handoff(&self, id: &str) -> AppResult<Handoff> {
        let r = self.c().query_row(
            "SELECT id, mission_id, from_pane, to_pane, summary, data_json, files_json, created_at, seen FROM handoffs WHERE id = ?1",
            [id],
            handoff_from,
        ).optional()?;
        r.map_or_else(|| fail("not_found", "handoff não encontrado"), Ok)
    }

    pub fn mark_handoffs_seen(&self, mission_id: &str) -> AppResult<()> {
        self.c().execute("UPDATE handoffs SET seen = 1 WHERE mission_id = ?1 AND seen = 0", [mission_id])?;
        Ok(())
    }

    /// (mission id, unseen handoffs) for missions of the workspace that have any.
    pub fn unseen_handoffs(&self, workspace_id: &str) -> AppResult<Vec<(String, i64)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT h.mission_id, COUNT(*) FROM handoffs h JOIN missions m ON m.id = h.mission_id
             WHERE m.workspace_id = ?1 AND h.seen = 0 GROUP BY h.mission_id ORDER BY h.mission_id",
        )?;
        let rows = st.query_map([workspace_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- tasks
    pub fn upsert_task(&self, t: &TaskItem) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO tasks(id, mission_id, title, status, owner_pane, created_at, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, status = excluded.status,
               owner_pane = excluded.owner_pane, updated_at = excluded.updated_at",
            params![t.id, t.mission_id, t.title, t.status, t.owner_pane, t.created_at, t.updated_at],
        )?;
        Ok(())
    }

    /// Boot: nada roda ainda, então toda tarefa `em_execucao` volta a `pendente` (sem dono). Devolve quantas.
    pub fn requeue_running_tasks(&self, now: i64) -> AppResult<usize> {
        Ok(self.c().execute("UPDATE tasks SET status = 'pendente', owner_pane = NULL, updated_at = ?1 WHERE status IN ('em_execucao', 'verificando')", [now])?)
    }

    pub fn tasks(&self, mission_id: &str) -> AppResult<Vec<TaskItem>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, mission_id, title, status, owner_pane, created_at, updated_at FROM tasks WHERE mission_id = ?1 ORDER BY created_at, rowid")?;
        let rows = st.query_map([mission_id], task_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_task(&self, id: &str) -> AppResult<()> {
        self.c().execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- media spending caps
    /// Attempts of `kind` since `since_ms`, for the whole workspace or one mission.
    pub fn media_attempts(&self, kind: &str, since_ms: i64, mission: Option<&str>) -> AppResult<i64> {
        let c = self.c();
        let n = c.query_row(
            "SELECT COUNT(*) FROM media_usage WHERE kind = ?1 AND created_at >= ?2 AND (?3 IS NULL OR mission_id = ?3)",
            params![kind, since_ms, mission],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    pub fn add_media_attempt(&self, kind: &str, mission: Option<&str>, ts: i64) -> AppResult<()> {
        self.c().execute("INSERT INTO media_usage(kind, mission_id, created_at) VALUES (?1, ?2, ?3)", params![kind, mission, ts])?;
        Ok(())
    }

    // ---------- global dictation history
    /// Most recent first.
    pub fn voice_history(&self, limit: i64) -> AppResult<Vec<VoiceEntry>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, ts, mode, text FROM voice_history ORDER BY ts DESC, id DESC LIMIT ?1")?;
        let rows = st.query_map(params![limit], |r| Ok(VoiceEntry { id: r.get(0)?, ts: r.get(1)?, mode: r.get(2)?, text: r.get(3)? }))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// One poll's windows, stored together at `ts` (the Redline history).
    pub fn insert_limits(&self, ts: i64, windows: &[crate::redline::Window]) -> AppResult<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        for w in windows {
            tx.execute(
                "INSERT INTO limits_history(ts, provider, account, window, used_percent, resets_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![ts, w.provider, w.account, w.window, w.used_percent, w.resets_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Drops the Redline history older than `before` (epoch ms).
    pub fn prune_limits(&self, before: i64) -> AppResult<()> {
        self.c().execute("DELETE FROM limits_history WHERE ts < ?1", params![before])?;
        Ok(())
    }

    /// Redline history from `since` (epoch ms), oldest first.
    pub fn limits_since(&self, since: i64) -> AppResult<Vec<crate::redline::LimitPoint>> {
        let c = self.c();
        let mut st = c.prepare("SELECT ts, provider, account, window, used_percent FROM limits_history WHERE ts >= ?1 ORDER BY ts")?;
        let rows = st.query_map(params![since], |r| {
            Ok(crate::redline::LimitPoint { ts: r.get(0)?, provider: r.get(1)?, account: r.get(2)?, window: r.get(3)?, used_percent: r.get(4)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_voice_history(&self, ts: i64, mode: &str, text: &str) -> AppResult<()> {
        self.c().execute("INSERT INTO voice_history(ts, mode, text) VALUES (?1, ?2, ?3)", params![ts, mode, text])?;
        Ok(())
    }

    pub fn clear_voice_history(&self) -> AppResult<()> {
        self.c().execute("DELETE FROM voice_history", [])?;
        Ok(())
    }

    // ---------- memories
    pub fn insert_memory(&self, m: &Memory) -> AppResult<i64> {
        let c = self.c();
        let status = if m.status.is_empty() { "ativa" } else { m.status.as_str() };
        c.execute(
            "INSERT INTO memories(workspace_id, mission_id, kind, text, author_pane, created_at, status, pinned, uses, last_used_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![m.workspace_id, m.mission_id, m.kind, m.text, m.author_pane, m.created_at, status, m.pinned, m.uses, m.last_used_at],
        )?;
        Ok(c.last_insert_rowid())
    }

    /// Active memories, most recent first. With a mission: that mission's memories plus the workspace-wide ones (no mission).
    pub fn memories(&self, workspace_id: &str, mission_id: Option<&str>, limit: i64) -> AppResult<Vec<Memory>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {MEM_COLS} FROM memories
             WHERE workspace_id = ?1 AND status = 'ativa' AND (?2 IS NULL OR mission_id = ?2 OR mission_id IS NULL)
             ORDER BY created_at DESC, id DESC LIMIT ?3"
        ))?;
        let rows = st.query_map(params![workspace_id, mission_id, limit], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Memories of one state ("sugerida" or "arquivada"), most recent first. The workspace only (no mission filter).
    pub fn memories_in(&self, workspace_id: &str, status: &str, limit: i64) -> AppResult<Vec<Memory>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {MEM_COLS} FROM memories WHERE workspace_id = ?1 AND status = ?2 ORDER BY created_at DESC, id DESC LIMIT ?3"))?;
        let rows = st.query_map(params![workspace_id, status, limit], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Every memory of a workspace and kind, whatever its state (the dedup check reads these).
    pub fn memories_of_kind(&self, workspace_id: &str, kind: &str) -> AppResult<Vec<Memory>> {
        let c = self.c();
        let mut st = c.prepare(&format!("SELECT {MEM_COLS} FROM memories WHERE workspace_id = ?1 AND kind = ?2 ORDER BY id"))?;
        let rows = st.query_map(params![workspace_id, kind], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Active, pinned memories of the scope (same mission rule as `memories`), most recent first.
    pub fn pinned_memories(&self, workspace_id: &str, mission_id: Option<&str>) -> AppResult<Vec<Memory>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {MEM_COLS} FROM memories
             WHERE workspace_id = ?1 AND status = 'ativa' AND pinned = 1 AND (?2 IS NULL OR mission_id = ?2 OR mission_id IS NULL)
             ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = st.query_map(params![workspace_id, mission_id], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Active fato/decisão/risco memories that match `fts` (an FTS5 expression), best bm25 first, then the newest id.
    pub fn relevant_memories(&self, workspace_id: &str, mission_id: Option<&str>, fts: &str, limit: i64) -> AppResult<Vec<Memory>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.id, m.workspace_id, m.mission_id, m.kind, m.text, m.author_pane, m.created_at, m.status, m.pinned, m.uses, m.last_used_at
             FROM memories m JOIN memories_fts ON memories_fts.rowid = m.id
             WHERE memories_fts MATCH ?1 AND m.workspace_id = ?2 AND m.status = 'ativa' AND m.kind IN ('fato','decisao','risco')
               AND (?3 IS NULL OR m.mission_id = ?3 OR m.mission_id IS NULL)
             ORDER BY bm25(memories_fts), m.id DESC LIMIT ?4",
        )?;
        let rows = st.query_map(params![fts, workspace_id, mission_id, limit], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// FTS5 search over active memories. Each whitespace-separated term is quoted, so punctuation cannot break the query.
    pub fn search_memories(&self, workspace_id: &str, q: &str, limit: i64) -> AppResult<Vec<Memory>> {
        let fts: Vec<String> = q.split_whitespace().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
        if fts.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.id, m.workspace_id, m.mission_id, m.kind, m.text, m.author_pane, m.created_at, m.status, m.pinned, m.uses, m.last_used_at
             FROM memories m JOIN memories_fts ON memories_fts.rowid = m.id
             WHERE m.workspace_id = ?1 AND m.status = 'ativa' AND memories_fts MATCH ?2 ORDER BY m.created_at DESC, m.id DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![workspace_id, fts.join(" "), limit], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Moves a memory to another state ("ativa", "sugerida", "arquivada"). false when no memory has that id.
    pub fn set_memory_status(&self, id: i64, status: &str) -> AppResult<bool> {
        Ok(self.c().execute("UPDATE memories SET status = ?2 WHERE id = ?1", params![id, status])? > 0)
    }

    pub fn get_memory(&self, id: i64) -> AppResult<Option<Memory>> {
        Ok(self.c().query_row(&format!("SELECT {MEM_COLS} FROM memories WHERE id = ?1"), [id], memory_from).optional()?)
    }

    /// false when no memory has that id.
    pub fn set_memory_pinned(&self, id: i64, pinned: bool) -> AppResult<bool> {
        Ok(self.c().execute("UPDATE memories SET pinned = ?2 WHERE id = ?1", params![id, pinned])? > 0)
    }

    /// A memory that entered a brief (or a merge): one more use, last used at `at`.
    pub fn touch_memories(&self, ids: &[i64], at: i64) -> AppResult<()> {
        let c = self.c();
        let tx = c.unchecked_transaction()?;
        for id in ids {
            tx.execute("UPDATE memories SET uses = uses + 1, last_used_at = ?2 WHERE id = ?1", params![id, at])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Dedup merge: the kept text and one more use.
    pub fn merge_memory(&self, id: i64, text: &str) -> AppResult<()> {
        self.c().execute("UPDATE memories SET text = ?2, uses = uses + 1 WHERE id = ?1", params![id, text])?;
        Ok(())
    }

    /// Expiry: active, unpinned memories not used since `cutoff` (last use, or creation) become "arquivada". Nothing is deleted.
    pub fn expire_unused(&self, cutoff: i64) -> AppResult<usize> {
        // preferências globais nunca expiram: são do usuário, não do workspace
        Ok(self.c().execute("UPDATE memories SET status = 'arquivada' WHERE status = 'ativa' AND pinned = 0 AND workspace_id <> '_global' AND COALESCE(last_used_at, created_at) < ?1", [cutoff])?)
    }

    /// false when no memory has that id.
    pub fn update_memory(&self, id: i64, text: &str) -> AppResult<bool> {
        Ok(self.c().execute("UPDATE memories SET text = ?2 WHERE id = ?1", params![id, text])? > 0)
    }

    /// Apaga a memória (e, pelo gatilho `memories_ad_vetor`, o vetor dela).
    pub fn delete_memory(&self, id: i64) -> AppResult<()> {
        self.c().execute("DELETE FROM memories WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---------- busca semântica (M1c): vetores por memória
    /// Vetores das memórias não arquivadas do workspace (opcionalmente de um tipo) gerados pelo `model`: (id, tipo, estado, bytes).
    pub fn vectors(&self, workspace_id: &str, kind: Option<&str>, model: &str) -> AppResult<Vec<(i64, String, String, Vec<u8>)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.id, m.kind, m.status, v.vec FROM memory_vectors v JOIN memories m ON m.id = v.memory_id
             WHERE m.workspace_id = ?1 AND m.status <> 'arquivada' AND v.model = ?2 AND (?3 IS NULL OR m.kind = ?3)",
        )?;
        let rows = st
            .query_map(params![workspace_id, model, kind], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn count_vectors(&self, workspace_id: &str) -> AppResult<i64> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM memory_vectors v JOIN memories m ON m.id = v.memory_id WHERE m.workspace_id = ?1", [workspace_id], |r| r.get(0))?)
    }

    pub fn put_vector(&self, id: i64, model: &str, dim: usize, vec: &[u8]) -> AppResult<()> {
        self.c().execute("INSERT OR REPLACE INTO memory_vectors(memory_id, model, dim, vec) VALUES (?1, ?2, ?3, ?4)", params![id, model, dim as i64, vec])?;
        Ok(())
    }

    pub fn delete_vector(&self, id: i64) -> AppResult<()> {
        self.c().execute("DELETE FROM memory_vectors WHERE memory_id = ?1", [id])?;
        Ok(())
    }

    /// Memórias não arquivadas do workspace, depois do id `apos`, que ainda não têm vetor (a reconstrução ao ligar a busca).
    pub fn memories_sem_vetor(&self, workspace_id: &str, apos: i64, limit: i64) -> AppResult<Vec<Memory>> {
        let c = self.c();
        let mut st = c.prepare(&format!(
            "SELECT {MEM_COLS} FROM memories m WHERE m.workspace_id = ?1 AND m.status <> 'arquivada' AND m.id > ?2
             AND NOT EXISTS (SELECT 1 FROM memory_vectors v WHERE v.memory_id = m.id) ORDER BY m.id LIMIT ?3"
        ))?;
        let rows = st.query_map(params![workspace_id, apos, limit], memory_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Tem vetor (qualquer modelo)?
    pub fn tem_vetor(&self, id: i64) -> AppResult<bool> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM memory_vectors WHERE memory_id = ?1", [id], |r| r.get::<_, i64>(0))? > 0)
    }

    /// Apaga os vetores do workspace (ao virar sensível ou ser removido). As memórias ficam.
    pub fn delete_vectors_of_workspace(&self, workspace_id: &str) -> AppResult<()> {
        self.c().execute("DELETE FROM memory_vectors WHERE memory_id IN (SELECT id FROM memories WHERE workspace_id = ?1)", [workspace_id])?;
        Ok(())
    }

    /// Dedup: a memória `id` vira arquivada, com o motivo "duplicata de #nova". Nada é apagado.
    pub fn arquivar_duplicata(&self, id: i64, nova: i64) -> AppResult<()> {
        self.c().execute("UPDATE memories SET status = 'arquivada', status_motivo = ?2 WHERE id = ?1", params![id, format!("duplicata de #{nova}")])?;
        Ok(())
    }

    /// Motivo do arquivamento (dedup), quando houver.
    pub fn status_motivo(&self, id: i64) -> AppResult<Option<String>> {
        Ok(self.c().query_row("SELECT status_motivo FROM memories WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
    }

    /// Ids das memórias ativas que casam com `fts` (expressão FTS5 pronta), com o bm25 (positivo: maior é melhor).
    pub fn fts_scores(&self, workspace_id: &str, fts: &str, limit: i64) -> AppResult<Vec<(i64, f32)>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT m.id, -bm25(memories_fts) FROM memories m JOIN memories_fts ON memories_fts.rowid = m.id
             WHERE m.workspace_id = ?1 AND m.status = 'ativa' AND memories_fts MATCH ?2 ORDER BY bm25(memories_fts) LIMIT ?3",
        )?;
        let rows = st.query_map(params![workspace_id, fts, limit], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)? as f32)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- gates
    pub fn insert_gate(&self, g: &Gate) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO gates(id, mission_id, pane_id, title, summary, options_json, decision, note, created_at, decided_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![g.id, g.mission_id, g.pane_id, g.title, g.summary, g.options_json, g.decision, g.note, g.created_at, g.decided_at],
        )?;
        Ok(())
    }

    /// Decides an undecided gate. Returns false when it was already decided (the guard makes this atomic).
    pub fn decide_gate(&self, id: &str, decision: &str, note: Option<&str>) -> AppResult<bool> {
        let n = self.c().execute(
            "UPDATE gates SET decision = ?2, note = ?3, decided_at = ?4 WHERE id = ?1 AND decision IS NULL",
            params![id, decision, note, now_ms()],
        )?;
        Ok(n == 1)
    }

    /// At boot: no process exists yet, so no `gate_request` call is waiting; gates left open by the last run are cancelled.
    pub fn cancel_open_gates(&self) -> AppResult<usize> {
        Ok(self.c().execute("UPDATE gates SET decision = 'cancelado', decided_at = ?1 WHERE decision IS NULL", params![now_ms()])?)
    }

    /// Undecided gates of one pane.
    pub fn open_gates_of_pane(&self, pane_id: &str) -> AppResult<Vec<Gate>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, mission_id, pane_id, title, summary, options_json, decision, note, created_at, decided_at
             FROM gates WHERE pane_id = ?1 AND decision IS NULL ORDER BY created_at, id",
        )?;
        let rows = st.query_map([pane_id], gate_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_gate(&self, id: &str) -> AppResult<Gate> {
        let r = self
            .c()
            .query_row(
                "SELECT id, mission_id, pane_id, title, summary, options_json, decision, note, created_at, decided_at FROM gates WHERE id = ?1",
                [id],
                gate_from,
            )
            .optional()?;
        r.map_or_else(|| fail("not_found", "gate não encontrado"), Ok)
    }

    /// Undecided gates, oldest first. `workspace_id = None` returns all of them.
    pub fn open_gates(&self, workspace_id: Option<&str>) -> AppResult<Vec<Gate>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT g.id, g.mission_id, g.pane_id, g.title, g.summary, g.options_json, g.decision, g.note, g.created_at, g.decided_at
             FROM gates g LEFT JOIN missions m ON m.id = g.mission_id
             WHERE g.decision IS NULL AND (?1 IS NULL OR m.workspace_id = ?1) ORDER BY g.created_at, g.id",
        )?;
        let rows = st.query_map([workspace_id], gate_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- decisions
    pub fn insert_decision(&self, d: &Decision) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO decisions(id, mission_id, pane_id, request, task_type, cli, model, effort, agent, decided_by, cost_usd, receipt, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![d.id, d.mission_id, d.pane_id, d.request, d.task_type, d.cli, d.model, d.effort, d.agent, d.decided_by, d.cost_usd, d.receipt, d.created_at],
        )?;
        Ok(())
    }

    /// Decisions created at or after `since_ms`, most recent first.
    pub fn decisions(&self, since_ms: i64, limit: i64) -> AppResult<Vec<Decision>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, mission_id, pane_id, request, task_type, cli, model, effort, agent, decided_by, cost_usd, receipt, created_at
             FROM decisions WHERE created_at >= ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![since_ms, limit], decision_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- celular (mobile.rs): devices keep only the SHA-256 of their token
    /// A new device. `pending`: paired by code, waiting for the desktop's approval before its token works.
    pub fn mobile_device_add(&self, id: &str, name: &str, token_hash: &str, now: i64, pending: bool) -> AppResult<()> {
        self.c().execute("INSERT INTO mobile_devices(id, name, token_hash, created_at, last_seen, revoked, pending) VALUES (?1,?2,?3,?4,NULL,0,?5)", params![id, name, token_hash, now, pending])?;
        Ok(())
    }

    pub fn mobile_devices(&self) -> AppResult<Vec<MobileDevice>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, name, created_at, last_seen, pending FROM mobile_devices WHERE revoked = 0 ORDER BY created_at, id")?;
        let rows = st
            .query_map([], |r| Ok(MobileDevice { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, last_seen: r.get(3)?, pending: r.get(4)? }))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (id, token hash, pending) of every device that is not revoked: the bearer token is compared against each one.
    pub fn mobile_active_hashes(&self) -> AppResult<Vec<(String, String, bool)>> {
        let c = self.c();
        let mut st = c.prepare("SELECT id, token_hash, pending FROM mobile_devices WHERE revoked = 0")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Whether the device still works: not revoked and approved.
    pub fn mobile_device_ok(&self, id: &str) -> AppResult<bool> {
        let n: i64 = self.c().query_row("SELECT COUNT(*) FROM mobile_devices WHERE id = ?1 AND revoked = 0 AND pending = 0", [id], |r| r.get(0))?;
        Ok(n == 1)
    }

    pub fn mobile_approve(&self, id: &str) -> AppResult<()> {
        self.c().execute("UPDATE mobile_devices SET pending = 0 WHERE id = ?1 AND revoked = 0", [id])?;
        Ok(())
    }

    pub fn mobile_touch(&self, id: &str, now: i64) -> AppResult<()> {
        self.c().execute("UPDATE mobile_devices SET last_seen = ?2 WHERE id = ?1", params![id, now])?;
        Ok(())
    }

    /// Revokes one device, or every device with `"all"`.
    pub fn mobile_revoke(&self, id: &str) -> AppResult<()> {
        let c = self.c();
        if id == "all" {
            c.execute("UPDATE mobile_devices SET revoked = 1", [])?;
        } else {
            c.execute("UPDATE mobile_devices SET revoked = 1 WHERE id = ?1", [id])?;
        }
        Ok(())
    }

    // ---------- cron
    pub fn upsert_cron(&self, j: &CronJob) -> AppResult<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO cron_jobs(id, workspace_id, name, expr, prompt, target, spawn_json, enabled, last_run, next_run, last_error, last_pane)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![j.id, j.workspace_id, j.name, j.expr, j.prompt, j.target, j.spawn_json, j.enabled, j.last_run, j.next_run, j.last_error, j.last_pane],
        )?;
        Ok(())
    }

    pub fn cron_jobs(&self, workspace_id: Option<&str>) -> AppResult<Vec<CronJob>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, workspace_id, name, expr, prompt, target, spawn_json, enabled, last_run, next_run, last_error, last_pane FROM cron_jobs
             WHERE (?1 IS NULL OR workspace_id = ?1) ORDER BY name, id",
        )?;
        let rows = st.query_map([workspace_id], cron_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_cron(&self, id: &str) -> AppResult<()> {
        self.c().execute("DELETE FROM cron_jobs WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The pane a spawn run just opened: the next run writes into it while it is usable.
    pub fn set_cron_last_pane(&self, id: &str, pane: &str) -> AppResult<()> {
        self.c().execute("UPDATE cron_jobs SET last_pane = ?2 WHERE id = ?1", params![id, pane])?;
        Ok(())
    }

    /// Records a run: only the run columns change, so a job deleted meanwhile is not written back.
    pub fn update_cron_run(&self, id: &str, last_run: i64, last_error: Option<&str>, next_run: Option<i64>) -> AppResult<()> {
        self.c().execute("UPDATE cron_jobs SET last_run = ?2, last_error = ?3, next_run = ?4 WHERE id = ?1", params![id, last_run, last_error, next_run])?;
        Ok(())
    }

    // ---------- helper sessions
    pub fn put_helper_session(&self, h: &HelperSession) -> AppResult<()> {
        self.c().execute(
            "INSERT OR REPLACE INTO helper_sessions(task_type, mission_id, pane_id, session_id, cli, closed_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![h.task_type, h.mission_id, h.pane_id, h.session_id, h.cli, h.closed_at],
        )?;
        Ok(())
    }

    pub fn helper_session(&self, task_type: &str, mission_id: &str) -> AppResult<Option<HelperSession>> {
        Ok(self
            .c()
            .query_row(
                "SELECT task_type, mission_id, pane_id, session_id, cli, closed_at FROM helper_sessions WHERE task_type = ?1 AND mission_id = ?2",
                [task_type, mission_id],
                |r| {
                    Ok(HelperSession { task_type: r.get(0)?, mission_id: r.get(1)?, pane_id: r.get(2)?, session_id: r.get(3)?, cli: r.get(4)?, closed_at: r.get(5)? })
                },
            )
            .optional()?)
    }

    /// Removes and returns the helper session of (task_type, mission) in one statement, so two resumes never get the same one.
    pub fn take_helper_session(&self, task_type: &str, mission_id: &str) -> AppResult<Option<HelperSession>> {
        Ok(self
            .c()
            .query_row(
                "DELETE FROM helper_sessions WHERE task_type = ?1 AND mission_id = ?2 RETURNING task_type, mission_id, pane_id, session_id, cli, closed_at",
                [task_type, mission_id],
                |r| Ok(HelperSession { task_type: r.get(0)?, mission_id: r.get(1)?, pane_id: r.get(2)?, session_id: r.get(3)?, cli: r.get(4)?, closed_at: r.get(5)? }),
            )
            .optional()?)
    }

    /// Decisions of one mission, most recent first.
    pub fn mission_decisions(&self, mission_id: &str, limit: i64) -> AppResult<Vec<Decision>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT id, mission_id, pane_id, request, task_type, cli, model, effort, agent, decided_by, cost_usd, receipt, created_at
             FROM decisions WHERE mission_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![mission_id, limit], decision_from)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---------- usage
    #[allow(clippy::too_many_arguments)]
    pub fn add_usage(&self, day: &str, cli: &str, model: &str, effort: &str, d_in: i64, d_out: i64, d_cost: f64, d_terms: i64) -> AppResult<()> {
        self.c().execute(
            "INSERT INTO usage_daily(day, cli, model, effort, tok_in, tok_out, cost_usd, terminals_opened)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(day, cli, model, effort) DO UPDATE SET tok_in = tok_in + excluded.tok_in,
               tok_out = tok_out + excluded.tok_out, cost_usd = cost_usd + excluded.cost_usd,
               terminals_opened = terminals_opened + excluded.terminals_opened",
            params![day, cli, model, effort, d_in, d_out, d_cost, d_terms],
        )?;
        Ok(())
    }

    pub fn usage_rows(&self, since_day: Option<&str>) -> AppResult<Vec<UsageRow>> {
        let c = self.c();
        let mut st = c.prepare("SELECT day, cli, model, effort, tok_in, tok_out, cost_usd, terminals_opened FROM usage_daily WHERE day >= ?1 ORDER BY day")?;
        let rows = st
            .query_map([since_day.unwrap_or("0000-00-00")], |r| {
                Ok(UsageRow { day: r.get(0)?, cli: r.get(1)?, model: r.get(2)?, effort: r.get(3)?, tok_in: r.get(4)?, tok_out: r.get(5)?, cost_usd: r.get(6)?, terminals_opened: r.get(7)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// (total tokens, total cost) across all history, for the top bar.
    pub fn totals(&self) -> AppResult<(i64, f64)> {
        Ok(self.c().query_row("SELECT COALESCE(SUM(tok_in + tok_out), 0), COALESCE(SUM(cost_usd), 0) FROM usage_daily", [], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(ws: &str, id: &str) -> PaneRow {
        PaneRow { id: id.into(), workspace_id: ws.into(), kind: "agent".into(), name: id.into(), permission: "pend".into(), cwd: "C:\\x".into(), status: "iniciando".into(), created_at: 1, ..Default::default() }
    }

    #[test]
    fn v15_database_gains_profile_columns_and_keeps_rows() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("v15.db");
        {
            let c = Connection::open(&path).unwrap();
            for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15] {
                c.execute_batch(s).unwrap();
            }
            c.pragma_update(None, "user_version", 15).unwrap();
            c.execute("INSERT INTO workspaces(id, path, name, last_opened_at) VALUES ('w', 'C:\\x', 'x', 1)", []).unwrap();
            c.execute("INSERT INTO panes(id, workspace_id, kind, name, ord, permission, cwd, status, created_at) VALUES ('p','w','agent','p',0,'pend','C:/','ocioso',1)", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let row = db.get_pane("p").unwrap();
        assert_eq!((row.workspace_id.as_str(), row.profile.as_deref()), ("w", None));
        db.metric_bump("p", "turns").unwrap();
    }

    #[test]
    fn v18_database_gains_memory_state_columns_and_keeps_rows() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("v16.db");
        {
            let c = Connection::open(&path).unwrap();
            for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
                c.execute_batch(s).unwrap();
            }
            c.pragma_update(None, "user_version", 16).unwrap();
            c.execute("INSERT INTO memories(workspace_id, kind, text, created_at) VALUES ('w', 'fato', 'usa pnpm', 1)", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let v: i64 = db.c().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
        let m = &db.memories("w", None, 10).unwrap()[0];
        assert_eq!((m.text.as_str(), m.status.as_str(), m.pinned, m.uses, m.last_used_at), ("usa pnpm", "ativa", false, 0, None));
        // a segunda abertura não tenta criar as colunas de novo
        drop(db);
        assert!(Db::open(&path).is_ok());
    }

    #[test]
    fn a_memory_column_added_by_another_migration_is_not_added_twice() {
        // banco que já tem `pinned` (outra ordem de migração) e está em 16: a v19 só cria o que falta
        let c = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c.execute_batch(s).unwrap();
        }
        c.execute_batch("ALTER TABLE memories ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;").unwrap();
        c.pragma_update(None, "user_version", 16).unwrap();
        Db::migrate(&c).unwrap();
        assert!(has_column(&c, "memories", "status").unwrap());
        assert!(has_column(&c, "memories", "pinned").unwrap());
    }

    #[test]
    fn a_database_numbered_past_19_without_the_memory_columns_gets_them() {
        // outra trilha levou o banco a uma versão maior sem as colunas da memória: a v19 roda pela checagem, sem rebaixar
        let c = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c.execute_batch(s).unwrap();
        }
        c.pragma_update(None, "user_version", 20).unwrap();
        Db::migrate(&c).unwrap();
        assert!(has_column(&c, "memories", "status").unwrap() && has_column(&c, "memories", "last_used_at").unwrap());
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24, "não rebaixa");
    }

    #[test]
    fn v19_database_without_waste_tables_gets_them() {
        // banco já em 19 mas sem as tabelas da v17 (ex.: migração fora de ordem): o migrate recria, sem baixar a versão
        let c = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c.execute_batch(s).unwrap();
        }
        c.pragma_update(None, "user_version", 16).unwrap();
        Db::migrate(&c).unwrap();
        c.execute_batch("DROP TABLE waste_findings; DROP TABLE waste_offsets;").unwrap();
        Db::migrate(&c).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('waste_findings', 'waste_offsets')", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
    }

    #[test]
    fn v19_database_without_memory_vectors_gets_the_table_at_21() {
        let c = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c.execute_batch(s).unwrap();
        }
        c.pragma_update(None, "user_version", 19).unwrap();
        Db::migrate(&c).unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
        assert!(c.query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get::<_, i64>(0)).is_ok());
        c.execute_batch("DROP TABLE memory_vectors;").unwrap();
        Db::migrate(&c).unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24, "a versão nunca desce");
        assert!(c.query_row("SELECT COUNT(*) FROM memory_vectors", [], |r| r.get::<_, i64>(0)).is_ok());
    }

    #[test]
    fn v16_database_gets_waste_tables_and_keeps_version_max() {
        let c = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c.execute_batch(s).unwrap();
        }
        c.pragma_update(None, "user_version", 16).unwrap();
        Db::migrate(&c).unwrap();
        // a v17 já existente (tabelas criadas) não dá erro ao rodar de novo
        Db::migrate(&c).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('waste_findings', 'waste_offsets')", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
    }

    #[test]
    fn subagentes_trabalhando_lista_pai_transcrito_e_criacao() {
        let db = Db::open_in_memory().unwrap();
        for (id, status, fechado) in [("a", "trabalhando", false), ("b", "concluido", false), ("c", "trabalhando", true)] {
            let mut p = pane("ws1", id);
            p.kind = "subagent".into();
            p.parent_id = Some("pai".into());
            p.url_or_path = Some("C:/t/a.jsonl".into());
            p.status = status.into();
            db.insert_pane(&p).unwrap();
            if fechado {
                db.close_pane(id).unwrap();
            }
        }
        let v = db.subagentes_trabalhando().unwrap();
        assert_eq!(v, vec![("pai".to_string(), Some("C:/t/a.jsonl".to_string()), 1)]);
    }

    #[test]
    fn subagente_trabalhando_nao_volta_no_boot() {
        let db = Db::open_in_memory().unwrap();
        let mut p = pane("ws1", "sub");
        p.kind = "subagent".into();
        p.parent_id = Some("pai".into());
        p.url_or_path = Some("C:/t/a.jsonl".into());
        p.status = "trabalhando".into();
        db.insert_pane(&p).unwrap();
        assert_eq!(db.subagentes_trabalhando().unwrap().len(), 1);
        // o boot fecha subagentes (não são processos): nenhum fica "trabalhando" depois da sessão anterior
        db.close_dead_panes().unwrap();
        assert!(db.subagentes_trabalhando().unwrap().is_empty());
    }

    #[test]
    fn execution_panes_never_come_back_at_boot() {
        let db = Db::open_in_memory().unwrap();
        for (id, kind) in [("ex", "exec"), ("lista", "execucoes"), ("agente", "agent")] {
            let mut p = pane("ws1", id);
            p.kind = kind.into();
            p.status = "trabalhando".into();
            db.insert_pane(&p).unwrap();
        }
        db.close_dead_panes().unwrap();
        assert!(db.get_pane("ex").unwrap().closed_at.is_some());
        assert!(db.get_pane("lista").unwrap().closed_at.is_some());
        assert!(db.get_pane("agente").unwrap().closed_at.is_none(), "agente em curso volta a ser restaurável, não é fechado aqui");
    }

    #[test]
    fn prune_metrics_drops_only_panes_closed_long_ago() {
        let db = Db::open_in_memory().unwrap();
        for id in ["old", "new", "open"] {
            db.insert_pane(&pane("ws1", id)).unwrap();
            db.metric_bump(id, "turns").unwrap();
        }
        db.close_pane("old").unwrap();
        db.close_pane("new").unwrap();
        db.c().execute("UPDATE panes SET closed_at = 1 WHERE id = 'old'", []).unwrap();
        let before = crate::util::now_ms() - 30 * 86_400_000;
        assert_eq!(db.prune_metrics(before).unwrap(), 1);
        assert!(db.pane_metrics("old").unwrap().is_none());
        assert!(db.pane_metrics("new").unwrap().is_some(), "fechado há pouco: fica");
        assert!(db.pane_metrics("open").unwrap().is_some(), "aberto: fica");
    }

    #[test]
    fn metrics_accept_only_known_fields() {
        let db = Db::open_in_memory().unwrap();
        db.metric_bump("p", "turns").unwrap();
        db.metric_bump("p", "turns").unwrap();
        db.metric_set("p", "first_turn_tokens", 1234).unwrap();
        let m = db.pane_metrics("p").unwrap().unwrap();
        assert_eq!((m.turns, m.first_turn_tokens), (2, Some(1234)));
        assert!(db.metric_bump("p", "turns; DROP TABLE panes").is_err());
        assert!(db.metric_set("p", "nope", 1).is_err());
        assert_eq!(db.pane_metrics("ninguem").unwrap(), None);
    }

    #[test]
    fn resumable_lists_only_closed_agents_with_a_session() {
        let db = Db::open_in_memory().unwrap();
        let mut a = pane("ws1", "a");
        a.session_id = Some("s-a".into());
        db.insert_pane(&a).unwrap();
        db.close_pane("a").unwrap();
        // open (still running or hibernated): not a closed conversation
        let mut b = pane("ws1", "b");
        b.session_id = Some("s-b".into());
        db.insert_pane(&b).unwrap();
        // closed but with no session to resume
        db.insert_pane(&pane("ws1", "c")).unwrap();
        db.close_pane("c").unwrap();
        let ids: Vec<String> = db.resumable_panes("ws1").unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn migrate_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        Db::migrate(&db.c()).unwrap();
    }

    #[test]
    fn v18_mission_columns_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let ws = db.upsert_workspace("C:\\proj").unwrap();
        db.insert_mission(&Mission { id: "m".into(), workspace_id: ws.id.clone(), name: "x".into(), mode: "livre".into(), status: "ativa".into(), created_at: 1, ..Default::default() }).unwrap();
        let m = db.get_mission("m").unwrap();
        assert_eq!((m.plan_json, m.verify_json), (None, None));
        db.set_mission_verify("m", r#"{"comando":"cargo test"}"#).unwrap();
        db.set_mission_plan("m", r#"{"aprovada":1}"#).unwrap();
        let m = db.get_mission("m").unwrap();
        assert_eq!(m.verify_json.as_deref(), Some(r#"{"comando":"cargo test"}"#));
        assert_eq!(m.plan_json.as_deref(), Some(r#"{"aprovada":1}"#));
    }

    #[test]
    fn v20_table_arrives_even_when_user_version_is_higher() {
        let db = Db::open_in_memory().unwrap();
        let c = db.c();
        c.execute_batch("DROP TABLE mission_phase_events;").unwrap();
        c.pragma_update(None, "user_version", 21).unwrap();
        Db::migrate(&c).unwrap();
        assert!(Db::tem_tabela(&c, "mission_phase_events").unwrap());
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24, "o user_version não regride");
        // e a migração é idempotente: rodar de novo não apaga linhas
        drop(c);
        db.fase_abrir("m", None, None, "planejamento", 1).unwrap();
        Db::migrate(&db.c()).unwrap();
        assert_eq!(db.fases_da_missao("m").unwrap().len(), 1);
    }

    #[test]
    fn fase_aberta_fecha_uma_vez_e_some_da_lista_de_abertas() {
        let db = Db::open_in_memory().unwrap();
        let id = db.fase_abrir("m", Some("p"), Some("m-A"), "execucao", 100).unwrap();
        assert_eq!(db.fase_aberta("m", Some("m-A"), "execucao").unwrap(), Some(id));
        assert_eq!(db.fase_aberta("m", None, "execucao").unwrap(), None, "tarefa diferente não casa");
        db.fase_fechar(id, 160, 10, 20, 0.5).unwrap();
        db.fase_fechar(id, 999, 0, 0, 0.0).unwrap(); // segunda vez: não mexe
        assert_eq!(db.fase_aberta("m", Some("m-A"), "execucao").unwrap(), None);
        let f = &db.fases_da_missao("m").unwrap()[0];
        assert_eq!((f.started_at, f.ended_at, f.tok_in, f.tok_out), (100, Some(160), 10, 20));
    }

    #[test]
    fn fases_orfas_fecham_no_boot_e_missao_sumida_perde_suas_fases() {
        let db = Db::open_in_memory().unwrap();
        db.fase_abrir("m-sumida", None, None, "planejamento", 100).unwrap();
        db.fase_abrir("m-aberta", None, None, "execucao", 200).unwrap();
        assert_eq!(db.fechar_fases_orfas().unwrap(), 2);
        assert!(db.fases_da_missao("m-aberta").unwrap()[0].ended_at == Some(200), "fecha no início, sem duração inventada");
        // m-sumida não está em `missions`: some; m-aberta também não existe lá, então sai junto
        assert_eq!(db.prune_fases_orfas().unwrap(), 2);
        assert!(db.fases_da_missao("m-sumida").unwrap().is_empty());
    }

    #[test]
    fn v18_columns_arrive_even_when_user_version_is_already_19() {
        let db = Db::open_in_memory().unwrap();
        let c = db.c();
        c.execute_batch("ALTER TABLE missions DROP COLUMN verify_json;").unwrap();
        c.pragma_update(None, "user_version", 19).unwrap();
        Db::migrate(&c).unwrap();
        assert!(Db::tem_coluna(&c, "missions", "verify_json").unwrap());
        assert!(Db::tem_coluna(&c, "missions", "plan_json").unwrap());
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24, "o user_version sobe até a última versão e não regride");
    }

    #[test]
    fn schema_v9_has_mission_indexes() {
        let db = Db::open_in_memory().unwrap();
        let v: i64 = db.c().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
        let n: i64 = db
            .c()
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name IN ('idx_panes_mission', 'idx_handoffs_mission', 'idx_gates_mission')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn resumable_lists_one_closed_row_per_session_and_never_an_open_one() {
        let db = Db::open_in_memory().unwrap();
        let pane = |id: &str, session: &str, closed: Option<i64>| PaneRow {
            id: id.into(), workspace_id: "ws".into(), kind: "agent".into(), name: id.into(), cwd: "C:/".into(), status: "ocioso".into(),
            session_id: Some(session.into()), closed_at: closed, ..Default::default()
        };
        db.insert_pane(&pane("antiga", "s1", Some(1))).unwrap();
        db.insert_pane(&pane("nova", "s1", Some(2))).unwrap();
        db.insert_pane(&pane("aberta", "s2", None)).unwrap();
        db.insert_pane(&pane("fechada-s2", "s2", Some(3))).unwrap();
        let ids: Vec<String> = db.resumable_panes("ws").unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["nova"], "s1 uma vez (a mais nova); s2 fora porque já está aberta");
        assert!(db.session_is_open("s2").unwrap());
        assert!(!db.session_is_open("s1").unwrap());
        assert_eq!(db.latest_session_pane("ws", "s1").unwrap().unwrap().id, "nova");
    }

    #[test]
    fn schema_v13_indexes_handoffs_by_pane() {
        let db = Db::open_in_memory().unwrap();
        let n: i64 = db
            .c()
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_handoffs_from'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn upsert_workspace_keeps_id() {
        let db = Db::open_in_memory().unwrap();
        let a = db.upsert_workspace("D:\\proj\\site").unwrap();
        let b = db.upsert_workspace("D:\\proj\\site").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, "site");
        assert_eq!(db.list_workspaces().unwrap().len(), 1);
    }

    #[test]
    fn panes_open_close_and_order() {
        let db = Db::open_in_memory().unwrap();
        db.insert_pane(&pane("w", "a")).unwrap();
        let mut b = pane("w", "b");
        b.ord = db.next_ord("w").unwrap();
        assert_eq!(b.ord, 1);
        db.insert_pane(&b).unwrap();
        db.set_pane_order(&["b".into(), "a".into()]).unwrap();
        let ids: Vec<_> = db.open_panes("w").unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["b", "a"]);
        db.close_pane("a").unwrap();
        assert_eq!(db.open_panes("w").unwrap().len(), 1);
    }

    #[test]
    fn execution_panes_are_never_restorable() {
        let db = Db::open_in_memory().unwrap();
        for kind in ["exec", "execucoes", "subagent"] {
            let mut p = pane("w", kind);
            p.kind = kind.into();
            db.insert_pane(&p).unwrap();
        }
        db.mark_all_restorable().unwrap();
        assert!(db.restorable_panes("w").unwrap().is_empty());
    }

    #[test]
    fn restorable_flow() {
        let db = Db::open_in_memory().unwrap();
        db.insert_pane(&pane("w", "a")).unwrap();
        db.mark_all_restorable().unwrap();
        assert!(db.open_panes("w").unwrap().is_empty());
        assert_eq!(db.restorable_panes("w").unwrap().len(), 1);
        db.resolve_restorable("w", false).unwrap();
        assert!(db.restorable_panes("w").unwrap().is_empty());
        assert!(db.open_panes("w").unwrap().is_empty());
    }

    #[test]
    fn workspace_path_is_normalized() {
        let db = Db::open_in_memory().unwrap();
        let a = db.upsert_workspace("D:/proj/site/").unwrap();
        let b = db.upsert_workspace("d:\\proj\\site").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.name, "site");
        assert_eq!(normalize_ws_path("C:\\"), "C:\\");
    }

    #[test]
    fn restorable_keep_brings_pane_back_and_browser_never_restorable() {
        let db = Db::open_in_memory().unwrap();
        db.insert_pane(&pane("w", "a")).unwrap();
        let mut b = pane("w", "b");
        b.kind = "browser".into();
        db.insert_pane(&b).unwrap();
        db.mark_all_restorable().unwrap();
        let r: Vec<_> = db.restorable_panes("w").unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(r, vec!["a"]);
        db.resolve_restorable("w", true).unwrap();
        assert_eq!(db.open_panes("w").unwrap().len(), 2);
        assert!(db.restorable_panes("w").unwrap().is_empty());
    }

    #[test]
    fn usage_rows_since_excludes_earlier_day() {
        let db = Db::open_in_memory().unwrap();
        db.add_usage("2026-10-13", "claude", "opus", "high", 1, 1, 0.1, 0).unwrap();
        db.add_usage("2026-10-14", "claude", "opus", "high", 1, 1, 0.1, 0).unwrap();
        let rows = db.usage_rows(Some("2026-10-14")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].day, "2026-10-14");
    }

    #[test]
    fn remove_workspace_closes_its_panes() {
        let db = Db::open_in_memory().unwrap();
        let ws = db.upsert_workspace("D:\\proj\\site").unwrap();
        db.insert_pane(&pane(&ws.id, "a")).unwrap();
        db.remove_workspace(&ws.id).unwrap();
        assert!(db.open_panes(&ws.id).unwrap().is_empty());
        assert!(db.get_workspace(&ws.id).is_err());
    }

    #[test]
    fn frame_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.insert_pane(&pane("w", "a")).unwrap();
        db.set_frame("a", Some("tela")).unwrap();
        assert_eq!(db.get_frame("a").unwrap().as_deref(), Some("tela"));
    }

    #[test]
    fn usage_accumulates() {
        let db = Db::open_in_memory().unwrap();
        db.add_usage("2026-10-14", "claude", "opus", "high", 10, 5, 0.5, 1).unwrap();
        db.add_usage("2026-10-14", "claude", "opus", "high", 1, 1, 0.25, 0).unwrap();
        let r = &db.usage_rows(None).unwrap()[0];
        assert_eq!((r.tok_in, r.tok_out, r.terminals_opened), (11, 6, 1));
        assert_eq!(db.totals().unwrap(), (17, 0.75));
    }

    #[test]
    fn missions_crud() {
        let db = Db::open_in_memory().unwrap();
        let mut m = Mission { id: "m".into(), workspace_id: "w".into(), name: "x".into(), mode: "livre".into(), status: "ativa".into(), status_reason: None, worktree_path: None, branch: None, base_branch: None, pinned: false, created_at: 1, archived_at: None, squad_id: None, recipe_id: None, brief: None, style: None, profile: None, plan_json: None, verify_json: None, adopted: false };
        db.insert_mission(&m).unwrap();
        m.status = "bloqueada".into();
        m.status_reason = Some("conflito".into());
        db.update_mission(&m).unwrap();
        assert_eq!(db.get_mission("m").unwrap(), m);
        assert_eq!(db.list_missions("w").unwrap().len(), 1);
    }

    #[test]
    fn mission_totals_include_closed_panes() {
        let db = Db::open_in_memory().unwrap();
        let mut a = pane("w", "a");
        a.mission_id = Some("m".into());
        a.tok_in = 10;
        a.tok_out = 5;
        a.cost_usd = 0.5;
        db.insert_pane(&a).unwrap();
        let mut b = pane("w", "b");
        b.mission_id = Some("m".into());
        b.tok_cache_r = 100;
        b.cost_usd = 0.25;
        db.insert_pane(&b).unwrap();
        db.close_pane("b").unwrap();
        assert_eq!(db.mission_totals("w").unwrap(), vec![("m".to_string(), 15, 100, 0.75)]);
    }

    #[test]
    fn rebase_keeps_previous_session_in_mission_totals() {
        let db = Db::open_in_memory().unwrap();
        let mut a = pane("w", "a");
        a.mission_id = Some("m".into());
        a.tok_in = 10;
        a.tok_cache_w = 90;
        a.cost_usd = 0.5;
        db.insert_pane(&a).unwrap();
        db.rebase_pane_usage("a").unwrap();
        let p = db.get_pane("a").unwrap();
        assert_eq!((p.base_tok_in, p.base_tok_cache_w, p.base_cost_usd, p.tok_in), (10, 90, 0.5, 10), "base = gasto até aqui, total intacto");
        // a sessão nova cresce o total: a base não some
        db.set_pane_usage("a", 12, 3, 0, 100, 0.9).unwrap();
        assert_eq!(db.mission_totals("w").unwrap(), vec![("m".to_string(), 15, 100, 0.9)]);
    }

    fn handoff(id: &str, mission: &str, to: &str, at: i64) -> Handoff {
        Handoff { id: id.into(), mission_id: Some(mission.into()), from_pane: "a".into(), to_pane: Some(to.into()), summary: "s".into(), data_json: None, files_json: None, created_at: at, seen: false }
    }

    fn memory(ws: &str, text: &str) -> Memory {
        Memory { workspace_id: ws.into(), kind: "nota".into(), text: text.into(), created_at: 1, ..Default::default() }
    }

    #[test]
    fn migrate_v1_to_v2_keeps_data() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("regente.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA_V1).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
            c.execute(
                "INSERT INTO panes(id, workspace_id, kind, name, ord, permission, cwd, status, created_at) VALUES ('p','w','agent','p',0,'pend','C:/','ocioso',1)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let row = db.get_pane("p").unwrap();
        assert_eq!(row.mcp_profile, "full");
        assert!(!row.name_manual);
        let v: i64 = db.c().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
    }

    #[test]
    fn migrate_v2_to_v3_keeps_data() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("regente.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA_V1).unwrap();
            c.execute_batch(SCHEMA_V2).unwrap();
            c.pragma_update(None, "user_version", 2).unwrap();
            c.execute("INSERT INTO panes(id, workspace_id, kind, name, ord, permission, cwd, status, created_at) VALUES ('p','w','agent','p',0,'pend','C:/','ocioso',1)", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let row = db.get_pane("p").unwrap();
        assert_eq!(row.mcp_profile, "full");
        assert_eq!(row.append_system, None);
        assert_eq!(row.disallowed_tools, None);
        let v: i64 = db.c().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
    }

    #[test]
    fn pane_groups_persist_per_workspace_and_release_panes_on_delete() {
        let db = Db::open_in_memory().unwrap();
        db.insert_pane(&pane("w1", "p1")).unwrap();
        db.insert_pane(&pane("w2", "p2")).unwrap();
        let g = db.create_pane_group("w1", "front").unwrap();
        db.update_pane_group(&g.id, Some("frontend"), Some(true)).unwrap();
        db.set_pane_group("p1", Some(&g.id)).unwrap();
        // grupo de outro workspace não aceita o painel
        assert!(db.set_pane_group("p2", Some(&g.id)).is_err());
        let groups = db.list_pane_groups("w1").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!((groups[0].name.as_str(), groups[0].collapsed), ("frontend", true));
        assert!(db.list_pane_groups("w2").unwrap().is_empty());
        assert_eq!(db.get_pane("p1").unwrap().group_id.as_deref(), Some(g.id.as_str()));
        db.delete_pane_group(&g.id).unwrap();
        assert!(db.list_pane_groups("w1").unwrap().is_empty());
        assert_eq!(db.get_pane("p1").unwrap().group_id, None);
        assert!(db.update_pane_group("nope", Some("x"), None).is_err());
    }

    #[test]
    fn pane_group_names_are_trimmed_and_bounded() {
        assert_eq!(clean_group_name("  Back  ").unwrap(), "Back");
        assert!(clean_group_name("   ").is_err());
        assert!(clean_group_name(&"x".repeat(41)).is_err());
        assert!(clean_group_name(&"x".repeat(40)).is_ok());
    }

    #[test]
    fn media_attempts_count_by_kind_window_and_mission() {
        let db = Db::open_in_memory().unwrap();
        db.add_media_attempt("image", Some("m1"), 10).unwrap();
        db.add_media_attempt("image", Some("m2"), 20).unwrap();
        db.add_media_attempt("video", Some("m1"), 30).unwrap();
        assert_eq!(db.media_attempts("image", 0, None).unwrap(), 2);
        assert_eq!(db.media_attempts("image", 15, None).unwrap(), 1);
        assert_eq!(db.media_attempts("image", 0, Some("m1")).unwrap(), 1);
        assert_eq!(db.media_attempts("audio", 0, None).unwrap(), 0);
    }

    #[test]
    fn voice_history_roundtrip_and_clear() {
        let db = Db::open_in_memory().unwrap();
        db.add_voice_history(1, "chat", "primeiro").unwrap();
        db.add_voice_history(2, "code", "segundo").unwrap();
        let rows = db.voice_history(10).unwrap();
        assert_eq!(rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["segundo", "primeiro"]);
        assert_eq!(rows[0].mode, "code");
        db.clear_voice_history().unwrap();
        assert!(db.voice_history(10).unwrap().is_empty());
    }

    #[test]
    fn handoff_roundtrip_and_seen() {
        let db = Db::open_in_memory().unwrap();
        db.insert_mission(&Mission { id: "m".into(), workspace_id: "ws".into(), name: "x".into(), mode: "livre".into(), status: "ativa".into(), status_reason: None, worktree_path: None, branch: None, base_branch: None, pinned: false, created_at: 1, archived_at: None, squad_id: None, recipe_id: None, brief: None, style: None, profile: None, plan_json: None, verify_json: None, adopted: false }).unwrap();
        db.insert_handoff(&handoff("h1", "m", "b", 1)).unwrap();
        db.insert_handoff(&handoff("h2", "m", "b", 2)).unwrap();
        let ids: Vec<_> = db.handoffs(Some("m"), None, 10).unwrap().into_iter().map(|h| h.id).collect();
        assert_eq!(ids, vec!["h2", "h1"]);
        assert_eq!(db.get_handoff("h1").unwrap().summary, "s");
        assert_eq!(db.unseen_handoffs("ws").unwrap(), vec![("m".to_string(), 2)]);
        db.mark_handoffs_seen("m").unwrap();
        assert!(db.unseen_handoffs("ws").unwrap().is_empty());
    }

    #[test]
    fn handoffs_are_scoped_to_the_mission_newest_first() {
        let db = Db::open_in_memory().unwrap();
        for id in ["m", "outra"] {
            db.insert_mission(&Mission { id: id.into(), workspace_id: "ws".into(), name: id.into(), mode: "livre".into(), status: "ativa".into(), status_reason: None, worktree_path: None, branch: None, base_branch: None, pinned: false, created_at: 1, archived_at: None, squad_id: None, recipe_id: None, brief: None, style: None, profile: None, plan_json: None, verify_json: None, adopted: false }).unwrap();
        }
        db.insert_handoff(&handoff("h1", "m", "b", 1)).unwrap();
        db.insert_handoff(&handoff("h2", "m", "b", 3)).unwrap();
        db.insert_handoff(&handoff("x", "outra", "b", 2)).unwrap();
        let ids: Vec<_> = db.handoffs(Some("m"), None, 50).unwrap().into_iter().map(|h| h.id).collect();
        assert_eq!(ids, vec!["h2", "h1"], "só da missão, da mais nova");
    }

    #[test]
    fn tasks_crud() {
        let db = Db::open_in_memory().unwrap();
        let mut t = TaskItem { id: "t".into(), mission_id: "m".into(), title: "x".into(), status: "todo".into(), owner_pane: None, created_at: 1, updated_at: 1 };
        db.upsert_task(&t).unwrap();
        t.status = "feito".into();
        db.upsert_task(&t).unwrap();
        assert_eq!(db.tasks("m").unwrap(), vec![t.clone()]);
        db.delete_task("t").unwrap();
        assert!(db.tasks("m").unwrap().is_empty());
    }

    #[test]
    fn memory_fts() {
        let db = Db::open_in_memory().unwrap();
        let a = db.insert_memory(&memory("w", "deploy usa vercel")).unwrap();
        db.insert_memory(&memory("w", "banco é postgres")).unwrap();
        assert_eq!(db.search_memories("w", "postgres", 10).unwrap().len(), 1);
        assert!(db.update_memory(2, "banco é mysql").unwrap());
        assert!(!db.update_memory(99, "fantasma").unwrap(), "unknown id is reported, not silently accepted");
        assert_eq!(db.search_memories("w", "postgres", 10).unwrap().len(), 0);
        assert_eq!(db.search_memories("w", "mysql", 10).unwrap().len(), 1);
        db.delete_memory(2).unwrap();
        assert_eq!(db.search_memories("w", "mysql", 10).unwrap().len(), 0);
        assert_eq!(db.memories("w", None, 10).unwrap().len(), 1);
        assert_eq!(a, 1);
    }

    #[test]
    fn recent_missions_orders_by_last_use_and_counts_pending_work() {
        let db = Db::open_in_memory().unwrap();
        let ws = db.upsert_workspace("C:\\proj").unwrap();
        let mission = |id: &str, created: i64| Mission {
            id: id.into(), workspace_id: ws.id.clone(), name: id.into(), mode: "livre".into(), status: "ativa".into(), status_reason: None,
            worktree_path: None, branch: None, base_branch: None, pinned: false, created_at: created, archived_at: None,
            squad_id: None, recipe_id: None, brief: None, style: None, profile: None, plan_json: None, verify_json: None, adopted: false,
        };
        db.insert_mission(&mission("velha", 100)).unwrap();
        db.insert_mission(&mission("nova", 200)).unwrap();
        // um painel da "velha" foi usado depois de tudo: é o uso mais recente
        db.insert_pane(&PaneRow { id: "p1".into(), workspace_id: ws.id.clone(), mission_id: Some("velha".into()), kind: "agent".into(), name: "p1".into(), cwd: "C:\\proj".into(), status: "ocioso".into(), created_at: 900, cost_usd: 1.5, ..Default::default() }).unwrap();
        db.insert_gate(&Gate { id: "g".into(), mission_id: Some("velha".into()), pane_id: "p1".into(), title: "t".into(), summary: "s".into(), options_json: None, decision: None, note: None, created_at: 901, decided_at: None }).unwrap();
        let rows = db.recent_missions(10).unwrap();
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["velha", "nova"]);
        assert_eq!(rows[0].live_panes, 1);
        assert_eq!(rows[0].open_gates, 1);
        assert!((rows[0].cost_usd - 1.5).abs() < 1e-9);
        assert_eq!(rows[0].last_activity, 900);
        assert_eq!(rows[0].workspace_name, "proj");
        assert_eq!(rows[1].live_panes, 0);
        assert_eq!(db.recent_missions(1).unwrap().len(), 1, "limit applies");
        assert!((db.mission_cost("velha").unwrap() - 1.5).abs() < 1e-9);
        assert_eq!(db.mission_cost("nova").unwrap(), 0.0);
    }

    #[test]
    fn gates_open_and_decide() {
        let db = Db::open_in_memory().unwrap();
        db.insert_gate(&Gate { id: "g".into(), mission_id: None, pane_id: "p".into(), title: "t".into(), summary: "s".into(), options_json: None, decision: None, note: None, created_at: 1, decided_at: None }).unwrap();
        assert_eq!(db.open_gates(None).unwrap().len(), 1);
        assert!(db.decide_gate("g", "aprovar", None).unwrap());
        assert!(!db.decide_gate("g", "ajustar", None).unwrap(), "a decided gate is not decided again");
        assert_eq!(db.open_gates(None).unwrap().len(), 0);
    }

    #[test]
    fn boot_cancels_gates_left_open() {
        let db = Db::open_in_memory().unwrap();
        db.insert_gate(&Gate { id: "g".into(), mission_id: None, pane_id: "p".into(), title: "t".into(), summary: "s".into(), options_json: None, decision: None, note: None, created_at: 1, decided_at: None }).unwrap();
        assert_eq!(db.cancel_open_gates().unwrap(), 1);
        assert_eq!(db.get_gate("g").unwrap().decision.as_deref(), Some("cancelado"));
        assert!(db.open_gates(None).unwrap().is_empty());
    }

    #[test]
    fn helper_session_replace() {
        let db = Db::open_in_memory().unwrap();
        let mut h = HelperSession { task_type: "rev".into(), mission_id: "m".into(), pane_id: "p1".into(), session_id: None, cli: Some("claude".into()), closed_at: 1 };
        db.put_helper_session(&h).unwrap();
        h.pane_id = "p2".into();
        db.put_helper_session(&h).unwrap();
        assert_eq!(db.helper_session("rev", "m").unwrap().unwrap().pane_id, "p2");
    }

    #[test]
    fn set_pane_name_respects_manual_names() {
        let db = Db::open_in_memory().unwrap();
        db.insert_pane(&pane("w", "a")).unwrap();
        db.set_pane_name("a", "Manual", true).unwrap();
        db.set_pane_name("a", "Automatico", false).unwrap();
        assert_eq!(db.get_pane("a").unwrap().name, "Manual");
        db.set_pane_name("a", "Outro", true).unwrap();
        assert_eq!(db.get_pane("a").unwrap().name, "Outro");
    }

    /// `prune_ajustes` mantém os mais recentes e apaga o que passou da data.
    #[test]
    fn prune_keeps_the_newest_and_drops_the_old() {
        let db = Db::open_in_memory().unwrap();
        for (i, at) in [(1, 10), (2, 20), (3, 30)] {
            db.insert_ajuste(&Ajuste { id: format!("a{i}"), created_at: at, workspace_id: None, mission_id: None, pane_id: None, kind: "compactacao".into(), acao: "pulado".into(), motivo: "m".into(), antes_json: None, depois_json: None, estimativa_json: None, efeito_json: None, desfazivel: false, desfeito_at: None }).unwrap();
        }
        db.prune_ajustes(2, 15).unwrap();
        let ids: Vec<String> = db.list_ajustes(None, 10).unwrap().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["a3", "a2"]);
        db.prune_ajustes(1, 0).unwrap();
        assert_eq!(db.list_ajustes(None, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_database_without_the_adopted_column_gets_it_without_downgrading() {
        let c = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c.execute_batch(s).unwrap();
        }
        c.pragma_update(None, "user_version", 23).unwrap();
        Db::migrate(&c).unwrap();
        assert!(has_column(&c, "missions", "adopted").unwrap());
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 24);
        // segunda passagem: idempotente, versão igual
        Db::migrate(&c).unwrap();
        let v2: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v2, 24);
        // outra trilha já criou a coluna com o banco em 23: a v24 não tenta criar de novo e só sobe a versão
        let c2 = Connection::open_in_memory().unwrap();
        for s in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15, SCHEMA_V16] {
            c2.execute_batch(s).unwrap();
        }
        c2.execute_batch("ALTER TABLE missions ADD COLUMN adopted INTEGER NOT NULL DEFAULT 0;").unwrap();
        c2.pragma_update(None, "user_version", 23).unwrap();
        Db::migrate(&c2).unwrap();
        let v3: i64 = c2.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v3, 24);
    }

    #[test]
    fn adopted_round_trips_and_the_branch_can_change() {
        let db = Db::open_in_memory().unwrap();
        db.insert_mission(&Mission { id: "m".into(), workspace_id: "w".into(), name: "ambiente01".into(), mode: "livre".into(), status: "ativa".into(), created_at: 1, adopted: true, worktree_path: Some("C:/x/ambiente01".into()), ..Default::default() }).unwrap();
        assert!(db.get_mission("m").unwrap().adopted);
        db.set_mission_branch("m", Some("spec/x")).unwrap();
        assert_eq!(db.get_mission("m").unwrap().branch.as_deref(), Some("spec/x"));
        db.set_mission_branch("m", None).unwrap();
        assert_eq!(db.get_mission("m").unwrap().branch, None);
    }
}
