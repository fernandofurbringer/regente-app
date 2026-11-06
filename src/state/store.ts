import { ask } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { isCleared } from "../lib/attention";
import type { GridPrefs, ShellPrefs } from "../lib/layout";
import { forgetPaneErrors } from "../lib/paneErrors";
import type { Accepted } from "../lib/review";
import type { ErrorBlock } from "../lib/errors";
import { sinceDay } from "../lib/format";
import { readSession, writeSession } from "../lib/session";
import { pushLog } from "../lib/toasts";
import { remover, type DevMap } from "../lib/devServers";
import type { RPayload } from "../lib/redline";
import type { EditorStatus } from "../lib/editorStatus";
import { api, errCode, errMsg, type Detected, type UsageRow, type GateOpen, type Mission, type MissionAttention, type PaneGroup, type PaneRow, type ProvidersConfig, type Settings, type JarvisConfirm, type SlotEvent, type SlotState, type VoiceConfig, type Workspace } from "../lib/ipc";

export interface Toast { id: number; kind: "info" | "erro" | "ok"; text: string; action?: { label: string; run(): void; alt?: { label: string; run(): void } }; ms: number }
/** Gasto e tokens (sem cache) de um período. */
export interface Spend { tokens: number; costUsd: number }
/** texto para o painel de IA de uma missão (ou do workspace, sem missão) */
export interface SendTo { workspaceId: string; missionId: string | null; text: string; title: string; /** painel de origem: o anel de erros dele zera quando o envio sai */ fromPane?: string }
export interface Slot { state: SlotState; paneId?: string; reason?: string }
export type View = "terminais" | "arquivos";
export type Dialog = null | "painel" | "missao";
/** `text`: instantâneo somente leitura (histórico de painel); sem ele, o arquivo é lido do disco. */
export interface OpenFile { path: string; line?: number; col?: number; text?: string }

