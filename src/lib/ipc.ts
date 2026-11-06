import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Conector MCP do Regente: vai para todo painel do Claude, em qualquer conta. */
export interface Conector { nome: string; transporte: "http" | "sse" | "stdio"; resumo: string; ativo: boolean }
/** Servidor MCP de uma conta do Claude que ainda não é conector. */
export interface ConectorImportavel { nome: string; transporte: string; conta: string }

export type PaneStatus = "iniciando" | "trabalhando" | "ocioso" | "concluido" | "erro" | "fechando" | "hibernado" | "acordando" | "pergunta";
export type PaneKind = "agent" | "terminal" | "browser" | "image" | "video" | "subagent" | "exec" | "execucoes";
export type Permission = "danger" | "auto" | "pend" | "plan";
/** full: tudo que a IA já tem; minimal: só o Regente; custom: o Regente e os servidores escolhidos (só Claude) */
export type McpProfile = "full" | "minimal" | "custom";
/** skill do Regente nas skills do Claude: instaladas pelo Regente, faltando, e pastas de terceiros com o mesmo nome */
export interface BaseSkillStatus { target: string; installed: string[]; missing: string[]; conflicts: string[] }
/** o que a instalação copiou (novas ou atualizadas) e as skills sem fonte, que ficaram de fora */
export interface BaseSkillInstall { written: string[]; noSource: string[] }
/** memória do painel: auto (missão ou só o workspace, conforme o painel), standalone (só a do workspace) ou off */
export type MemoryMode = "auto" | "standalone" | "off";
/** servidor MCP do usuário no Claude (só nome, origem e transporte) */
export interface McpServerInfo { name: string; source: string; transport: string }
export interface McpListing { supported: boolean; servers: McpServerInfo[] }

export interface CronJob { id: string; workspaceId: string; name: string; expr: string; prompt: string; target: string; spawnJson: string | null; enabled: boolean; lastRun: number | null; nextRun: number | null; lastError: string | null }
/** Issue do GitHub usada no brief de uma missão. */
export interface GhIssue { number: number; title: string; body: string; labels: string[] }
/** Resultado da busca nos painéis (Ctrl+Shift+F): trechos de buffer ("terminal"), de disco ("historico", saída antiga que já saiu do buffer) ou da conversa ("conversa"). */
export interface PaneHit { paneId: string; paneName: string; workspaceId: string; source: "terminal" | "conversa" | "historico"; excerpt: string }
export interface PaneSearch { hits: PaneHit[]; truncated: boolean; diskLimited: boolean }
/** Prompt salvo (Configurações → Prompts); as variáveis {{...}} são expandidas ao colar. */
export interface PromptDef { id: string; name: string; text: string; sendEnter: boolean }
export interface Workspace { id: string; path: string; name: string; lastOpenedAt: number; pinned: boolean }
/** Missão recente (Home → Continuar de onde parei): último uso = painel criado, entrega ou gate mais novo. */
export interface RecentMission {
  id: string; workspaceId: string; workspaceName: string; workspacePath: string; name: string; status: string;
  livePanes: number; costUsd: number; unseen: number; openGates: number; lastActivity: number;
}
export interface Mission {
  id: string; workspaceId: string; name: string; mode: string; status: string; statusReason: string | null;
  worktreePath: string | null; branch: string | null; baseBranch: string | null; pinned: boolean;
  createdAt: number; archivedAt: number | null;
  /** missão adotada: o worktree é do projeto (nunca criado nem removido pelo Regente) */
  adopted: boolean;
  /** perfil de trabalho da missão (id); null = segue o projeto/padrão */
  profile?: string | null;
}
export interface PaneRow {
  id: string; workspaceId: string; missionId: string | null; kind: PaneKind; name: string; ord: number;
  cli: string | null; shell: string | null; model: string | null; account: string | null; permission: Permission;
  effort: string | null; route: string | null; sessionId: string | null; cwd: string; urlOrPath: string | null;
  status: PaneStatus; resident: boolean; createdAt: number; closedAt: number | null;
  tokIn: number; tokOut: number; tokCacheR: number; tokCacheW: number; costUsd: number;
  parentId: string | null; role: string | null; agent: string | null; taskType: string | null; mcpProfile: string;
  allowedSkills: string | null; nameManual: boolean; groupId: string | null; memoryMode: MemoryMode | null;
  /** perfil de trabalho que o painel abriu (id), fixo até fechar */
  profile?: string | null;
}
/** atenção da missão: bloqueada, pronta para integrar e os motivos (evento `mission-state`) */
export interface MissionAttention { workspaceId: string; missionId: string; blocked: boolean; ready: boolean; reasons: ("pergunta" | "permissao" | "falha")[] }
/** grupo nomeado de painéis do workspace (recolhível na grade) */
export interface PaneGroup { id: string; workspaceId: string; name: string; collapsed: boolean; ord: number }
/** item da conversa de um subagente: texto (pedido ou resposta) ou ferramenta recolhida */
export type SubagentItem = { kind: "text"; role: "voce" | "claude"; text: string } | { kind: "tool"; name: string; status: "ok" | "erro" | "pendente" };
export interface SubagentView { items: SubagentItem[]; description: string | null }

/** Execução de shell (Bash/PowerShell) de um painel do Claude. `output` vem vazia na lista; cheia por `execGet`. */
export interface ExecRow {
  id: string; parentPane: string; tool: string; command: string; description: string | null; background: boolean;
  startedAt: number; endedAt: number | null; status: "rodando" | "fundo" | "ok" | "erro"; exitCode: number | null;
  output: string; total: number; promotedPane: string | null;
}
/** Pedaço da saída de uma execução: `de` é a posição (bytes) em que o trecho começa; `exec.output` é o trecho. */
export interface ExecTrecho { exec: ExecRow; de: number }