interface State {
  workspaces: Workspace[];
  tabs: string[];
  active: string | null;
  panes: Record<string, PaneRow>;
  /** grupos de painéis por workspace */
  groups: Record<string, PaneGroup[]>;
  missions: Record<string, Mission[]>;
  slots: Record<string, Record<number, Slot>>;
  focus: Record<string, string | null>;
  view: Record<string, View>;
  openFile: Record<string, OpenFile | null>;
  /** caminhos com alterações não salvas no editor, por workspace */
  dirty: Record<string, string[]>;
  /** barra de status do editor por workspace (posição, indentação, EOL, linguagem, tamanho); null sem arquivo no editor */
  editorStatus: Record<string, EditorStatus | null>;
  /** URL de servidor local detectada no terminal, por workspace (pulsa o + NAVEGADOR) */
  readyUrl: Record<string, string | null>;
  /** medidor de contexto por painel (último `pane-context`); ausente = sem medida */
  paneCtx: Record<string, { pct: number; tokens: number | null; janela: number | null }>;
  /** proposta de compactação por painel (chip do medidor); ausente = nenhuma */
  compactPropostas: Record<string, { acao: string; motivo: string }>;
  /** último painel de terminal com foco: destino da voz (ditado) */
  activePane: string | null;
  /** configuração de voz (voice.json); null até carregar */
  voice: VoiceConfig | null;
  /** ações pedidas pelo celular aguardando confirmação, na ordem em que chegaram (só na janela principal); a primeira aparece */
  jarvisConfirms: JarvisConfirm[];
  /** build do Windows para o windowsPty do xterm */
  windowsBuild: number;
  providers: ProvidersConfig | null;
  detected: Detected[];
  settings: Settings | null;
  /** total de todo o histórico, e hoje e os 7 dias (mesma atualização de 2 s) */
  totals: Spend & { today: Spend; week: Spend };
  /** leitura dos limites das contas (Redline); null até a primeira leitura */
  limits: RPayload | null;
  toasts: Toast[];
  /** sino: os últimos avisos, mais novo primeiro */
  toastLog: Toast[];
  restore: Record<string, PaneRow[]>;
  /** "Escolher…" do banner de restauração: os cartões por missão aparecem em cada workspace */
  restoreDetail: boolean;
  /** "Agora não" do aviso: esconde o aviso nesta sessão do app; os painéis continuam restauráveis no banco */
  restoreDismissed: boolean;
  search: Record<string, string>;
  detached: Record<string, boolean>;
  settingsOpen: boolean;
  /** o assistente de boas-vindas aberto por Configurações (mesmo com o onboarding concluído) */
  onboardingOpen: boolean;
  dialog: Dialog;
  /** missão selecionada no rail (filtra a grade) por workspace */
  missionSel: Record<string, string | null>;
  /** custo agregado da missão (inclui painéis fechados), por id da missão */
  /** tokens = entrada+saída; cacheTokens = cache lido+escrito (não entra no total, só na dica) */
  missionTotals: Record<string, { tokens: number; cacheTokens: number; costUsd: number }>;
  /** entregas (handoffs) ainda não vistas, por id da missão */
  unseen: Record<string, number>;
  /** atenção de cada missão, por id da missão (bloqueada, pronta, motivos) */
  attention: Record<string, MissionAttention>;
  /** gates abertos de todos os workspaces, do mais antigo para o mais novo */
  gates: GateOpen[];
  /** o diálogo completo do gate está aberto (senão ele aparece só como faixa no topo) */
  gateOpen: boolean;
  /** paleta de comandos aberta: "all" (ações) ou "files" (Ctrl+P, arquivos); null = fechada */
  palette: null | "all" | "files";
  /** seção que as Configurações abrem (ação da paleta); null = a primeira */
  settingsSection: string | null;
  /** galeria de capturas do Shot aberta (botão SHOT ou paleta) */
  shotGallery: boolean;
  /** workspace cujas sessões anteriores estão no diálogo "Retomar sessão anterior"; null = fechado */
  sessionPicker: string | null;
  /** painel cuja conversa está sendo copiada/salva (Markdown); null = fechado */
  exportPane: string | null;
  /** "Enviar ao agente": texto a colar num painel de IA da missão (o diálogo escolhe o painel) */
  sendTo: SendTo | null;
  /** últimos erros detectados na saída de cada terminal (anel de 5, o mais recente por último) */
  paneErrors: Record<string, ErrorBlock[]>;
  /** compositor aberto por painel de IA (Ctrl+Shift+Enter ou o ⋯ do painel) */
  composerOpen: Record<string, boolean>;
  /** gaveta de atividade (subtarefas e comandos) aberta por painel do Claude */
  activityOpen: Record<string, boolean>;
  /** modo e tamanhos da grade por workspace (lib/layout); ausente = o salvo no armazenamento local */
  gridPrefs: Record<string, GridPrefs>;
  /** largura da árvore de arquivos; null = a salva */
  shellPrefs: ShellPrefs | null;
  /** envios anteriores do compositor por painel (↑/↓ recupera; no máximo 50) */
  composerHist: Record<string, string[]>;
  /** busca em todos os painéis aberta (Ctrl+Shift+F) */
  paneSearch: boolean;
  /** painel cujo modelo/esforço está sendo trocado (chip do cabeçalho ou paleta); null = fechado */
  modelPane: string | null;
  /** missão cujas mudanças estão sendo revisadas (Revisar mudanças); null = fechado */
  reviewMission: string | null;
  /** diálogo de commit ou de PR aberto, no workspace (e na missão, se houver); null = fechado */
  gitDialog: { kind: "commit" | "pr"; wsId: string; missionId: string | null } | null;
  /** Monitor de recursos (RAM/CPU por painel) aberto */
  resourcesOpen: boolean;
  /** arquivos aceitos na revisão, por id da missão (em memória: some ao fechar o Regente) */
  reviewed: Record<string, Accepted>;
  /** brief vindo da paleta (missão a partir da área de transferência); o wizard consome e limpa */
  missionPrefill: { mode: "squad" | "agentico"; brief: string; style?: "plano" } | null;
  // ▼ trilha A
  /** alvo do diálogo "Verificação…" da missão */
  verifyMission: string | null;
  // ▲ trilha A
  // ▼ trilha B
  /** quando cada painel entrou no status atual (ms epoch); o cabeçalho mostra o tempo de "Trabalhando" */
  statusSince: Record<string, number>;
  /** painel que recebe o ditado enquanto a voz grava (o cabeçalho dele fica com a borda vermelha) */
  voiceTarget: string | null;
  /** linha do tempo de checkpoints aberta: escopo (missão ou workspace) e título para o cabeçalho */
  timeline: { scope: string; title: string; mission: boolean } | null;
  // ▲ trilha B
  // ▼ trilha C
  /** "Comparar abordagens" (A1c): workspace e grupo aberto (groupId null = formulário); null = fechado */
  compare: { workspaceId: string; groupId: string | null } | null;
  /** "Enquanto você estava fora" (A1.6): desde quando (ms) o digest aparece; null = fechado */
  digestSince: number | null;
  /** servidores de desenvolvimento anunciados pelos painéis (A1.7, RM-44): um por painel */
  devServers: DevMap;
  /** quando a janela perdeu o foco (ms); o digest abre ao voltar depois do limite de ausência */
  blurredAt: number | null;
  // ▲ trilha C
}