/** Ajustes do painel Execuções (promoção a painel próprio). */
export interface ExecucoesPrefs {
  promover: boolean; promoverAposS: number; fecharOkS: number; maxPromovidos: number; desligadoClis: string[];
}
export interface PaneSpec {
  workspaceId: string; missionId?: string | null; kind: PaneKind; name?: string | null; cli?: string | null;
  shell?: string | null; model?: string | null; account?: string | null; permission?: Permission;
  effort?: string | null; route?: string | null; cwd: string; urlOrPath?: string | null; run?: string | null;
  prompt?: string | null; parentId?: string | null; role?: string | null; agent?: string | null; taskType?: string | null;
  mcpProfile?: McpProfile | null; allowedSkills?: string[] | null; mcpServers?: string[] | null; memoryMode?: MemoryMode | null;
  appendSystem?: string | null; disallowedTools?: string[] | null;
  /** terminal em workspace remoto: sessão tmux externa a anexar (nunca encerrada pelo Regente) */
  attachSession?: string | null;
}
export interface PermissionArgs { danger: string[]; auto: string[]; pend: string[]; plan: string[] }
export interface CliDef {
  id: string; label: string; command: string; install: string | null; models: string[]; efforts: string[];
  permission: PermissionArgs; modelArgs: string[]; effortArgs: string[]; sessionNew: string[]; sessionResume: string[];
  resumeFirst: boolean; accountEnv: string | null; cost: string | null; statusline: boolean; routable: boolean;
  promptArgs?: string[]; systemArgs?: string[]; disallowArgs?: string[]; mcp?: string | null; pluginArgs?: string[];
}
export interface RouteDef { id: string; label: string; baseUrl: string; secret: string | null; models: string[] }
export interface ShellDef { id: string; label: string; command: string; args: string[] }
export interface ProvidersConfig { clis: CliDef[]; routes: RouteDef[]; shells: ShellDef[] }
export interface Detected { id: string; kind: "cli" | "shell"; path: string | null }
export interface BriefSuggestion { text: string; source: string }
export interface SessionInfo { cli: string; id: string; modifiedMs: number; size: number; prompt: string | null }
export interface CliVersion { current: string | null; latest: string | null; newer: boolean; updateCmd: string | null; note: string | null }
export interface PriceEntry { match: string; input: number; output: number; cacheRead: number; cacheWrite: number }
export interface Prices { models: PriceEntry[] }
export interface Settings {
  theme: "dark" | "light" | "contraste"; terminalFont: string; terminalFontSize: number; scrollback: number; defaultShell: string;
  onboardingDone: boolean; accounts: Record<string, string[]>; accountProxy: Record<string, string>; spawnConcurrency: number;
  autoHibernate: { enabled: boolean; idleMinutes: number; residentFloor: number };
  /** "Enquanto você estava fora": minutos sem foco para o digest (0 = desligado) */
  ausencia: { minutos: number };
  memoryDisabled: string[];
  /** workspaces em modo sensível (sem segredo do cofre nos painéis) */
  sensitiveWorkspaces: string[];
  favorites: string[];
  quickstartDismissed: string[];
  allowRegenteTools: boolean;
  /** comandos de shell chegam ao painel Execuções (hooks Pre/PostToolUse) */
  execucoes: boolean;
  /** promoção a painel próprio, limites e CLIs sem execuções */
  execucoesPrefs: ExecucoesPrefs;
  /** um painel por subagente do Claude */
  subagentPanes: boolean;
  /** segundos até fechar subagente concluído; 0 = mantém */
  subagentCloseSecs: number;
  /** marca da migração do bloco B (subagentes e comandos dentro do painel); a interface não edita */
  inlineV1: boolean;
  /** canal das atualizações (URL do latest.json); vazio = sem canal configurado */
  updateEndpoint: string;
  /** atalhos da janela editados (ação -> combo); o que falta vem do padrão (`lib/shortcuts.ts`) */
  shortcuts: Record<string, string>;
  /** avisos do Windows: agente terminou, perguntou, gate aberto */
  notify: { finished: boolean; asks: boolean; gates: boolean };
  /** orçamento de custo em US$ (null = sem limite); pausar faz a missão estourada recusar painel novo */
  budget: { dailyUsd: number | null; missionUsd: number | null; pauseOnLimit: boolean };
  /** perfis de trabalho (ver `PerfisPrefs`) */
  perfis: PerfisPrefs;
  /** reabre as abas e restaura os painéis da véspera sem perguntar */
  restoreOnOpen: boolean;
  /** leitor de tela no terminal (xterm `screenReaderMode`) */
  screenReaderMode: boolean;
  // ▼ trilha B
  /** checkpoints também na raiz do workspace (só se for repositório) */
  checkpointsRaiz: boolean;
  /** memória: sugestões automáticas e expiração (`memory.rs` MemoriaPrefs) */
  memoria: { aceitarSugestoes: boolean; esquecerDias: number | null };
  /** busca por significado (M1c): ligada só depois da instalação (`embed_set_ativo`) */
  buscaSemantica: { ativo: boolean; modelo: string };
  /** voz local Supertonic 3: voz (F1–F5, M1–M5), velocidade (0,8–1,3) e passos (4, 8 ou 16) */
  vozLocal: { voz: string; velocidade: number; passos: number };
  // ▲ trilha B
}
import type { RPayload as RedlinePayload, RPoint } from "./redline";
/** Redline: por conta (`provedor/conta`): `ativa` lê o limite, `noTopo` mostra no chip da barra superior. */
export interface RedlinePrefs {
  accounts: Record<string, { ativa: boolean; noTopo: boolean }>;
}
/** Shot (F3): atalhos de captura desligados por padrão; `agentCapture` libera a captura pelo agente. */
export interface ShotConfig { hotkeys: boolean; fullKey: string; regionKey: string; recordKey: string; agentCapture: boolean }

export interface UpdateStatus { state: "nao-configurado" | "atualizado" | "disponivel"; current: string; version: string | null; notes: string | null }
export interface EmbedStatus {
  ativo: boolean;
  modelo: string;
  instalado: boolean;
  pronto: boolean;
  tamanhoMb: number;
  modeloMb: number;
  onnxruntimeMb: number;
}

/** progresso da instalação da busca por significado (`embed.rs` Progresso) */
export type EmbedProgresso = { evento: "arquivo"; nome: string; bytes: number; total: number } | { evento: "pronto" };

/** voz local (Supertonic 3): estado da instalação (`voz_local_status`); `bytesTotal` são os arquivos do modelo */
export interface VozLocalStatus { instalada: boolean; carregada: boolean; bytesTotal: number; vozes: string[] }
export type WhisperModelo = "preciso" | "leve";
export interface WhisperModeloStatus { instalado: boolean; bytes: number }
/** Whisper local (`whisper_local_status`): motor (binários), tamanho do zip e, por modelo, se está baixado */
export interface WhisperLocalStatus { binarios: boolean; bytesZip: number; modelos: Record<WhisperModelo, WhisperModeloStatus>; instalando: boolean }
/** progresso da instalação da voz local (`voz_local::Progresso`) */
export type VozProgresso = { evento: "arquivo"; nome: string; bytes: number; total: number } | { evento: "pronto" };

export type UpdateDownloadEvent =
  | { event: "started"; data: { contentLength: number | null } }
  | { event: "progress"; data: { chunk: number } }
  | { event: "finished" };
export interface WhatsNew { version: string; notes: string }

export interface MobileDevice { id: string; name: string; createdAt: number; lastSeen: number | null; pending: boolean }
/** Estado do servidor do celular (Configurações → Celular). `code` (só quando mostrado) e `url` só existem com o servidor ligado. */
export interface MobileStatus {
  running: boolean; url?: string; httpOnly?: boolean; fingerprint?: string | null; code?: string | null;
  devices: MobileDevice[]; port: number; lanIp: string | null;
}
export interface ShotInfo { path: string; name: string; kind: "png" | "mp4" | "frames" }
export interface VoiceConfig {
  engine: "groq" | "openai" | "local"; baseUrl: string; modelChat: string; modelCode: string; secret: string | null;
  whisperServer: string | null; whisperModel: string | null;
  /** modelo do Whisper local embutido e minutos até soltar a memória (0 = logo após cada ditado) */
  whisperModelo: WhisperModelo; whisperOciosoMin: 0 | 2 | 5 | 15;
  language: "auto" | "pt" | "en" | "es";
  mode: "codigo" | "planejamento" | "conversa"; toEnglish: boolean; terms: string[]; autoEnter: boolean;
  hotkey: string; hold: boolean; globalHotkey: string; globalMode: "chat" | "code";
  globalEnabled: boolean; globalHold: boolean;
  /** preço por minuto de áudio (US$) para somar o gasto de voz no Uso; 0 = sem preço */
  usdPerMin: number;
}
export interface VoiceEntry { id: number; ts: number; mode: "chat" | "code"; text: string }
export interface MediaConfig {
  imageProvider: "google" | "openai"; googleModel: string; openaiModel: string; videoModel: string; elevenVoice: string;
  enabled: string[]; baseUrls: Record<string, string>;
  /** teto de gerações por dia (todas as missões) e por missão, por tipo: image, video, audio */
  maxPerDay: Record<string, number>; maxPerMission: Record<string, number>;
}
export interface JarvisConfig {
  provider: "anthropic" | "openai"; baseUrl: string; model: string; effort: "low" | "medium" | "high";
  secret: string | null;
}
/** Ação pedida pelo celular que o usuário confirma na janela principal (`source` é o pedido de origem). */
export type JarvisConfirm = { id: string; workspace: string; source?: "jarvis" | "celular"; device?: string } & (
  | { kind: "gate"; gateId: string; decision: string; note: string | null; title: string }
  | { kind: "write"; paneId: string; paneName: string; text: string; enter: boolean }
  | { kind: "mission"; mode: string; name: string; briefExcerpt: string; input: Record<string, unknown> }
);
/** Memória (`db.rs` Memory). `status`: ativa, sugerida (espera revisão) ou arquivada (expirou). */
/** Arquivo de instruções do projeto (`instrucoes.rs`): texto atual e o hash que a escrita confere. */
export interface Instrucao { arquivo: string; texto: string; hash: string }
export interface SugestaoInstrucao { original: string; proposto: string }
export interface MemoryRow { id: number; workspaceId: string; missionId: string | null; kind: string; text: string; authorPane: string | null; createdAt: number; status: "ativa" | "sugerida" | "arquivada"; pinned: boolean; uses: number; lastUsedAt: number | null }
export interface UsageRow { day: string; cli: string; model: string; effort: string; tokIn: number; tokOut: number; costUsd: number; terminalsOpened: number }
/** Desperdício agregado por tipo (RM-21). `exemplo` some em workspace sensível. */
export interface WasteRow { kind: string; count: number; panes: number; tokensEst: number; lastAt: number; exemplo: string | null; removidos: number }
/** Higiene das instruções (RM-22): custo fixo e avisos. Só leitura. */
export interface HygieneReport { arquivos: { path: string; linhas: number; bytes: number; tokensEst: number; importacoes: number }[]; avisos: { gravidade: "info" | "aviso" | "alto"; arquivo: string; codigo: string; texto: string }[]; tokensFixos: number }
/** Painel de economia (RM-23). Valores de `economia` são estimativas: só com 5+ painéis por lado. */
export interface EconomyReport { grupos: { profile: string | null; taskType: string | null; model: string | null; missionId: string | null; panes: number; costUsd: number; tokens: number; cacheHitRatio: number | null; ctxMedioTurno: number | null; primeiroTurnoMedio: number | null; compactacoes: number; recomecos: number; escalonamentos: number }[]; acertoCache: number | null; contextoMedioTurno: number | null; economia: { taskType: string; amostrasPrecisao: number; amostrasEco: number; custoPrecisao: number; custoEco: number; economiaEstimada: number }[] }
/** Índice de código do workspace (RM-24): contagens do último passe e se o perfil o liga. */
export interface CodeIndexStats { ligado: boolean; sensivel: boolean; arquivos: number; simbolos: number; atualizadoMs: number | null }
/** Guarda de qualidade do workspace: ok = sem queda depois do último ajuste. */
export interface QualityGuard { ok: boolean; motivo: string }
export interface FsEntry { name: string; path: string; isDir: boolean }
export interface FsIndexEntry { rel: string; path: string }
export interface FsIndex { files: FsIndexEntry[]; truncated: boolean }
export interface ResTotals { memMb: number; cpuPct: number; procs: number }
export interface ResChild { pid: number; name: string; memMb: number; cpuPct: number }
export interface ResPane { paneId: string; name: string; status: string; memMb: number; cpuPct: number; procs: number; children: ResChild[] }
export interface ResSnapshot { total: ResTotals; panes: ResPane[]; ui: ResTotals; app: ResTotals; cpuReady: boolean }
export interface FileText { text: string; mtimeMs: number }
export interface PaneDetails {
  program: string; args: string[]; env: [string, string][]; cwd: string; sessionId: string | null;
  routeLabel: string | null; pid: number | null; memoryMb: number | null; skills: string[];
}
export type SlotState = "na_fila" | "abrindo" | "aguardando" | "ok" | "falha" | "cancelado" | "done";
export interface SlotEvent { missionId: string; index?: number; state: SlotState; paneId?: string; reason?: string }
export interface StatusEvent { id: string; status: PaneStatus; exitCode?: number; reason?: string | null }
/** Uso de contexto de um painel (statusline): `pane-context`. `ctxPct` null = sem medida ainda. */
export interface PaneContextEvent { workspaceId: string; paneId: string; ctxPct: number | null; ctxTokens: number | null; windowSize: number | null }
export interface CompactPropostaEvent { workspaceId: string; paneId: string; acao: string | null; motivo: string }
export interface UsageEvent { id: string; tokIn: number; tokOut: number; tokCacheR: number; tokCacheW: number; costUsd: number; model: string; sessionId: string | null }
export interface NewMission { workspaceId: string; name: string; mode: "livre" | "squad" | "agentico"; worktree: boolean; slots?: PaneSpec[]; squadId?: string; brief?: string; recipeId?: string; answers?: Record<number, string>; pilot?: { cli?: string | null; model?: string | null; effort?: string | null; account?: string | null }; style?: "chain" | "torre" | "plano" | "paralelo"; profile?: string | null }
export interface RecipeDef { id: string; name: string; description: string; mode: "livre" | "squad" | "agentico"; squad: string | null; pilot: { cli?: string | null; model?: string | null; effort?: string | null } | null; slots: Record<string, unknown>[]; intake: Record<string, string>; worktree: boolean | null; style: string | null; source: string }
export interface TaskType { id: string; name: string; category: string; description: string; examples: string[]; notExamples: string[]; cli: string; model: string | null; effort: string | null; agent: string; account: string | null; skills: string[]; perfil: string | null }
export interface AgenticConfig { types: TaskType[]; chooseAgent: boolean; decisor: { kind: "none" | "openai" | "anthropic"; baseUrl: string; model: string; secret: string | null }; defaultStyle: "chain" | "torre" | "paralelo" }
export interface Decision { id: string; missionId: string | null; paneId: string | null; request: string; taskType: string | null; cli: string | null; model: string | null; effort: string | null; agent: string | null; decidedBy: string; costUsd: number; receipt: string; createdAt: number }
export type GateOpen ={ id: string; missionId: string | null; paneId: string; title: string; summary: string; optionsJson: string | null; decision: string | null; note: string | null; createdAt: number; decidedAt: number | null };
/** Arquivo que a missão muda contra a base: A novo, M alterado, D apagado (revisão antes de integrar). */
/** `add`/`del`: linhas +/− contra a base; `fingerprint`: hash do conteúdo no disco (vazio se apagado). */
export interface ChangedFile { path: string; status: "A" | "M" | "D"; add: number; del: number; fingerprint: string }
/** Arquivo que pode entrar no commit: A novo, M alterado, D apagado, ? não rastreado. */
export interface GitEntry { path: string; code: "A" | "M" | "D" | "?" }
/** `remote`: `origin/x` sem local `x`; `ahead`/`behind` contra o remoto (última busca) */
export interface GitBranches { current: string; local: string[]; remote: string[]; upstream: string | null; ahead: number; behind: number; fetchError: string | null }
/** Worktree do repositório do workspace (bloco A); `adopted` = já é missão aberta. */
export interface WorktreeInfo { path: string; head: string; branch: string | null; adopted: boolean }
/** Estado vivo do worktree de uma missão: branch (null = livre), ↑ahead ↓behind contra `base`, alterados, último commit (ms). */
export interface WorktreeState { branch: string | null; base: string; ahead: number; behind: number; dirty: number; lastCommitAt: number | null; head: string }
/** Marca do Git de um arquivo da árvore de Arquivos (caminho absoluto, como a árvore mostra). */
export interface GitMarca { /** pasta nova inteira: tudo dentro é novo */ dir: boolean; path: string; code: "A" | "M" | "D" | "?" }
/** Mensagem sugerida: source "ia" ou "modelo" (pelos nomes dos arquivos); aiError diz por que a IA ficou de fora. */
export interface GitSuggestion { message: string; source: "ia" | "modelo"; aiError: string | null }
export interface PrSuggestion { title: string; body: string; source: "ia" | "modelo"; aiError: string | null }
/** As duas versões de um arquivo; binário ou acima de 2 MB vem com a flag e textos vazios. */
export interface FileVersions { base: string; current: string; binary: boolean; tooBig: boolean }
export interface TaskItem { id: string; missionId: string; title: string; status: "a_fazer" | "fazendo" | "feito" | string; ownerPane: string | null; createdAt: number; updatedAt: number }
/** Plano de uma missão agêntica: `arquivo` é o plano.md (caminho absoluto); null sem plano. */
export interface PlanView { plano: unknown; arquivo: string | null; tarefas: TaskItem[] }
/** entrega de um ajudante (`data_json`/`files_json` vêm como texto JSON) */
export interface Handoff { id: string; missionId: string | null; fromPane: string; toPane: string | null; summary: string; dataJson: string | null; filesJson: string | null; createdAt: number; seen: boolean }
export interface IntakeQ { q: string; autofill: "first_url" | "inspired_by" | null }
export interface SquadRoster { agent: string; role: string | null; cli: string | null; model: string | null; effort: string | null; max: number | null }
export interface SquadPhase { name: string; steps: string[]; gate: string | null }
export interface SquadDef {
  id: string; name: string; description: string; intake: IntakeQ[]; roster: SquadRoster[]; phases: SquadPhase[];
  maestroNotes: string; maestro: SquadRoster | null; peakPanes: number | null; source: "original" | "regente";
}
export interface FormerPane { name: string; cli: string | null; model: string | null; effort: string | null; role: string | null; agent: string | null; costUsd: number; durationMin: number }
export type ImageCheck = "ok" | "grande" | "fora" | "invalida";