interface Actions {
  boot(openStartup?: boolean): Promise<void>;
  openWorkspace(path: string, quiet?: boolean): Promise<Workspace | null>;
  openPath(path: string): Promise<void>;
  /** fecha a aba; pergunta antes se o editor dela tem alterações não salvas. false = cancelado */
  closeTab(id: string): Promise<boolean>;
  setActive(id: string | null): void;
  loadPanes(ws: string): Promise<void>;
  loadMissions(ws: string): Promise<void>;
  upsertPane(p: PaneRow): void;
  patchPane(id: string, p: Partial<PaneRow>): void;
  /** modo sensível de um workspace (sem segredo do cofre nos painéis dele) */
  setWorkspaceSensitive(ws: string, on: boolean): Promise<void>;
  groupCreate(ws: string): Promise<void>;
  groupPatch(ws: string, id: string, patch: { name?: string; collapsed?: boolean }): Promise<void>;
  groupDelete(ws: string, id: string): Promise<void>;
  /** grupo criado pelo app (subagente): entra ou atualiza na lista do workspace, sem chamada */
  groupSync(ws: string, group: PaneGroup): void;
  /** põe o painel num grupo do workspace (null tira do grupo) */
  paneSetGroup(paneId: string, groupId: string | null): Promise<void>;
  removePane(id: string): void;
  setSlot(e: SlotEvent): void;
  toast(text: string, kind?: Toast["kind"], action?: Toast["action"], ms?: number): void;
  /** Erro do backend: a ação vem do código (`budget_exceeded`, `bad_cwd`), nunca do texto. */
  toastErr(e: unknown): void;
  dismiss(id: number): void;
  clearToastLog(): void;
  refreshTotals(): Promise<void>;
  refreshLimits(): Promise<void>;
  saveSettings(s: Settings): Promise<void>;
  patch(p: Partial<State>): void;
  refreshMissionTotals(ws: string): Promise<void>;
  loadUnseen(ws: string): Promise<void>;
  /** atenção das missões do workspace (o evento `mission-state` mantém em dia) */
  loadAttention(ws: string): Promise<void>;
  setAttention(a: MissionAttention): void;
  /** seleciona a missão no rail; marca as entregas dela como vistas */
  selectMission(ws: string, id: string | null): void;
  /** entrega nova: recarrega as pendentes; avisa (só na janela principal) quando a missão não está aberta */
  onHandoff(missionId: string, summary: string, notify: boolean): void;
  loadGates(): Promise<void>;
  addGate(g: GateOpen): void;
  removeGate(id: string): void;
  /** restaura (accept) ou descarta os painéis pendentes de todos os workspaces, de uma vez */
  decideRestore(accept: boolean): Promise<void>;
  /** descarta (destrutivo, com confirmação) os painéis pendentes de todos os workspaces */
  discardRestore(): Promise<void>;
}

let toastSeq = 1;
let sessionWatched = false;
/** salva abas, missão, visão e rail sempre que mudam (só na janela principal, depois de restaurar) */
function watchSession() {
  if (sessionWatched) return;
  sessionWatched = true;
  useStore.subscribe((s, p) => {
    if (s.tabs === p.tabs && s.active === p.active && s.missionSel === p.missionSel && s.view === p.view) return;
    writeSession({ tabs: s.tabs, active: s.active, missionSel: s.missionSel, view: s.view });
  });
}
/** só a gravação de configurações mais recente desfaz a tela quando falha */
let settingsSeq = 0;
/** última carga (ou mudança) da atenção por workspace: resposta antiga não sobrescreve a mais nova */
const attentionSeq = new Map<string, number>();