export interface AgentDef {
  name: string; description: string; cli: string | null; model: string | null; effort: string | null;
  role: string | null; skills: string[]; body: string; source: "original" | "regente" | "claude"; path: string;
}
export interface SkillDef { name: string; description: string; source: "original" | "regente" | "claude" | "codex"; dir: string }
export interface MarketItem { source: string; name: string; kind: "skill" | "agent"; dir: string; files: string[]; description: string }
export interface InstalledIn { claude: boolean; codex: boolean; antigravity: boolean; regente: boolean }
// ▼ trilha A
/** Modelo e esforço de um papel no perfil (`cli` vazio = claude). */
export interface PerfilSlot { cli?: string; model: string; effort: string }
/** Perfil de trabalho completo, como o backend serializa (presets em `perfis.default.json`). */
export interface Perfil {
  id: string; nome: string;
  modelos: { planejar: PerfilSlot; implementar: PerfilSlot; revisar: PerfilSlot; testar: PerfilSlot; explorar: PerfilSlot };
  /** quem faz o quê na missão agêntica (CLI, modelo e esforço de cada papel); vence o arquivo do agente */
  papeis: { piloto: PerfilSlot; scout: PerfilSlot; builder: PerfilSlot; reviewer: PerfilSlot; controller: PerfilSlot; cruzado: PerfilSlot | null };
  /** degraus de escalonamento, do mais barato ao mais caro: `modelo:esforço` */
  escada: string[]; escalarAoFalhar: boolean;
  contexto: { janelaLongos: number; paybackK: number; restanteMin: number; recomecar: string; nativaOciosa: boolean };
  cacheTtlLongos: string;
  mcp: { ajudantes: string; filtrarPorPapel: boolean };
  saidas: { bashMax: number | null; leituraMax: number | null; resumirAcima: number; digestFalhas: boolean };
  testes: { relacionadosPrimeiro: boolean; completoNoFim: boolean };
  disciplinaExecutor: boolean; artefatos: { modo: string; formato: string };
  indiceCodigo: boolean; subagenteModelo: string | null; estiloConciso: boolean; paraleloMax: number;
}
/** Preferências de perfil: padrão, presets editados inteiros e ajustes por workspace (merge-patch). */
export interface PerfisPrefs { padrao: string; editados: Record<string, Perfil>; patches: Record<string, unknown> }
/** Resposta de `perfis_get`: os três presets e as preferências. */
export interface PerfisInfo extends PerfisPrefs { presets: Perfil[] }
/** Perfil efetivo de um workspace/missão: origem e se há ajuste do workspace (o chip mostra `*`). */
export interface PerfilEfetivo { perfil: Perfil; origem: "missao" | "padrao"; ajustado: boolean }
/** Fases da missão (X1.6). Ids: planejamento, execucao, verificacao, correcoes. Tempo em ms. */
export interface MissionPhases {
  atual: { phase: string; started_at: number } | null;
  per_phase: { phase: string; wall_ms: number; tok_in: number; tok_out: number; cost_usd: number }[];
  slowest_task: { task_id: string; wall_ms: number } | null;
  test_ms: number;
  code_ms: number;
}

/** Verificação da missão (X1b): comando próprio (ou o do projeto), toggles e limites. */
export interface VerifyCfg { comando: string | null; rodarAoTerminar: boolean; autoLoop: boolean; maxIter: number; timeoutMin: number }
/** Resultado de uma rodada:  é o resumo compacto que volta ao agente quando falha. */
export interface VerifyResultado { ok: boolean; exitCode: number | null; durationMs: number; digest: string; comando: string }
/** Visão da verificação de uma missão: painel da saída, configuração, comando do projeto, confiança e última rodada. */
export interface VerifyVisao { workspaceId: string; painel: string | null; cfg: VerifyCfg; comandoProjeto: string | null; confiado: boolean; rodando: boolean; ultimo: { resultado: VerifyResultado; modo: string; em: number } | null }
/** Linha do feed "Ajustes automáticos". `desfazivel` só para `perfil_patch`. */
export type AutoAjusteModo = "aplicar" | "propor" | "desligado";
/** Piloto automático de eficiência (T1): modo e o que aprendeu (null = ainda não aprendeu). */
export interface AutoAjuste { modo: AutoAjusteModo; aprendido: { k: number | null; rResumo: number | null; fatorRestante: number | null } }
export interface Ajuste { id: string; createdAt: number; workspaceId: string | null; missionId: string | null; paneId: string | null; kind: string; acao: "aplicado" | "proposto" | "pulado" | "aprendido"; motivo: string; antesJson: string | null; depoisJson: string | null; estimativaJson: string | null; efeitoJson: string | null; desfazivel: boolean; desfeitoAt: number | null }
// ▲ trilha A
// ▼ trilha B
/** Checkpoint do trabalho dos agentes (`checkpoints.rs`). `n` numera dentro do escopo; `files` vem capado, `totalFiles` inteiro. */
export interface Checkpoint { n: number; id: string; time: number; pane: string; files: { path: string; added: number; removed: number }[]; totalFiles: number; added: number; removed: number; /** arquivos grandes (acima de 20 MB) que ficaram de fora */ largeSkipped: number }
/** Volta a um checkpoint: `safety` é o n do checkpoint "antes de voltar" (restaurá-lo desfaz); `falhas` são arquivos que não voltaram. */
export interface Restored { safety: number; falhas: string[] }
// ▲ trilha B
// ▼ trilha C
/** `.regente/projeto.json` do workspace (ver `projeto.rs`): o arquivo (ou null) e o que o Regente detectou agora. */
export interface ProjetoInfo {
  valor: Record<string, unknown> | null;
  /** Hash dos comandos do arquivo lido (null se não existe ou é inválido). É o que a confiança grava. */
  hash: string | null;
  /** Erro de leitura (link, arquivo grande), se houver. */
  erro: string | null;
  caminho: string;
  detectado: { stacks: string[]; comandos: { build?: string; testar?: string; relacionados?: Record<string, string> } };
}
// comparar abordagens (A1c, RM-42)
export interface CompareVariant { groupId: string; missionId: string; label: string; createdAt: number }
export interface CompareVariante { missionId: string; label: string; status: string; files: number; add: number; del: number; costUsd: number; durationMs: number; working: boolean; uncommitted: number; verify: { resultado: VerifyResultado; modo: string; em: number } | null }
export interface CompareResumo { groupId: string; pronta: boolean; variantes: CompareVariante[]; total: { files: number; add: number; del: number; costUsd: number } }
// enquanto você estava fora (A1.6, RM-43): dados do banco e do git; o texto sai no front (lib/digest.ts)
export interface AwayData { missions: { id: string; name: string; status: string; files: number; add: number; del: number; costUsd: number; verify: { resultado: VerifyResultado; modo: string; em: number } | null }[]; handoffs: { missionId: string | null; fromPane: string; summary: string; createdAt: number }[] }
export interface CompareVariantSpec { label: string; cli: string; model?: string | null }
// ▲ trilha C