export const useStore = create<State & Actions>((set, get) => ({
  workspaces: [], tabs: [], active: null, panes: {}, groups: {}, missions: {}, slots: {}, focus: {}, view: {}, openFile: {}, dirty: {}, editorStatus: {},
  readyUrl: {}, paneCtx: {}, compactPropostas: {}, activePane: null, voice: null, jarvisConfirms: [], windowsBuild: 22000, providers: null, detected: [], settings: null, totals: { tokens: 0, costUsd: 0, today: { tokens: 0, costUsd: 0 }, week: { tokens: 0, costUsd: 0 } }, limits: null, toasts: [], toastLog: [],
  restore: {}, restoreDetail: false, restoreDismissed: false, search: {}, detached: {}, settingsOpen: false, onboardingOpen: false, dialog: null, missionSel: {},
  missionTotals: {}, unseen: {}, attention: {}, gates: [], gateOpen: false, palette: null, settingsSection: null, shotGallery: false, sessionPicker: null, exportPane: null, sendTo: null, paneErrors: {}, composerOpen: {}, activityOpen: {}, gridPrefs: {}, shellPrefs: null, composerHist: {}, paneSearch: false, modelPane: null, missionPrefill: null, reviewMission: null, gitDialog: null, resourcesOpen: false, reviewed: {},
  // ▼ trilha A
  verifyMission: null,
  // ▲ trilha A
  // ▼ trilha B
  statusSince: {},
  voiceTarget: null,
  timeline: null,
  // ▲ trilha B
  // ▼ trilha C
  compare: null,
  digestSince: null,
  devServers: {},
  blurredAt: null,
  // ▲ trilha C

  async boot(openStartup = true) {
    api.appInfo().then((i) => { if (i.windowsBuild) set({ windowsBuild: i.windowsBuild }); }).catch(() => {});
    const [workspaces, providers, settings, detected] = await Promise.all([
      api.workspaceList(), api.providersGet(), api.settingsGet(), api.providersDetect(),
    ]);
    document.documentElement.dataset.theme = settings.theme;
    set({ workspaces, providers, settings, detected });
    get().refreshTotals().catch(() => {});
    get().loadGates().catch((e) => get().toast(errMsg(e), "erro"));
    if (!openStartup) return;
    const saved = readSession();
    if (saved) {
      // as abas da véspera: as de workspace que sumiu (ou que não abre) ficam de fora
      const known = new Set(workspaces.map((w) => w.id));
      for (const id of saved.tabs.filter((t) => known.has(t))) {
        const ws = workspaces.find((w) => w.id === id);
        if (ws) await get().openWorkspace(ws.path, true);
      }
      const tabs = get().tabs;
      const missionSel: Record<string, string | null> = {};
      for (const [ws, id] of Object.entries(saved.missionSel)) {
        if (tabs.includes(ws) && (get().missions[ws] ?? []).some((m) => m.id === id)) missionSel[ws] = id;
      }
      const active = saved.active === null ? null : tabs.includes(saved.active) ? saved.active : (tabs[tabs.length - 1] ?? null);
      set({ active, missionSel, view: saved.view });
      if (settings.restoreOnOpen) await get().decideRestore(true);
    }
    watchSession();
    const p = await api.startupPath();
    if (p) await get().openPath(p);
  },

  async decideRestore(accept) {
    // "Agora não" (accept false) só esconde o aviso nesta sessão: nada é fechado nem apagado
    if (!accept) {
      set({ restore: {}, restoreDetail: false, restoreDismissed: true });
      return;
    }
    const pending = get().restore;
    const left: Record<string, PaneRow[]> = {};
    for (const wsId of Object.keys(pending)) {
      try {
        for (const r of await api.restoreApply(wsId, true)) get().upsertPane(r);
      } catch (e) {
        left[wsId] = pending[wsId];
        get().toast(errMsg(e), "erro");
      }
    }
    set({ restore: left, restoreDetail: Object.keys(left).length > 0 && get().restoreDetail });
  },

  async discardRestore() {
    const pending = get().restore;
    const n = Object.values(pending).reduce((a, rows) => a + rows.length, 0);
    if (!n) return;
    if (!(await ask(`Descartar ${n} ${n === 1 ? "painel" : "painéis"} da sessão anterior? Eles não serão restaurados.`, { title: "Descartar restauração", kind: "warning" }))) return;
    const left: Record<string, PaneRow[]> = {};
    for (const wsId of Object.keys(pending)) {
      try {
        await api.restoreApply(wsId, false);
      } catch (e) {
        left[wsId] = pending[wsId];
        get().toast(errMsg(e), "erro");
      }
    }
    set({ restore: left, restoreDetail: Object.keys(left).length > 0 && get().restoreDetail });
  },

  async openWorkspace(path, quiet = false) {
    try {
      const ws = await api.workspaceOpen(path);
      set((s) => ({
        workspaces: [ws, ...s.workspaces.filter((w) => w.id !== ws.id)],
        tabs: s.tabs.includes(ws.id) ? s.tabs : [...s.tabs, ws.id],
        active: ws.id,
      }));
      await Promise.all([get().loadPanes(ws.id), get().loadMissions(ws.id)]);
      get().loadUnseen(ws.id).catch((e) => get().toast(errMsg(e), "erro"));
      const pending = await api.restoreList(ws.id);
      if (pending.length && !get().restoreDismissed) set((s) => ({ restore: { ...s.restore, [ws.id]: pending } }));
      return ws;
    } catch (e) {
      if (!quiet) get().toast(errMsg(e), "erro");
      return null;
    }
  },

  async openPath(path) {
    if (await get().openWorkspace(path, true)) return;
    const parent = path.replace(/[\\/][^\\/]+$/, "");
    const ws = await get().openWorkspace(parent);
    if (ws) set((s) => ({ openFile: { ...s.openFile, [ws.id]: { path } }, view: { ...s.view, [ws.id]: "arquivos" } }));
  },

  async closeTab(id) {
    const name = get().workspaces.find((w) => w.id === id)?.name ?? "";
    if (get().dirty[id]?.length && !(await ask(`Há alterações não salvas no editor de ${name}. Fechar e descartá-las?`, { title: "Alterações não salvas", kind: "warning" }))) return false;
    set((s) => {
      const tabs = s.tabs.filter((t) => t !== id);
      return { tabs, active: s.active === id ? (tabs[tabs.length - 1] ?? null) : s.active };
    });
    return true;
  },

  setActive(id) { set({ active: id }); },

  async loadPanes(ws) {
    const [rows, groups] = await Promise.all([api.paneList(ws), api.paneGroupList(ws)]);
    set((s) => {
      const panes = Object.fromEntries(Object.entries(s.panes).filter(([, p]) => p.workspaceId !== ws));
      for (const r of rows) panes[r.id] = r;
      return { panes, groups: { ...s.groups, [ws]: groups } };
    });
  },

  async setWorkspaceSensitive(ws, on) {
    const running = await api.workspaceSensitiveSet(ws, on);
    const cur = get().settings;
    if (cur) {
      const rest = cur.sensitiveWorkspaces.filter((w) => w !== ws);
      set({ settings: { ...cur, sensitiveWorkspaces: on ? [...rest, ws] : rest } });
    }
    // painéis com rota que usa o Cofre seguem rodando com o segredo: avisa cada um, com "fechar"
    for (const id of running) {
      const name = get().panes[id]?.name ?? "Painel";
      get().toast(`${name} usa segredo do Cofre e não foi reiniciado. Feche-o para o modo sensível valer.`, "erro", { label: "Fechar", run: () => { api.paneClose(id).catch((e) => get().toast(errMsg(e), "erro")); } });
    }
  },

  async groupCreate(ws) {
    const n = (get().groups[ws] ?? []).length + 1;
    const g = await api.paneGroupCreate(ws, `Grupo ${n}`);
    set((s) => ({ groups: { ...s.groups, [ws]: [...(s.groups[ws] ?? []), g] } }));
  },

  async groupPatch(ws, id, patch) {
    await api.paneGroupUpdate(id, patch);
    set((s) => ({ groups: { ...s.groups, [ws]: (s.groups[ws] ?? []).map((g) => (g.id === id ? { ...g, ...patch } : g)) } }));
  },

  groupSync(ws, group) {
    set((s) => ({ groups: { ...s.groups, [ws]: [...(s.groups[ws] ?? []).filter((g) => g.id !== group.id), group] } }));
  },

  async groupDelete(ws, id) {
    await api.paneGroupDelete(id);
    set((s) => ({
      groups: { ...s.groups, [ws]: (s.groups[ws] ?? []).filter((g) => g.id !== id) },
      panes: Object.fromEntries(Object.entries(s.panes).map(([k, p]) => [k, p.groupId === id ? { ...p, groupId: null } : p])),
    }));
  },

  async paneSetGroup(paneId, groupId) {
    await api.paneSetGroup(paneId, groupId);
    get().patchPane(paneId, { groupId });
  },

  async loadMissions(ws) {
    const list = await api.missionList(ws);
    set((s) => ({ missions: { ...s.missions, [ws]: list } }));
    get().refreshMissionTotals(ws).catch((e) => get().toast(errMsg(e), "erro"));
    get().loadAttention(ws).catch((e) => get().toast(errMsg(e), "erro"));
  },

  async loadAttention(ws) {
    const seq = (attentionSeq.get(ws) ?? 0) + 1;
    attentionSeq.set(ws, seq);
    const list = await api.missionAttention(ws);
    // resposta antiga: outra carga ou um mission-state novo deste workspace chegou depois
    if (attentionSeq.get(ws) !== seq) return;
    set((s) => {
      const attention = Object.fromEntries(Object.entries(s.attention).filter(([, a]) => a.workspaceId !== ws));
      for (const a of list) if (!isCleared(a)) attention[a.missionId] = a;
      return { attention };
    });
  },

  setAttention(a) {
    attentionSeq.set(a.workspaceId, (attentionSeq.get(a.workspaceId) ?? 0) + 1);
    set((s) => {
      const attention = { ...s.attention };
      if (isCleared(a)) delete attention[a.missionId];
      else attention[a.missionId] = a;
      return { attention };
    });
  },

  async refreshMissionTotals(ws) {
    const rows = await api.missionTotals(ws);
    set((s) => ({ missionTotals: { ...s.missionTotals, ...Object.fromEntries(rows.map((r) => [r.missionId, { tokens: r.tokens, cacheTokens: r.cacheTokens, costUsd: r.costUsd }])) } }));
  },

  async loadUnseen(ws) {
    const rows = await api.unseenHandoffs(ws);
    set((s) => {
      const unseen = { ...s.unseen };
      for (const m of s.missions[ws] ?? []) delete unseen[m.id];
      for (const r of rows) unseen[r.missionId] = r.count;
      return { unseen };
    });
  },

  selectMission(ws, id) {
    set((s) => ({ missionSel: { ...s.missionSel, [ws]: id } }));
    if (!id || !get().unseen[id]) return;
    api.missionMarkSeen(id).then(() => get().loadUnseen(ws)).catch((e) => get().toast(errMsg(e), "erro"));
  },

  onHandoff(missionId, summary, notify) {
    const st = get();
    const ws = Object.keys(st.missions).find((w) => st.missions[w].some((m) => m.id === missionId));
    if (ws && st.missionSel[ws] === missionId) {
      api.missionMarkSeen(missionId).then(() => get().loadUnseen(ws)).catch((e) => get().toast(errMsg(e), "erro"));
      return;
    }
    if (ws) get().loadUnseen(ws).catch((e) => get().toast(errMsg(e), "erro"));
    if (notify) get().toast(`Entrega pronta: ${summary.length > 80 ? `${summary.slice(0, 79)}…` : summary}`, "ok", ws ? { label: "Ler", run: () => get().selectMission(ws, missionId) } : undefined, 8000);
  },

  async loadGates() {
    set({ gates: await api.gatesOpen() });
  },

  addGate(g) {
    set((s) => (s.gates.some((x) => x.id === g.id) ? {} : { gates: [...s.gates, g] }));
  },

  removeGate(id) {
    set((s) => ({ gates: s.gates.filter((g) => g.id !== id) }));
  },

  upsertPane(p) {
    set((s) => {
      const moved = !s.panes[p.id] || s.panes[p.id].status !== p.status;
      return { panes: { ...s.panes, [p.id]: p }, ...(moved ? { statusSince: { ...s.statusSince, [p.id]: Date.now() } } : {}) };
    });
  },

  patchPane(id, p) {
    set((s) => {
      const cur = s.panes[id];
      if (!cur) return {};
      const moved = p.status !== undefined && p.status !== cur.status;
      return { panes: { ...s.panes, [id]: { ...cur, ...p } }, ...(moved ? { statusSince: { ...s.statusSince, [id]: Date.now() } } : {}) };
    });
  },

  removePane(id) {
    set((s) => {
      const panes = { ...s.panes };
      delete panes[id];
      const search = { ...s.search };
      delete search[id];
      const detached = { ...s.detached };
      delete detached[id];
      const focus = Object.fromEntries(Object.entries(s.focus).map(([ws, f]) => [ws, f === id ? null : f]));
      // o destino da voz some junto com o painel
      const activePane = s.activePane === id ? null : s.activePane;
      const paneErrors = { ...s.paneErrors };
      delete paneErrors[id];
      const composerOpen = { ...s.composerOpen };
      delete composerOpen[id];
      const composerHist = { ...s.composerHist };
      delete composerHist[id];
      forgetPaneErrors(id);
      // o servidor de dev some junto com o painel (A1.7)
      return { panes, search, detached, focus, activePane, paneErrors, composerOpen, composerHist, devServers: remover(s.devServers, id) };
    });
  },

  setSlot(e) {
    if (e.state === "done") {
      setTimeout(() => set((s) => {
        const slots = { ...s.slots };
        delete slots[e.missionId];
        return { slots };
      }), 3000);
      return;
    }
    if (e.index === undefined) return;
    set((s) => ({ slots: { ...s.slots, [e.missionId]: { ...s.slots[e.missionId], [e.index!]: { state: e.state, paneId: e.paneId, reason: e.reason } } } }));
  },

  toast(text, kind = "info", action, ms) {
    const id = toastSeq++;
    const t: Toast = { id, kind, text, action, ms: ms ?? (kind === "erro" ? 9000 : 5000) };
    set((s) => ({ toasts: [...s.toasts, t], toastLog: pushLog(s.toastLog, t) }));
  },

  toastErr(e) {
    const code = errCode(e);
    if (code === "shutting_down") return get().toast("O Regente está fechando; tente de novo depois de abrir.", "info");
    const action = code === "budget_exceeded"
      ? { label: "Ajustar orçamento", run: () => get().patch({ settingsOpen: true, settingsSection: "uso" }) }
      : code === "bad_cwd"
        ? { label: "Escolher pasta", run: () => get().patch({ dialog: "painel" }) }
        : undefined;
    get().toast(errMsg(e), "erro", action);
  },

  dismiss(id) { set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })); },

  clearToastLog() { set({ toastLog: [] }); },

  async refreshTotals() {
    const [all, rows] = await Promise.all([api.usageTotals(), api.usageRows(sinceDay("7"))]);
    const today = sinceDay("hoje");
    const sum = (rs: UsageRow[]): Spend => rs.reduce((a, r) => ({ tokens: a.tokens + r.tokIn + r.tokOut, costUsd: a.costUsd + r.costUsd }), { tokens: 0, costUsd: 0 });
    set({ totals: { ...all, today: sum(rows.filter((r) => r.day === today)), week: sum(rows) } });
  },

  async refreshLimits() { set({ limits: await api.redlineCurrent() }); },

  async saveSettings(st) {
    // otimista: aplica já e desfaz se o backend recusar
    const prev = get().settings;
    const my = ++settingsSeq;
    const apply = (x: Settings | null) => {
      if (x) document.documentElement.dataset.theme = x.theme;
      set({ settings: x });
    };
    apply(st);
    try {
      await api.settingsSet(st);
    } catch (e) {
      if (my === settingsSeq) apply(prev);
      throw e;
    }
  },

  patch(p) { set(p); },
}));

export const wsOf = (id: string | null) => useStore.getState().workspaces.find((w) => w.id === id) ?? null;