export const api = {
  appInfo: () => invoke<{ dataDir: string; version: string; windowsBuild: number }>("app_info"),
  startupPath: () => invoke<string | null>("startup_path"),
  workspaceOpen: (path: string) => invoke<Workspace>("workspace_open", { path }),
  workspaceList: () => invoke<Workspace[]>("workspace_list"),
  sshAliases: () => invoke<string[]>("ssh_aliases"),
  remoteSessions: (workspaceId: string) => invoke<string[]>("remote_sessions", { workspaceId }),
  workspaceRemove: (id: string) => invoke<void>("workspace_remove", { id }),
  paneList: (workspaceId: string) => invoke<PaneRow[]>("pane_list", { workspaceId }),
  paneSpawn: (spec: PaneSpec) => invoke<PaneRow>("pane_spawn", { spec }),
  paneAttach: (id: string, onData: (b: Uint8Array) => void) => {
    const ch = new Channel<ArrayBuffer>();
    ch.onmessage = (buf) => onData(new Uint8Array(buf));
    return invoke<number>("pane_attach", { id, onData: ch });
  },
  paneDetach: (id: string, sink: number) => invoke<void>("pane_detach", { id, sink }),
  paneWrite: (id: string, data: string) => invoke<void>("pane_write", { id, data }),
  /** issue do GitHub (número, #número ou URL) via `gh`, na pasta do workspace; só no clique do usuário */
  ghIssue: (workspaceId: string, reference: string) => invoke<GhIssue>("gh_issue", { workspaceId, reference }),
  /** troca modelo/esforço: Claude no ocioso manda /model; o resto reinicia retomando a sessão. `frame`: a tela atual */
  paneSwitchModel: (id: string, model: string | null, effort: string | null, frame: string, fresh: boolean) => invoke<PaneRow>("pane_switch_model", { id, model, effort, frame, fresh }),
  /** Continua o painel do Claude em outra conta ("principal" ou extra), retomando a conversa. */
  paneSwitchAccount: (id: string, account: string, frame: string) => invoke<PaneRow>("pane_switch_account", { id, account, frame }),
  /** Conta usada por último na pasta com a CLI (null = principal). */
  /** Branch, upstream e ↑↓ num processo git só (rodapé); listas vazias: as branches são do `gitBranches`. */
  gitSyncState: (workspaceId: string) => invoke<GitBranches>("git_sync_state", { workspaceId }),
  /** Branch atual (do workspace ou do worktree da missão) lida do HEAD; null fora de git; "" = destacado. */
  gitHead: (workspaceId: string, missionId: string | null) => invoke<string | null>("git_head", { workspaceId, missionId }),
  workspaceLastAccount: (workspaceId: string, cli: string) => invoke<string | null>("workspace_last_account", { workspaceId, cli }),
  /** prompt já expandido, colado com bracketed paste; Enter só se `enter` */
  promptPaste: (id: string, text: string, enter: boolean) => invoke<void>("prompt_paste", { id, text, enter }),
  /** conversa do painel em Markdown (Claude: JSONL; outros: `screen`, o texto do terminal) */
  paneTranscript: (id: string, fullTools: boolean, screen: string | null) => invoke<string>("pane_transcript", { id, fullTools, screen }),
  exportMarkdown: (path: string, text: string) => invoke<void>("export_markdown", { path, text }),
  /** busca nos buffers e nas conversas de todos os painéis abertos (até 200 resultados) */
  searchPanes: (query: string) => invoke<PaneSearch>("search_panes", { query }),
  /** Busca só no histórico em disco de um painel (o que já saiu do buffer do terminal), mais recentes primeiro */
  searchPaneHistory: (id: string, query: string) => invoke<PaneSearch>("search_pane_history", { id, query }),
  promptsList: () => invoke<PromptDef[]>("prompts_list"),
  promptsSave: (prompts: PromptDef[]) => invoke<PromptDef[]>("prompts_save", { prompts }),
  workspaceBranch: (workspaceId: string) => invoke<string>("workspace_branch", { workspaceId }),
  gitIsRepo: (workspaceId: string) => invoke<boolean>("git_is_repo", { workspaceId }),
  /** cria o repositório Git na pasta do workspace (sem efeito se já for) */
  gitInit: (workspaceId: string) => invoke<void>("git_init", { workspaceId }),
  /** branches do workspace; `fetch` busca o remoto antes (falha de rede vem em `fetchError`) */
  gitBranches: (workspaceId: string, fetch: boolean) => invoke<GitBranches>("git_branches", { workspaceId, fetch }),
  /** troca (ou cria, com `create`) a branch do workspace; devolve a atual */
  gitSwitch: (workspaceId: string, branch: string, create: boolean) => invoke<string>("git_switch", { workspaceId, branch, create }),
  /** puxa só em avanço rápido; devolve o resumo */
  gitPull: (workspaceId: string) => invoke<string>("git_pull", { workspaceId }),
  /** recebe (rebase) e envia; branch sem remoto é publicada; devolve o resumo */
  gitSync: (workspaceId: string) => invoke<string>("git_sync", { workspaceId }),
  paneResize: (id: string, cols: number, rows: number) => invoke<void>("pane_resize", { id, cols, rows }),
  paneClose: (id: string) => invoke<void>("pane_close", { id }),
  paneRename: (id: string, name: string) => invoke<void>("pane_rename", { id, name }),
  paneReorder: (ids: string[]) => invoke<void>("pane_reorder", { ids }),
  /** servidores MCP do Claude que o perfil custom pode manter (leitura; sem env nem tokens) */
  mcpServersList: (cli: string, cwd: string) => invoke<McpListing>("mcp_servers_list", { cli, cwd }),
  paneGroupList: (workspaceId: string) => invoke<PaneGroup[]>("pane_group_list", { workspaceId }),
  paneGroupCreate: (workspaceId: string, name: string) => invoke<PaneGroup>("pane_group_create", { workspaceId, name }),
  /** conversa de um painel de subagente (`subagent_items`) */
  subagentItems: (id: string) => invoke<SubagentView>("subagent_items", { id }),
  /** execuções de shell de um painel do Claude (sem saída) */
  execLista: (pane: string) => invoke<ExecRow[]>("exec_lista", { pane }),
  /** uma execução com a saída */
  execGet: (paneId: string, execId: string, desde: number) => invoke<ExecTrecho | null>("exec_get", { paneId, execId, desde }),
  execFixar: (pane: string, fixar: boolean) => invoke<void>("exec_fixar", { pane, fixar }),
  /** liga ou desliga o registro de shell deste painel do Claude (até o app reiniciar) */
  execDesligar: (pane: string, desligar: boolean) => invoke<void>("exec_desligar", { pane, desligar }),
  execDesligado: (pane: string) => invoke<boolean>("exec_desligado", { pane }),
  /** abre (ou acha) o painel Execuções do painel do Claude; devolve o id dele */
  execPainel: (pane: string) => invoke<string>("exec_painel", { pane }),
  /** "Abrir em painel": promove a execução agora, respeitando o teto */
  execPromover: (pane: string, execId: string) => invoke<void>("exec_promover", { pane, execId }),
  /** fecha os painéis promovidos concluídos e não fixados; devolve quantos */
  execFecharConcluidas: (pane: string) => invoke<number>("exec_fechar_concluidas", { pane }),
  paneGroupUpdate: (id: string, patch: { name?: string; collapsed?: boolean }) => invoke<void>("pane_group_update", { id, ...patch }),
  paneGroupDelete: (id: string) => invoke<void>("pane_group_delete", { id }),
  /** põe o painel no grupo (ou tira do grupo, com null) */
  paneSetGroup: (id: string, groupId: string | null) => invoke<void>("pane_set_group", { id, groupId }),
  paneHibernate: (id: string, frame: string) => invoke<void>("pane_hibernate", { id, frame }),
  paneWake: (id: string) => invoke<void>("pane_wake", { id }),
  paneSetResident: (id: string, resident: boolean) => invoke<void>("pane_set_resident", { id, resident }),
  paneFocus: (id: string) => invoke<void>("pane_focus", { id }),
  panesVisible: (ids: string[]) => invoke<void>("panes_visible", { ids }),
  paneHibernateReasons: (id: string, manual = false) => invoke<string[]>("pane_hibernate_reasons", { id, manual }),
  paneCompact: (id: string) => invoke<void>("pane_compact", { id }),
  paneFreshStart: (id: string) => invoke<void>("pane_fresh_start", { id }),
  cronList: (workspaceId?: string) => invoke<CronJob[]>("cron_list", { workspaceId: workspaceId ?? null }),
  cronSave: (job: CronJob) => invoke<CronJob>("cron_save", { job }),
  cronDelete: (id: string) => invoke<void>("cron_delete", { id }),
  cronRunNow: (id: string) => invoke<void>("cron_run_now", { id }),
  cronPreview: (expr: string) => invoke<number[]>("cron_preview", { expr }),
  paneFrame: (id: string) => invoke<string | null>("pane_frame", { id }),
  paneDetails: (id: string) => invoke<PaneDetails>("pane_details", { id }),
  paneHistory: (id: string) => invoke<string>("pane_history", { id }),
  restoreList: (workspaceId: string) => invoke<PaneRow[]>("restore_list", { workspaceId }),
  /** `paneIds`: só estes painéis (um cartão); sem ele, todos os que sobraram */
  restoreApply: (workspaceId: string, accept: boolean, paneIds?: string[]) => invoke<PaneRow[]>("restore_apply", { workspaceId, accept, paneIds }),
  missionCreate: (input: NewMission) => invoke<Mission>("mission_create", { input }),
  missionList: (workspaceId: string) => invoke<Mission[]>("mission_list", { workspaceId }),
  /** worktrees do repositório do workspace, sem o principal (bloco A) */
  worktreesList: (workspaceId: string) => invoke<WorktreeInfo[]>("worktrees_list", { workspaceId }),
  /** adota um worktree existente como missão (nada muda no git) */
  missionAdopt: (workspaceId: string, path: string) => invoke<Mission>("mission_adopt", { workspaceId, path }),
  /** branch, ↑↓ contra a base, alterados e último commit do worktree da missão (sem fetch) */
  missionWorktreeState: (id: string) => invoke<WorktreeState>("mission_worktree_state", { id }),
  gateAnswer: (id: string, decision: string, note?: string) => invoke<"waiter" | "typed" | "pane_closed" | "plan">("gate_answer", { id, decision, note: note ?? null }),
  gatesOpen: (workspaceId?: string) => invoke<GateOpen[]>("gates_open", { workspaceId: workspaceId ?? null }),
  tasksList: (missionId: string) => invoke<TaskItem[]>("tasks_list", { missionId }),
  planGet: (missionId: string) => invoke<PlanView>("plan_get", { missionId }),
  planResume: (missionId: string) => invoke<number>("plan_resume", { missionId }),
  unseenHandoffs: (workspaceId: string) => invoke<{ missionId: string; count: number }[]>("unseen_handoffs", { workspaceId }),
  missionMarkSeen: (id: string) => invoke<void>("mission_mark_seen", { id }),
  squadsList: () => invoke<SquadDef[]>("squads_list"),
  squadSave: (squad: SquadDef) => invoke<void>("squad_save", { squad }),
  squadFormerBrief: () => invoke<FormerPane[]>("squad_former_brief"),
  memoryList: (workspaceId: string, missionId?: string | null) => invoke<MemoryRow[]>("memory_list", { workspaceId, missionId: missionId ?? null }),
  memorySearch: (workspaceId: string, q: string) => invoke<MemoryRow[]>("memory_search", { workspaceId, q }),
  memoryUpdate: (id: number, text: string) => invoke<void>("memory_update", { id, text }),
  memoryDelete: (id: number) => invoke<void>("memory_delete", { id }),
  memoryToggle: (workspaceId: string, enabled: boolean) => invoke<void>("memory_toggle", { workspaceId, enabled }),
  /** devolve os painéis abertos com rota que usa segredo do Cofre (ao ligar): seguem rodando até reiniciar */
  workspaceSensitiveSet: (workspaceId: string, on: boolean) => invoke<string[]>("workspace_sensitive_set", { workspaceId, on }),
  recipesList: () => invoke<RecipeDef[]>("recipes_list"),
  recipeSave: (recipe: RecipeDef) => invoke<void>("recipe_save", { recipe }),
  recipeDelete: (id: string) => invoke<void>("recipe_delete", { id }),
  agenticGet: () => invoke<AgenticConfig>("agentic_get"),
  agenticSave: (config: AgenticConfig) => invoke<void>("agentic_save", { config }),
  voiceGet: () => invoke<VoiceConfig>("voice_get"),
  voiceSave: (config: VoiceConfig) => invoke<void>("voice_save", { config }),
  /** o áudio vai como corpo binário; mime e destino em cabeçalhos (ver commands::voice_transcribe) */
  voiceTranscribe: async (audio: Blob, mime: string, purpose: "pane" | "global") =>
    invoke<string>("voice_transcribe", await audio.arrayBuffer(), { headers: { mime, purpose } }),
  /** ditado global: o áudio é transcrito e colado no app focado; devolve o texto colado */
  voiceGlobalFinish: async (audio: Blob, mime: string) => invoke<string>("voice_global_finish", await audio.arrayBuffer(), { headers: { mime, purpose: "global" } }),
  voiceGlobalCancel: () => invoke<void>("voice_global_cancel"),
  voiceHistoryList: () => invoke<VoiceEntry[]>("voice_history_list"),
  voiceHistoryClear: () => invoke<void>("voice_history_clear"),
  /** Redline: a leitura nova de todas as contas ativas (também atualiza a bandeja e o chip) */
  redlineNow: () => invoke<RedlinePayload>("redline_now"),
  /** Redline: as últimas leituras, sem consultar nada */
  redlineCurrent: () => invoke<RedlinePayload>("redline_current"),
  /** Redline: histórico de até 30 dias, do mais antigo ao mais novo */
  redlineHistory: (days: number) => invoke<RPoint[]>("redline_history", { days }),
  redlinePrefs: () => invoke<{ prefs: RedlinePrefs; accounts: string[] }>("redline_prefs"),
  redlineSetPrefs: (prefs: RedlinePrefs) => invoke<void>("redline_set_prefs", { prefs }),
  redlineOpen: () => invoke<void>("redline_open"),
  shotGet: () => invoke<{ config: ShotConfig; dir: string; ffmpeg: boolean }>("shot_get"),
  shotSave: (config: ShotConfig) => invoke<void>("shot_save", { config }),
  /** tela inteira do monitor sob o cursor; devolve o arquivo salvo */
  shotFull: () => invoke<string>("shot_full"),
  shotRegionBegin: () => invoke<void>("shot_region_begin"),
  /** gravar região: o overlay escolhe a região e a gravação começa; a barra "Parar" fica aberta */
  shotRecordBegin: () => invoke<void>("shot_record_begin"),
  /** Parar: devolve o vídeo (o app abre o corte pelo evento shot-recorded) */
  shotRecordStop: () => invoke<string>("shot_record_stop"),
  /** quadros de um trecho (segundos) do vídeo, 1 ou 2 por segundo, e a folha de contato */
  shotExtract: (path: string, start: number, end: number, fps: 1 | 2) =>
    invoke<{ dir: string; frames: number; contact: string }>("shot_extract", { path, start, end, fps }),
  /** região escolhida no overlay, em pixels da imagem; devolve o arquivo (ou null no modo gravar) */
  shotRegionCommit: (x: number, y: number, w: number, h: number) => invoke<string | null>("shot_region_commit", { x, y, w, h }),
  shotRegionCancel: () => invoke<void>("shot_region_cancel"),
  /** a tela congelada do overlay, em PNG */
  shotOverlayImage: () => invoke<ArrayBuffer>("shot_overlay_image"),
  shotList: () => invoke<ShotInfo[]>("shot_list"),
  /** o início do editor: a imagem a desenhar (o original, depois da primeira edição) e o documento salvo */
  shotEditGet: (path: string) => invoke<{ base: string; doc: string | null }>("shot_edit_get", { path }),
  shotRead: (path: string) => invoke<ArrayBuffer>("shot_read", { path }),
  /** autosave do editor: PNG renderizado (base64) e o documento {crop, layers} em JSON */
  shotSaveEdit: (path: string, pngBase64: string, layers: string) => invoke<void>("shot_save_edit", { path, png: pngBase64, layers }),
  shotDelete: (path: string) => invoke<void>("shot_delete", { path }),
  mediaGet: () => invoke<MediaConfig>("media_get"),
  mediaSave: (config: MediaConfig) => invoke<void>("media_save", { config }),
  /** [Testar]: a chamada mais barata do provedor; devolve o arquivo gerado ou a conta alcançada */
  mediaTest: (provider: string) => invoke<string>("media_test", { provider }),
  /** gerações de hoje por tipo, contra os tetos */
  mediaUsage: () => invoke<{ kind: string; today: number; perDay: number | null; perMission: number | null }[]>("media_usage"),
  jarvisGet: () => invoke<JarvisConfig>("jarvis_get"),
  jarvisSave: (config: JarvisConfig) => invoke<void>("jarvis_save", { config }),
  jarvisWriteConfirmed: (paneId: string, text: string, enter: boolean) => invoke<void>("jarvis_write_confirmed", { paneId, text, enter }),
  decisorTest: () => invoke<{ taskType: string; model: string; latencyMs: number; costUsd: number; measured: boolean }>("decisor_test"),
  decisionsList: (sinceDays?: number) => invoke<Decision[]>("decisions_list", { sinceDays: sinceDays ?? null }),
  missionUpdate: (mission: Mission) => invoke<Mission>("mission_update", { mission }),
  missionIntegrate: (id: string) => invoke<Mission>("mission_integrate", { id }),
  /** `manterPasta`: arquiva mesmo com `worktree_locked` (a pasta sai no próximo boot). */
  missionArchive: (id: string, manterPasta = false) => invoke<Mission>("mission_archive", { id, manterPasta }),
  missionDiffFiles: (id: string) => invoke<ChangedFile[]>("mission_diff_files", { id }),
  missionFileVersions: (id: string, path: string) => invoke<FileVersions>("mission_file_versions", { id, path }),
  missionRevertFile: (id: string, path: string) => invoke<void>("mission_revert_file", { id, path }),
  gitStatus: (workspaceId: string, missionId: string | null) => invoke<GitEntry[]>("git_status", { workspaceId, missionId }),
  /** diff de trabalho capado (30 KB) do workspace ou da missão: `@diff` do compositor */
  diffText: (workspaceId: string, missionId: string | null) => invoke<string>("diff_text", { workspaceId, missionId }),
  gitCommitSuggest: (workspaceId: string, missionId: string | null, paths: string[]) => invoke<GitSuggestion>("git_commit_suggest", { workspaceId, missionId, paths }),
  gitCommit: (workspaceId: string, missionId: string | null, paths: string[], message: string) => invoke<string>("git_commit", { workspaceId, missionId, paths, message }),
  gitPrSuggest: (workspaceId: string, missionId: string | null, base: string) => invoke<PrSuggestion>("git_pr_suggest", { workspaceId, missionId, base }),
  gitPrBase: (workspaceId: string, missionId: string | null) => invoke<string>("git_pr_base", { workspaceId, missionId }),
  /** mensagem do modelo local para abrir o commit preenchida; null quando há IA (ela roda só no clique) */
  gitCommitPrefill: (paths: string[]) => invoke<string | null>("git_commit_prefill", { paths }),
  /** título e descrição do modelo local para abrir o PR preenchidos; null quando há IA */
  gitPrPrefill: (workspaceId: string, missionId: string | null, base: string) => invoke<PrSuggestion | null>("git_pr_prefill", { workspaceId, missionId, base }),
  gitPrOpen: (workspaceId: string, missionId: string | null, base: string, title: string, body: string) => invoke<string>("git_pr_open", { workspaceId, missionId, base, title, body }),
  spawnCancel: (missionId: string, index: number) => invoke<boolean>("spawn_cancel", { missionId, index }),
  recentMissions: (limit: number) => invoke<RecentMission[]>("recent_missions", { limit }),
  missionTotals: (workspaceId: string) => invoke<{ missionId: string; tokens: number; cacheTokens: number; costUsd: number }[]>("mission_totals", { workspaceId }),
  missionAttention: (workspaceId: string) => invoke<MissionAttention[]>("mission_attention", { workspaceId }),
  providersGet: () => invoke<ProvidersConfig>("providers_get"),
  providersSave: (config: ProvidersConfig) => invoke<void>("providers_save", { config }),
  providersDetect: () => invoke<Detected[]>("providers_detect"),
  cliVersionCheck: (id: string) => invoke<CliVersion>("cli_version_check", { id }),
  briefSuggest: (text: string) => invoke<BriefSuggestion>("brief_suggest", { text }),
  sessionsList: (workspaceId: string) => invoke<SessionInfo[]>("sessions_list", { workspaceId }),
  sessionResume: (workspaceId: string, cli: string, sessionId: string) => invoke<PaneRow>("session_resume", { workspaceId, cli, sessionId }),
  routeTest: (routeId: string) => invoke<string>("route_test", { routeId }),
  accountAdd: (cli: string, name: string) => invoke<Settings>("account_add", { cli, name }),
  accountRemove: (cli: string, name: string) => invoke<Settings>("account_remove", { cli, name }),
  pricesGet: () => invoke<Prices>("prices_get"),
  pricesSave: (prices: Prices) => invoke<void>("prices_save", { prices }),
  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (settings: Settings) => invoke<void>("settings_set", { settings }),
  terminalFontFromWt: () => invoke<string>("terminal_font_from_wt"),
  vaultNames: () => invoke<string[]>("vault_names"),
  conectoresList: () => invoke<Conector[]>("conectores_list"),
  conectorSave: (nome: string, config: string) => invoke<Conector[]>("conector_save", { nome, config }),
  conectorContext7: (chave: string | null) => invoke<Conector[]>("conector_context7", { chave }),
  conectorToggle: (nome: string, ativo: boolean) => invoke<Conector[]>("conector_toggle", { nome, ativo }),
  conectorDelete: (nome: string) => invoke<Conector[]>("conector_delete", { nome }),
  conectoresImportaveis: () => invoke<ConectorImportavel[]>("conectores_importaveis"),
  conectorImport: (nome: string, conta: string) => invoke<Conector[]>("conector_import", { nome, conta }),
  vaultSet: (name: string, value: string) => invoke<void>("vault_set", { name, value }),
  vaultDelete: (name: string) => invoke<void>("vault_delete", { name }),
  usageRows: (since: string | null) => invoke<UsageRow[]>("usage_rows", { since }),
  wasteSummary: (sinceMs: number, workspaceId?: string | null) => invoke<WasteRow[]>("waste_summary", { sinceMs, workspaceId: workspaceId ?? null }),
  hygieneScan: (workspaceId: string) => invoke<HygieneReport>("hygiene_scan", { workspaceId }),
  economyReport: (sinceMs: number, workspaceId?: string | null) => invoke<EconomyReport>("economy_report", { sinceMs, workspaceId: workspaceId ?? null }),
  qualityGuard: (workspaceId: string) => invoke<QualityGuard>("quality_guard", { workspaceId }),
  codeIndexStats: (workspaceId: string) => invoke<CodeIndexStats>("code_index_stats", { workspaceId }),
  codeIndexSet: (workspaceId: string, ligado: boolean) => invoke<void>("code_index_set", { workspaceId, ligado }),
  usageTotals: () => invoke<{ tokens: number; costUsd: number }>("usage_totals"),
  /** `ignorados`: inclui `.git`, `node_modules`, `target`… */
  /** Copia para dentro de `dir` com nome livre ("a (cópia).ts"); devolve o caminho novo. */
  /** A árvore mostra `root`: mudanças viram `fs-changed` e a troca de branch `git-head` (contado por raiz). */
  fsWatch: (root: string) => invoke<void>("fs_watch", { root }),
  fsUnwatch: (root: string) => invoke<void>("fs_unwatch", { root }),
  /** Acha em disco um caminho citado no terminal (pasta do painel, topo do git, workspace, repositório aninhado, sufixo único). */
  fsResolverLink: (texto: string, cwd: string, workspaceId: string) => invoke<{ path: string; dir: boolean; dentro: boolean } | null>("fs_resolver_link", { texto, cwd, workspaceId }),
  /** Texto de um arquivo fora dos workspaces, para abrir só leitura. */
  fsReadExterno: (path: string) => invoke<FileText>("fs_read_externo", { path }),
  fsCopy: (src: string, dir: string) => invoke<string>("fs_copy", { src, dir }),
  fsList: (path: string, ignorados = false) => invoke<FsEntry[]>("fs_list", { path, ignorados }),
  fsIndex: (root: string) => invoke<FsIndex>("fs_index", { root }),
  /** marcas do Git dos arquivos sob `root` (caminhos absolutos, no mesmo formato da árvore); vazio fora de repositório */
  fsGitMarcas: (root: string) => invoke<GitMarca[]>("fs_git_marcas", { root }),
  /** texto do arquivo no último commit (null: fora do Git, não rastreado, binário ou grande demais) */
  fsGitBase: (path: string) => invoke<string | null>("fs_git_base", { path }),
  resourcesSnapshot: () => invoke<ResSnapshot>("resources_snapshot"),
  fsRead: (path: string) => invoke<FileText>("fs_read", { path }),
  fsWrite: (path: string, text: string, expectedMtime: number | null) => invoke<number>("fs_write", { path, text, expectedMtime }),
  fsCreate: (path: string, isDir: boolean) => invoke<void>("fs_create", { path, isDir }),
  fsRename: (from: string, to: string) => invoke<void>("fs_rename", { from, to }),
  fsDelete: (path: string) => invoke<void>("fs_delete", { path }),
  imageCheck: (path: string) => invoke<ImageCheck>("image_check", { path }),
  diagGenerate: () => invoke<string>("diag_generate"),
  catalogAgents: () => invoke<AgentDef[]>("catalog_agents"),
  catalogSkills: () => invoke<SkillDef[]>("catalog_skills"),
  /** skill do Regente: estado na pasta de skills do Claude (só leitura) */
  baseSkillStatus: () => invoke<BaseSkillStatus>("base_skill_status"),
  baseSkillInstall: () => invoke<BaseSkillInstall>("base_skill_install"),
  /** remove o que o Regente instalou; vai para a Lixeira */
  baseSkillRemove: () => invoke<string[]>("base_skill_remove"),
  catalogSkillText: (dir: string) => invoke<string>("catalog_skill_text", { dir }),
  catalogSaveAgent: (agent: AgentDef) => invoke<string>("catalog_save_agent", { agent }),
  marketplaceSearch: (q: string, kind: "skill" | "agent" | null, force = false) => invoke<MarketItem[]>("marketplace_search", { q, kind, force }),
  marketplaceInstall: (item: MarketItem, target: string) => invoke<string>("marketplace_install", { item, target }),
  marketplaceInstalled: (item: MarketItem) => invoke<InstalledIn>("marketplace_installed", { item }),
  marketplaceUninstall: (name: string, kind: string, target: string) => invoke<void>("marketplace_uninstall", { name, kind, target }),
  mobileStatus: () => invoke<MobileStatus>("mobile_status"),
  mobileStart: (httpOnly: boolean) => invoke<MobileStatus>("mobile_start", { httpOnly }),
  mobileStop: () => invoke<void>("mobile_stop"),
  /** SVG do QR de pareamento (token novo, uso único, 10 min) */
  mobilePairQr: () => invoke<string>("mobile_pair_qr"),
  mobileRevoke: (id: string) => invoke<void>("mobile_revoke", { id }),
  /** mostra um código de 6 dígitos por 5 minutos (inativo até o usuário pedir) */
  mobileShowCode: () => invoke<string>("mobile_show_code"),
  /** aprova (ou recusa) um celular pareado por código */
  mobileApprove: (id: string, approve: boolean) => invoke<void>("mobile_approve", { id, approve }),
  /** verifica o canal; nada é baixado aqui */
  updateCheck: () => invoke<UpdateStatus>("update_check"),
  /** baixa e instala a atualização verificada, com progresso */
  updateInstall: (onEvent: (e: UpdateDownloadEvent) => void) => {
    const ch = new Channel<UpdateDownloadEvent>();
    ch.onmessage = onEvent;
    return invoke<void>("update_install", { onEvent: ch });
  },
  /** busca por significado: estado da instalação e da ligação (tamanhos em MB para a tela) */
  embedStatus: () => invoke<EmbedStatus>("embed_status"),
  /** baixa o ONNX Runtime e o modelo (SHA-256 conferido), com progresso */
  embedInstall: (onEvent: (e: EmbedProgresso) => void) => {
    const ch = new Channel<EmbedProgresso>();
    ch.onmessage = onEvent;
    return invoke<void>("embed_install", { onEvent: ch });
  },
  embedSetAtivo: (ativo: boolean) => invoke<void>("embed_set_ativo", { ativo }),
  embedRemove: () => invoke<void>("embed_remove"),
  /** cancela a instalação em curso (o `.part` é apagado) */
  embedCancel: () => invoke<void>("embed_cancel"),
  /** voz local (Supertonic 3): estado, instalação com progresso, cancelamento, remoção e prova de voz (WAV em base64) */
  vozLocalStatus: () => invoke<VozLocalStatus>("voz_local_status"),
  vozLocalInstalar: (onEvent: (p: VozProgresso) => void) => {
    const ch = new Channel<VozProgresso>();
    ch.onmessage = onEvent;
    return invoke<void>("voz_local_instalar", { onEvent: ch });
  },
  vozLocalCancelar: () => invoke<void>("voz_local_cancelar"),
  vozLocalRemover: () => invoke<void>("voz_local_remover"),
  vozLocalOuvir: (voz: string) => invoke<string>("voz_local_ouvir", { voz }),
  /** Whisper local embutido (ditado sem nuvem): estado, instalação com progresso, cancelamento e remoção */
  whisperLocalStatus: () => invoke<WhisperLocalStatus>("whisper_local_status"),
  whisperLocalInstalar: (modelo: WhisperModelo, onEvent: (p: VozProgresso) => void) => {
    const ch = new Channel<VozProgresso>();
    ch.onmessage = onEvent;
    return invoke<void>("whisper_local_instalar", { modelo, onEvent: ch });
  },
  whisperLocalCancelar: () => invoke<void>("whisper_local_cancelar"),
  whisperLocalRemover: (modelo: WhisperModelo) => invoke<void>("whisper_local_remover", { modelo }),
  whatsNewGet: () => invoke<WhatsNew | null>("whats_new_get"),
  whatsNewSeen: () => invoke<void>("whats_new_seen"),
  // ▼ trilha A
  perfisGet: () => invoke<PerfisInfo>("perfis_get"),
  perfisSave: (perfis: PerfisPrefs) => invoke<void>("perfis_save", { perfis }),
  /** O perfil que uma missão nova teria aqui com o perfil `profile` (null = padrão), com o ajuste do projeto. */
  perfilPreview: (workspaceId: string, profile: string | null) => invoke<Perfil>("perfil_preview", { workspaceId, profile }),
  perfilEfetivo: (workspaceId: string, missionId?: string | null) => invoke<PerfilEfetivo>("perfil_efetivo", { workspaceId, missionId: missionId ?? null }),
  missionSetProfile: (id: string, profile: string | null) => invoke<void>("mission_set_profile", { id, profile }),
  perfilRestore: (id: string) => invoke<void>("perfil_restore", { id }),
  ajustesList: (workspaceId?: string | null) => invoke<Ajuste[]>("ajustes_list", { workspaceId: workspaceId ?? null }),
  ajustesUndo: (id: string) => invoke<void>("ajustes_undo", { id }),
  ajustesAplicar: (id: string) => invoke<void>("ajustes_aplicar", { id }),
  ajustesIgnorar: (id: string) => invoke<void>("ajustes_ignorar", { id }),
  autoajusteGet: () => invoke<AutoAjuste>("autoajuste_get"),
  autoajusteSetModo: (modo: AutoAjusteModo) => invoke<void>("autoajuste_set_modo", { modo }),
  autoajusteRodar: () => invoke<void>("autoajuste_rodar"),
  autoajusteZerarAprendizado: () => invoke<void>("autoajuste_zerar_aprendizado"),
  panePerfil: (id: string) => invoke<string[]>("pane_perfil", { id }),
  perfilReadDenies: () => invoke<string[]>("perfil_read_denies"),
  paneIdleSecs: (id: string) => invoke<number | null>("pane_idle_secs", { id }),
  perfilPatchRemove: (workspaceId: string) => invoke<void>("perfil_patch_remove", { workspaceId }),
  missionVerifyGet: (id: string) => invoke<VerifyVisao>("mission_verify_get", { id }),
  missionVerifySet: (id: string, value: VerifyCfg) => invoke<VerifyVisao>("mission_verify_set", { id, value }),
  missionVerifyRun: (id: string) => invoke<VerifyResultado>("mission_verify_run", { id }),
  /** Desempenho por fase da missão (X1.6): barras por fase, tarefa mais lenta, testes × código. */
  missionPhases: (missionId: string) => invoke<MissionPhases>("mission_phases", { missionId }),
  missionVerifyCancel: (id: string) => invoke<boolean>("mission_verify_cancel", { id }),
  missionVerifyEnviar: (id: string) => invoke<void>("mission_verify_enviar", { id }),
  // ▲ trilha A
  // ▼ trilha B
  handoffList: (missionId: string) => invoke<Handoff[]>("handoff_list", { missionId }),
  /** escopo = id da missão com worktree ou do workspace; `pane` vai para a mensagem do commit */
  checkpointTake: (scope: string, pane?: string) => invoke<Checkpoint | null>("checkpoint_take", { scope, pane: pane ?? null }),
  checkpointList: (scope: string) => invoke<Checkpoint[]>("checkpoint_list", { scope }),
  checkpointDiff: (scope: string, n: number, path: string) => invoke<FileVersions>("checkpoint_diff", { scope, n, path }),
  checkpointRestore: (scope: string, n: number) => invoke<Restored>("checkpoint_restore", { scope, n }),
  memoryListStatus: (workspaceId: string, status: "sugerida" | "arquivada") => invoke<MemoryRow[]>("memory_list_status", { workspaceId, status }),
  memorySetStatus: (id: number, status: "ativa" | "sugerida" | "arquivada") => invoke<void>("memory_set_status", { id, status }),
  memorySetPinned: (id: number, pinned: boolean) => invoke<void>("memory_set_pinned", { id, pinned }),
  memoryAddGlobal: (text: string) => invoke<number>("memory_add_global", { text }),
  instrucoesListar: (workspaceId: string) => invoke<Instrucao[]>("instrucoes_listar", { workspaceId }),
  instrucoesSugerir: (workspaceId: string, file: string) => invoke<SugestaoInstrucao>("instrucoes_sugerir", { workspaceId, file }),
  instrucoesAplicar: (workspaceId: string, file: string, texto: string, hashOriginal: string) => invoke<void>("instrucoes_aplicar", { workspaceId, file, texto, hashOriginal }),
  // ▲ trilha B
  // ▼ trilha C
  awayData: (workspaceId: string, since: number) => invoke<AwayData>("away_data", { workspaceId, since }),
  awayResumir: (workspaceId: string, texto: string) => invoke<string>("away_resumir", { workspaceId, texto }),
  projetoGet: (workspaceId: string) => invoke<ProjetoInfo>("projeto_get", { workspaceId }),
  projetoSave: (workspaceId: string, value: Record<string, unknown>) => invoke<void>("projeto_save", { workspaceId, value }),
  projetoTrust: (workspaceId: string, trust: boolean, hash: string | null) => invoke<void>("projeto_trust", { workspaceId, trust, hash }),
  projetoGlossario: (workspaceId: string, rel: string) => invoke<string>("projeto_glossario", { workspaceId, rel }),
  projetoTrusted: (workspaceId: string) => invoke<boolean>("projeto_trusted", { workspaceId }),
    compareStart: (workspaceId: string, name: string, prompt: string, variants: CompareVariantSpec[], permission: Permission) => invoke<CompareVariant[]>("compare_start", { workspaceId, name, prompt, variants, permission }),
    compareEspaco: (workspaceId: string) => invoke<{ livreBytes: number | null; estimadoBytes: number }>("compare_espaco", { workspaceId }),
  compareGet: (groupId: string) => invoke<CompareResumo>("compare_get", { groupId }),
  compareKeep: (groupId: string, missionId: string) => invoke<{ integrada: boolean; falhas: string[] }>("compare_keep", { groupId, missionId }),
// ▲ trilha C
};

export function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => cb(e.payload));
}

export function errMsg(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

export function errCode(e: unknown): string | null {
  return e && typeof e === "object" && "code" in e ? String((e as { code: unknown }).code) : null;
}
