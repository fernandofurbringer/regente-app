import { useEffect, useMemo, useRef, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { filterByQuery } from "../lib/fuzzy";
import { isRemote } from "../lib/remote";
import { lastPickFor } from "../lib/lastAgent";
import { lastAgentReply } from "../lib/lastReply";
import { api, errMsg, type FsEntry, type PaneRow, type PromptDef, type RecipeDef } from "../lib/ipc";
import { parseHotkey } from "../lib/hotkey";
import { MODE_LABEL, SHELL_DEFAULT, comArranjo, nextMode, type GridMode, type GridPrefs } from "../lib/layout";
import { gridPrefsOf, setGridPrefs, setShellPrefs } from "../lib/layoutStore";
import { isStack, stackFromPanes, stackMissionInput } from "../lib/stacks";
import { copyDefers, interceptsIn, matchAction, resolveShortcuts } from "../lib/shortcuts";
import { closeQuestion, waitingPanes } from "../lib/attention";
import { isInlineSubagent } from "../lib/activity";
import { shownPanes } from "../lib/visible";
import { useStore, wsOf } from "../state/store";
import { askFromEditor, selectionSendTo, VERB_ORDER, verbLabel, type AgentVerb } from "../lib/askAgent";
import { focusPane, goToPane } from "../lib/focus";
import Modal from "./Modal";
import { terms } from "./TerminalPane";
import { insertIntoActive } from "./PromptPicker";
import { continueLastSession } from "./SessionPicker";
// ▼ trilha B
import { checkpointAgora } from "../lib/checkpointTrigger";
// ▲ trilha B
import { SETTINGS_SECTIONS, spawnInActive } from "./settings/Settings";

const SKIP_DIRS = new Set(["node_modules", "target", "dist", "build", "__pycache__"]);
const FILE_LIMIT = 3000;
/** Pastas visitadas ao listar arquivos (a árvore de um monorepo não pesa a abertura da paleta). */
const DIR_LIMIT = 500;
const SHOWN = 60;

interface Item { id: string; label: string; group: string; keys?: string; run(): void | Promise<void> }

const toast = (text: string) => useStore.getState().toast(text, "erro");

/** Roda a ação com o painel ativo; sem painel ativo, avisa. Erros viram toast. */
function comPainelAtivo(f: (p: PaneRow) => Promise<unknown>) {
  const st = useStore.getState();
  const p = st.activePane ? st.panes[st.activePane] : undefined;
  if (!p) return toast("Nenhum painel ativo.");
  f(p).catch((e) => toast(errMsg(e)));
}
const wsPanes = (ws: string | null): PaneRow[] => {
  const st = useStore.getState();
  const sel = ws ? st.missionSel[ws] : null;
  return Object.values(st.panes)
    .filter((p) => p.workspaceId === ws && (!sel || p.missionId === sel))
    .sort((a, b) => a.ord - b.ord);
};

// goToPane e focusPane moram em lib/focus.ts (a janela avulsa de painel importa só isso, sem a paleta)
export { goToPane };

function focusIndex(n: number) {
  const panes = shownPanes(useStore.getState().active ?? "");
  const p = panes[n - 1];
  if (p) focusPane(p);
  else toast(`Não há painel ${n} neste workspace.`);
}

/** Próximo (+1) ou anterior (-1) painel do workspace; sem painel ativo, o primeiro (ou o último). */
function stepPane(delta: 1 | -1) {
  const st = useStore.getState();
  const panes = shownPanes(st.active ?? "");
  if (!panes.length) return toast("Não há painéis neste workspace.");
  const cur = panes.findIndex((p) => p.id === st.activePane);
  const next = cur < 0 ? (delta > 0 ? 0 : panes.length - 1) : (cur + delta + panes.length) % panes.length;
  focusPane(panes[next]);
}

/** Ctrl+Shift+A: o painel que espera você há mais tempo (pergunta, erro ou gate). Repetir avança para o próximo. */
export function goToAttention() {
  const st = useStore.getState();
  const list = waitingPanes(Object.values(st.panes).filter((p) => !isInlineSubagent(p, st.settings?.subagentPanes ?? false)), st.statusSince, st.gates);
  if (!list.length) return st.toast("Nenhum painel aguardando você.", "ok");
  const cur = list.findIndex((p) => p.id === st.activePane);
  const target = list[(cur + 1) % list.length];
  // o painel precisa estar à vista: sai do filtro de missão e abre o grupo recolhido dele
  const ws = target.workspaceId;
  if (st.active !== ws) st.setActive(ws);
  if (target.missionId && st.missionSel[ws] !== target.missionId) st.selectMission(ws, target.missionId);
  const group = st.groups[ws]?.find((g) => g.id === target.groupId);
  if (group?.collapsed) void st.groupPatch(ws, group.id, { collapsed: false }).catch((e) => toast(errMsg(e)));
  goToPane(target.id);
}

function newTerminal() {
  if (!useStore.getState().active) return toast("Abra um workspace primeiro.");
  spawnInActive({ kind: "terminal" }).catch(() => {});
}

/** Mesmo CLI, modelo, esforço e conta do último painel de IA criado no workspace; sem nenhum, abre o formulário. */
function newPanel() {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  if (!ws) return toast("Abra um workspace primeiro.");
  const last = Object.values(st.panes).filter((p) => p.workspaceId === ws.id && p.kind === "agent").sort((a, b) => b.createdAt - a.createdAt)[0];
  if (!last) return st.patch({ dialog: "painel" });
  // o modelo e o esforço da última vez que essa IA foi usada (troca de modelo inclusa); senão os do último painel
  const pick = lastPickFor(last.cli);
  spawnInActive({ kind: "agent", cli: last.cli, model: pick ? pick.model : last.model, effort: pick ? pick.effort : last.effort, account: last.account, route: last.route, permission: last.permission }).catch(() => {});
}

/** Missão nova com o brief da área de transferência (modo agêntico, que tem brief). */
async function missionFromClipboard() {
  const st = useStore.getState();
  if (!st.active) return toast("Abra um workspace primeiro.");
  let text = "";
  try {
    text = (await navigator.clipboard.readText()).trim();
  } catch {
    return toast("Não foi possível ler a área de transferência.");
  }
  if (!text) return toast("A área de transferência está vazia.");
  st.patch({ missionPrefill: { mode: "agentico", brief: text }, dialog: "missao" });
}

/** Modelo e esforço do painel de IA ativo (diálogo do chip do cabeçalho). */
function switchModelActive() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active || pane.kind !== "agent") return toast("Clique num painel de IA primeiro.");
  st.patch({ modelPane: pane.id });
}

// ▼ trilha B
/** Escopo dos checkpoints: a missão do painel ativo (se tem worktree), senão o workspace. */
function checkpointScope(): { scope: string; title: string; mission: boolean } | null {
  const st = useStore.getState();
  if (!st.active) return null;
  const pane = st.panes[st.activePane ?? ""];
  const m = pane?.missionId ? Object.values(st.missions).flat().find((x) => x.id === pane.missionId) : undefined;
  if (m?.worktreePath) return { scope: m.id, title: m.name, mission: true };
  return { scope: st.active, title: wsOf(st.active)?.name ?? "Workspace", mission: false };
}

function checkpointNow() {
  const sc = checkpointScope();
  if (!sc) return toast("Abra um workspace primeiro.");
  checkpointAgora(sc.scope);
}

function linhaDoTempo() {
  const sc = checkpointScope();
  if (!sc) return toast("Abra um workspace primeiro.");
  useStore.getState().patch({ timeline: sc });
}
// ▲ trilha B

/** Revisão das mudanças da missão escolhida no rail: aceitar ou reverter arquivo a arquivo. */
function reviewMission() {
  const st = useStore.getState();
  const id = st.active ? st.missionSel[st.active] : null;
  const m = id ? (st.missions[st.active!] ?? []).find((x) => x.id === id) : undefined;
  if (!m) return toast("Escolha uma missão no painel de missões primeiro.");
  if (!m.worktreePath) return toast("Esta missão não usa worktree: não há mudanças para revisar.");
  st.patch({ reviewMission: m.id });
}

/** Commit ou PR no alvo escolhido: a missão selecionada no rail, ou o workspace. */
function gitAction(kind: "commit" | "pr") {
  const st = useStore.getState();
  if (!st.active) return toast("Abra um workspace primeiro.");
  const missionId = st.missionSel[st.active] ?? null;
  const m = missionId ? (st.missions[st.active] ?? []).find((x) => x.id === missionId) : undefined;
  if (missionId && m?.worktreePath === undefined) return toast("Esta missão não usa worktree.");
  st.patch({ gitDialog: { kind, wsId: st.active, missionId: missionId && m ? m.id : null } });
}

/** Missão agêntica no estilo Plano → Execução (o wizard abre com ele marcado). */
function newMissionPlan() {
  if (!useStore.getState().active) return toast("Abra um workspace primeiro.");
  useStore.getState().patch({ missionPrefill: { mode: "agentico", brief: "", style: "plano" }, dialog: "missao" });
}

function newMission() {
  if (!useStore.getState().active) return toast("Abra um workspace primeiro.");
  useStore.getState().patch({ dialog: "missao" });
}

/** Liga ou desliga o modo sensível do workspace ativo (o cofre passa a mascarar os valores dele na tela de detalhes). */
async function toggleSensitive() {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  if (!ws) return toast("Abra um workspace primeiro.");
  const on = !(st.settings?.sensitiveWorkspaces ?? []).includes(ws.id);
  await st.setWorkspaceSensitive(ws.id, on).then(
    () => st.toast(on ? "Modo sensível ligado neste workspace." : "Modo sensível desligado neste workspace.", "ok"),
    (e) => st.toast(errMsg(e), "erro"),
  );
}

async function newGroup() {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  if (!ws) return toast("Abra um workspace primeiro.");
  await st.groupCreate(ws.id).catch((e) => st.toast(errMsg(e), "erro"));
  st.toast("Grupo criado. Arraste painéis para o cabeçalho dele.", "ok");
}

function searchPanes() {
  if (!useStore.getState().active) return toast("Abra um workspace primeiro.");
  useStore.getState().patch({ paneSearch: true });
}

/** Copia só a última resposta do agente do painel ativo (Ctrl+Shift+Y e a paleta). */
async function copyLastReply() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active || pane.kind !== "agent") return toast("Clique num painel de IA primeiro.");
  // só o transcrito do Claude Code tem turnos; outros CLIs guardam só o texto do terminal
  if (pane.cli !== "claude") return toast("Disponível só para Claude Code");
  try {
    const reply = lastAgentReply(await api.paneTranscript(pane.id, false, null));
    if (!reply) return toast("Ainda não há resposta do agente neste painel.");
    await navigator.clipboard.writeText(reply);
    st.toast("Resposta copiada", "ok");
  } catch (e) {
    st.toastErr(e);
  }
}

/** Copiar/salvar a conversa do painel ativo (Ctrl+Shift+C e a paleta). */
function copyConversation() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active || (pane.kind !== "agent" && pane.kind !== "terminal")) return toast("Clique num painel de IA ou terminal primeiro.");
  st.patch({ exportPane: pane.id });
}

async function closeActive() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active) return toast("Clique num painel para escolhê-lo primeiro.");
  const q = closeQuestion(pane);
  if (q && !(await ask(q, { title: "Fechar painel", kind: "warning" }))) return;
  await api.paneClose(pane.id).catch((e) => toast(errMsg(e)));
}

/** Mostra de novo o aviso de restauração (o "Agora não" o escondia só nesta sessão) e relê os painéis salvos. */
async function restoreLastSession() {
  const st = useStore.getState();
  st.patch({ restoreDismissed: false });
  let n = 0;
  for (const ws of st.workspaces) {
    const pending = await api.restoreList(ws.id).catch(() => []);
    if (!pending.length) continue;
    n += pending.length;
    useStore.getState().patch({ restore: { ...useStore.getState().restore, [ws.id]: pending } });
  }
  if (!n) useStore.getState().toast("Nenhum painel da última sessão para restaurar.", "info");
}

async function hibernateActive() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active) return toast("Clique num painel para escolhê-lo primeiro.");
  await api.paneHibernate(pane.id, terms.get(pane.id)?.screen() ?? "").catch((e) => toast(errMsg(e)));
}

/** "Enviar seleção ao agente…": o texto selecionado no terminal ativo vai para o diálogo de envio. */
function sendTerminalSelection() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active) return toast("Clique num terminal primeiro.");
  const sel = terms.get(pane.id)?.selection().trim() ?? "";
  if (!sel) return toast("Selecione um texto no terminal primeiro.");
  st.patch({ sendTo: selectionSendTo(pane, sel) });
}

/** Compositor do painel de IA ativo (Ctrl+Shift+Enter e a paleta). */
export function toggleComposerActive() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active || pane.kind !== "agent") return toast("Clique num painel de IA primeiro.");
  st.patch({ composerOpen: { ...st.composerOpen, [pane.id]: !st.composerOpen[pane.id] } });
}

/** Abre ou fecha a gaveta de atividade (subtarefas e comandos) do painel ativo. */
function toggleActivityActive() {
  const st = useStore.getState();
  const id = st.activePane;
  if (!id || !st.panes[id]) return toast("Nenhum painel ativo.");
  st.patch({ activityOpen: { ...st.activityOpen, [id]: !st.activityOpen[id] } });
}

/** Muda a grade do workspace ativo (bloco C); `aviso` mostra o resultado num toast. */
function gradeAtiva(f: (p: GridPrefs) => GridPrefs, aviso?: (p: GridPrefs) => string) {
  const ws = useStore.getState().active;
  if (!ws) return toast("Abra um workspace.");
  const next = f(gridPrefsOf(ws));
  setGridPrefs(ws, next);
  if (aviso) useStore.getState().toast(aviso(next), "info");
}
/** Ctrl+Shift+L: próximo modo da grade. */
export const cycleGridMode = () => gradeAtiva((p) => comArranjo(p, nextMode(p.modo)), (p) => `Grade: ${MODE_LABEL[p.modo]}`);
const gradeModo = (modo: GridMode) => () => gradeAtiva((p) => comArranjo(p, modo), (p) => `Grade: ${MODE_LABEL[p.modo]}`);
const gradeCols = (cols: GridPrefs["cols"]) => () => gradeAtiva((p) => comArranjo(p, "igual", cols));
function resetTamanhos() {
  gradeAtiva((p) => ({ ...p, colFr: [], rowPx: {} }));
  setShellPrefs({ ...SHELL_DEFAULT });
}

function askNowFromEditor(verb: AgentVerb) {
  const err = askFromEditor(verb);
  if (err) toast(err);
}

/** Ctrl+P e a ação "Abrir arquivo": a lista de arquivos do workspace. Local: uma chamada (cache no backend); remoto: varredura por pasta. */
/** Arquivos do workspace (ou de uma pasta remota). `truncated`: a lista é parcial. */
export async function listFiles(root: string): Promise<{ files: { label: string; path: string }[]; truncated: boolean }> {
  if (!isRemote(root)) {
    const r = await api.fsIndex(root);
    return { files: r.files.map((e) => ({ label: e.rel, path: e.path })), truncated: r.truncated };
  }
  const out: { label: string; path: string }[] = [];
  const queue: [string, string][] = [[root, ""]];
  let visited = 0;
  while (queue.length && out.length < FILE_LIMIT && visited < DIR_LIMIT) {
    visited++;
    const [dir, rel] = queue.shift()!;
    let entries: FsEntry[];
    try {
      entries = await api.fsList(dir);
    } catch {
      continue;
    }
    for (const e of entries) {
      const r = rel ? `${rel}/${e.name}` : e.name;
      if (!e.isDir) out.push({ label: r, path: e.path });
      else if (!e.name.startsWith(".") && !SKIP_DIRS.has(e.name)) queue.push([e.path, r]);
    }
  }
  return { files: out, truncated: out.length >= FILE_LIMIT };
}

function openFile(path: string) {
  const st = useStore.getState();
  if (!st.active) return;
  st.patch({ openFile: { ...st.openFile, [st.active]: { path } }, view: { ...st.view, [st.active]: "arquivos" } });
}

/** Salva os painéis de IA do workspace ativo como uma stack (receita de modo livre). */
async function saveStack() {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  const agents = wsPanes(st.active).filter((p) => p.kind === "agent");
  if (!ws || agents.length === 0) return toast("Nenhum painel de IA aberto para salvar.");
  const now = Date.now();
  const name = `Stack ${new Date(now).toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" })}`;
  const danger = agents.filter((p) => p.permission === "danger").length;
  const downgrade = danger ? ` ${danger} com "Perigo" foi salvo como "Padrão do CLI".` : "";
  try {
    await api.recipeSave(stackFromPanes(agents, name, now));
    st.toast(`Stack salva com ${agents.length} painel(is).${downgrade} Abra pela paleta com "Abrir stack".`, "ok");
  } catch (e) {
    toast(errMsg(e));
  }
}

/** Saída inteira do painel ativo (disco + memória) no editor de arquivos, como o item do menu do painel. */
async function historyActive() {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active) return toast("Clique num painel para escolhê-lo primeiro.");
  try {
    const text = await api.paneHistory(pane.id);
    st.patch({
      openFile: { ...st.openFile, [pane.workspaceId]: { path: `historico-${pane.id}.txt`, text } },
      view: { ...st.view, [pane.workspaceId]: "arquivos" },
    });
  } catch (e) {
    toast(errMsg(e));
  }
}

/** Sessões anteriores num diálogo, mesmo com painéis abertos. */
function resumeSession() {
  const st = useStore.getState();
  if (!st.active) return toast("Abra um workspace primeiro.");
  st.patch({ sessionPicker: st.active });
}

/** Abre todos os painéis de uma stack como uma missão livre no workspace ativo. */
async function openStack(r: RecipeDef) {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  if (!ws) return;
  try {
    await api.missionCreate(stackMissionInput(r, { id: ws.id, path: ws.path }));
    st.toast(`Stack "${r.name}" aberta.`, "ok");
  } catch (e) {
    toast(errMsg(e));
  }
}

/** Ações da paleta, com o estado atual. `files` entra como "arquivo: <caminho>". */
function buildItems(files: { label: string; path: string }[], prompts: PromptDef[], stacks: RecipeDef[]): Item[] {
  const st = useStore.getState();
  const keys = resolveShortcuts(st.settings?.shortcuts);
  const panes = shownPanes(st.active ?? "");
  const items: Item[] = [
    { id: "saveStack", label: "Salvar painéis abertos como stack", group: "Painéis", run: saveStack },
    { id: "paneHistory", label: "Abrir histórico completo (painel ativo)", group: "Painéis", run: historyActive },
    { id: "resumeSession", label: "Retomar sessão anterior…", group: "Painéis", run: resumeSession },
    { id: "continueLast", label: "Continuar a última conversa", group: "Painéis", run: () => continueLastSession(useStore.getState().active ?? "") },
    ...stacks.map((s): Item => ({ id: `stack-${s.id}`, label: `Abrir stack: ${s.name}`, group: "Stacks", run: () => openStack(s) })),
    { id: "mission", label: "Nova missão", group: "Missões", keys: keys.newMission, run: newMission },
    { id: "missionPlan", label: "Nova missão Plano → Execução", group: "Missões", run: newMissionPlan },
    { id: "missionClip", label: "Missão a partir da área de transferência", group: "Missões", run: missionFromClipboard },
    { id: "review", label: "Revisar mudanças da missão", group: "Missões", run: reviewMission },
    { id: "commit", label: "Commitar…", group: "Git", run: () => gitAction("commit") },
    { id: "prOpen", label: "Abrir PR…", group: "Git", run: () => gitAction("pr") },
    { id: "resume", label: "Continuar de onde parei", group: "Missões", run: () => useStore.getState().setActive(null) },
    { id: "newPanel", label: "Novo painel (como o último)", group: "Painéis", keys: keys.newPanel, run: newPanel },
    { id: "attention", label: "Ir para painel aguardando resposta", group: "Painéis", keys: keys.attention, run: goToAttention },
    { id: "newTerminal", label: "Novo terminal", group: "Painéis", keys: keys.newTerminal, run: newTerminal },
    { id: "newGroup", label: "Novo grupo de painéis", group: "Painéis", run: newGroup },
    {
      id: "sensitive",
      label: (st.settings?.sensitiveWorkspaces ?? []).includes(st.active ?? "") ? "Desligar modo sensível deste workspace" : "Ligar modo sensível deste workspace",
      group: "Cofre",
      run: toggleSensitive,
    },
    { id: "next", label: "Próximo painel", group: "Painéis", keys: keys.nextPane, run: () => stepPane(1) },
    { id: "prev", label: "Painel anterior", group: "Painéis", keys: keys.prevPane, run: () => stepPane(-1) },
    { id: "searchPanes", label: "Buscar em todos os painéis", group: "Painéis", keys: keys.searchPanes, run: searchPanes },
    { id: "copyConv", label: "Copiar conversa do painel ativo", group: "Painéis", keys: keys.copyConversation, run: copyConversation },
    { id: "sendSel", label: "Enviar seleção ao agente…", group: "Painéis", run: sendTerminalSelection },
    { id: "composerToggle", label: "Compositor do painel de IA (mensagem com menções)", group: "Painéis", keys: keys.composer, run: toggleComposerActive },
    { id: "activity", label: "Subtarefas e comandos do painel ativo", group: "Painéis", keys: keys.activity, run: toggleActivityActive },
    { id: "gradeModo", label: "Grade: próximo modo", group: "Grade", keys: keys.gridMode, run: cycleGridMode },
    { id: "gradeIgual", label: "Grade: painéis iguais", group: "Grade", run: gradeModo("igual") },
    { id: "gradeEsq", label: "Grade: principal à esquerda", group: "Grade", run: gradeModo("principal-esq") },
    { id: "gradeCima", label: "Grade: principal em cima", group: "Grade", run: gradeModo("principal-cima") },
    { id: "gradeColunas", label: "Grade: colunas lado a lado", group: "Grade", run: gradeModo("colunas") },
    { id: "gradeAuto", label: "Grade: colunas automáticas", group: "Grade", run: gradeCols("auto") },
    { id: "grade1", label: "Grade: 1 coluna", group: "Grade", run: gradeCols(1) },
    { id: "grade2", label: "Grade: 2 colunas", group: "Grade", run: gradeCols(2) },
    { id: "grade3", label: "Grade: 3 colunas", group: "Grade", run: gradeCols(3) },
    { id: "gradeReset", label: "Redefinir tamanhos (grade e árvore)", group: "Grade", run: resetTamanhos },
    ...VERB_ORDER.map((v): Item => ({ id: `ask-${v}`, label: `Pedir ao agente: ${verbLabel(v)} este arquivo (ou seleção)`, group: "Arquivos", run: () => askNowFromEditor(v) })),
    { id: "copyReply", label: "Copiar última resposta do agente", group: "Painéis", keys: keys.copyLastReply, run: copyLastReply },
    { id: "model", label: "Modelo e esforço do painel ativo", group: "Painéis", run: switchModelActive },
    { id: "hibernate", label: "Hibernar painel ativo", group: "Painéis", run: hibernateActive },
    { id: "restoreBanner", label: "Restaurar painéis da última sessão", group: "Painéis", run: restoreLastSession },
    { id: "close", label: "Fechar painel ativo", group: "Painéis", run: closeActive },
    ...panes.slice(0, 9).map((p, i) => ({ id: `pane${i + 1}`, label: `Focar painel ${i + 1}: ${p.name}`, group: "Painéis", keys: keys[`pane${i + 1}`], run: () => focusIndex(i + 1) })),
    ...st.tabs.map((id, i) => ({ id: `ws${id}`, label: `Focar workspace ${i + 1}: ${wsOf(id)?.name ?? id}`, group: "Workspaces", run: () => useStore.getState().setActive(id) })),
    { id: "shot", label: "Galeria de capturas", group: "Janela", run: () => useStore.getState().patch({ shotGallery: true }) },
    { id: "redline", label: "Limites de uso", group: "Janela", run: () => api.redlineOpen().catch((e) => toast(errMsg(e))) },
    { id: "recursos", label: "Monitor de recursos", group: "Janela", run: () => useStore.getState().patch({ resourcesOpen: true }) },
    { id: "openFile", label: "Abrir arquivo", group: "Arquivos", keys: keys.quickOpenAlt, run: () => useStore.getState().patch({ palette: "files" }) },
    { id: "settings", label: "Configurações", group: "Configurações", run: () => useStore.getState().patch({ settingsOpen: true, settingsSection: null }) },
    ...SETTINGS_SECTIONS.map((s) => ({ id: `cfg-${s.id}`, label: `Configurações: ${s.label}`, group: "Configurações", run: () => useStore.getState().patch({ settingsOpen: true, settingsSection: s.id }) })),
    { id: "quickstart-show", label: "Mostrar cards de início", group: "Configurações", run: () => {
      const cur = useStore.getState().settings;
      if (cur) useStore.getState().saveSettings({ ...cur, quickstartDismissed: [] }).catch((e) => toast(errMsg(e)));
    } },
    { id: "prompts", label: "Prompts salvos", group: "Prompts", run: () => useStore.getState().patch({ settingsOpen: true, settingsSection: "prompts" }) },
    { id: "baseSkill", label: "Skill do Regente: instalar ou remover", group: "Configurações", run: () => useStore.getState().patch({ settingsOpen: true, settingsSection: "catalogo" }) },
    ...prompts.map((p) => ({ id: `prompt:${p.id}`, label: `prompt: ${p.name}`, group: "Prompts", run: () => insertIntoActive(p) })),
    ...files.map((f) => ({ id: `file:${f.path}`, label: `arquivo: ${f.label}`, group: "Arquivos", run: () => openFile(f.path) })),
    // ▼ trilha A
    { id: "execPainel", label: "Mostrar execuções do painel ativo", group: "Execuções", run: () => comPainelAtivo((p) => api.execPainel(p.id).then((id) => setTimeout(() => goToPane(id), 50))) },
    { id: "execFechar", label: "Fechar execuções concluídas", group: "Execuções", run: () => comPainelAtivo((p) => api.execFecharConcluidas(p.id).then((n) => useStore.getState().toast(n ? `${n} execução(ões) concluída(s) fechada(s)` : "Nenhuma execução concluída para fechar.", "ok"))) },
    // ▲ trilha A
    // ▼ trilha B
    { id: "checkpointAgora", label: "Checkpoint agora", group: "Checkpoints", run: checkpointNow },
    { id: "linhaDoTempo", label: "Linha do tempo de checkpoints", group: "Checkpoints", run: linhaDoTempo },
    { id: "memSugestoes", label: "Revisar sugestões de memória", group: "Memória", run: () => useStore.getState().patch({ settingsOpen: true, settingsSection: "memoria" }) },
    // ▲ trilha B
    // ▼ trilha C
    { id: "compararAbordagens", label: "Comparar abordagens (mesmo pedido, variantes)…", group: "Missões", run: compararAbordagens },
    { id: "enquantoFora", label: "O que aconteceu enquanto eu estava fora", group: "Missões", run: enquantoFora },
    // ▲ trilha C
  ];
  return items;
}

/** Digest "Enquanto você estava fora": desde a última saída da janela, ou a última hora se não saiu. */
function enquantoFora() {
  const st = useStore.getState();
  if (!st.active) return toast("Abra um workspace primeiro.");
  st.patch({ digestSince: st.blurredAt ?? Date.now() - 60 * 60_000 });
}

function compararAbordagens() {
  const st = useStore.getState();
  if (!st.active) return toast("Abra um workspace primeiro.");
  st.patch({ compare: { workspaceId: st.active, groupId: null } });
}

function togglePalette() {
  const st = useStore.getState();
  st.patch({ palette: st.palette ? null : "all" });
}

function openQuickOpen() {
  useStore.getState().patch({ palette: "files" });
}

/** Atalhos da janela: capturados antes do xterm; em campos de texto só os que têm Ctrl (ou Alt/Win/F-key). */
export function useAppShortcuts(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (st.settingsOpen) return; // o gravador de atalhos das Configurações recebe as teclas
      const resolved = resolveShortcuts(st.settings?.shortcuts);
      const id = matchAction(e, resolved);
      if (!id) return;
      // com um diálogo aberto só a paleta responde (para fechar a si mesma); Esc é dos diálogos
      if (document.querySelector(".modal-backdrop") && !(id === "palette" && st.palette)) return;
      const target = e.target as HTMLElement | null;
      const typing = !!target?.closest?.("input, textarea, select, [contenteditable='true'], .monaco-editor");
      if (typing && !parseHotkey(resolved[id]).ctrl) return;
      if (!interceptsIn(resolved[id], !!target?.closest?.(".xterm"))) return;
      if (id === "copyConversation") {
        const termSel = !!terms.get(st.activePane ?? "")?.hasSelection();
        if (copyDefers({ inputFocused: typing && !target?.closest?.(".xterm"), terminalSelection: termSel, pageSelection: window.getSelection()?.toString() ?? "" })) return;
      }
      e.preventDefault();
      e.stopPropagation();
      const run: Record<string, () => void> = {
        palette: togglePalette,
        paletteAlt: togglePalette,
        newTerminal,
        newPanel,
        attention: goToAttention,
        newMission,
        gate: () => useStore.getState().patch({ gateOpen: true }),
        quickOpen: openQuickOpen,
        quickOpenAlt: openQuickOpen,
        copyConversation,
        copyLastReply,
        searchPanes,
        composer: toggleComposerActive,
        activity: toggleActivityActive,
        gridMode: cycleGridMode,
        nextPane: () => stepPane(1),
        prevPane: () => stepPane(-1),
        ...Object.fromEntries(Array.from({ length: 9 }, (_, i) => [`pane${i + 1}`, () => focusIndex(i + 1)])),
      };
      run[id]?.();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [enabled]);
}

export default function CommandPalette() {
  const mode = useStore((s) => s.palette);
  const ws = useStore((s) => s.active);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const [files, setFiles] = useState<{ label: string; path: string }[] | null>(null);
  const [filesCut, setFilesCut] = useState(false);
  const [prompts, setPrompts] = useState<PromptDef[]>([]);
  const [stacks, setStacks] = useState<RecipeDef[]>([]);
  const listRef = useRef<HTMLDivElement>(null);
  const walking = useRef(false);
  const close = () => useStore.getState().patch({ palette: null });

  // prompts salvos: lidos ao abrir a paleta (a tela de Prompts pode ter mudado a lista)
  useEffect(() => {
    if (mode === "all") api.promptsList().then(setPrompts).catch(() => setPrompts([]));
    if (mode) api.recipesList().then((rs) => setStacks(rs.filter(isStack))).catch(() => setStacks([]));
  }, [mode]);

  // arquivos: ao abrir com Ctrl+P, ou na primeira busca de "todas" (sem pesar a abertura da paleta)
  useEffect(() => {
    const root = wsOf(ws)?.path;
    if (!mode || files || walking.current || (mode === "all" && !query.trim()) || !root) return;
    walking.current = true;
    listFiles(root).then((r) => { setFiles(r.files); setFilesCut(r.truncated); }).catch(() => setFiles([])).finally(() => { walking.current = false; });
  }, [mode, query, files, ws]);

  // as ações só mudam quando o estado que elas leem muda (painéis, abas, configurações, missão)
  const panes = useStore((s) => s.panes);
  const tabs = useStore((s) => s.tabs);
  const settings = useStore((s) => s.settings);
  const missionSel = useStore((s) => s.missionSel);
  const actions = useMemo(() => (mode ? buildItems(files ?? [], prompts, stacks) : []), [mode, files, prompts, stacks, ws, panes, tabs, settings, missionSel]);
  const items = useMemo(() => {
    if (!mode) return [];
    if (mode === "files") return filterByQuery(actions.filter((i) => i.id.startsWith("file:")), query, (i) => i.label).slice(0, SHOWN);
    return filterByQuery(actions.filter((i) => !i.id.startsWith("file:")), query, (i) => `${i.group} ${i.label}`).slice(0, SHOWN);
  }, [mode, query, actions]);

  useEffect(() => { setCursor(0); }, [query, mode]);
  useEffect(() => {
    if (!mode) { setQuery(""); setFiles(null); }
  }, [mode]);

  if (!mode) return null;

  async function runAt(i: number) {
    const item = items[i];
    if (!item) return;
    close();
    try {
      await item.run();
    } catch (e) {
      toast(errMsg(e));
    }
  }

  return (
    <Modal label="Paleta de comandos" onClose={close} style={{ width: "min(600px, 92vw)", display: "flex", flexDirection: "column", gap: 8 }}>
      <input
        autoFocus
        placeholder={mode === "files" ? "Abrir arquivo…" : "Digite um comando…"}
        aria-label="Buscar comando"
        aria-controls="palette-list"
        aria-activedescendant={items[cursor] ? `palette-${cursor}` : undefined}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") { e.preventDefault(); setCursor((c) => Math.min(c + 1, items.length - 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setCursor((c) => Math.max(c - 1, 0)); }
          else if (e.key === "Enter") { e.preventDefault(); void runAt(cursor); }
        }}
      />
      <div id="palette-list" role="listbox" aria-label="Comandos" ref={listRef} style={{ maxHeight: "55vh", overflowY: "auto" }}>
        {items.length === 0 && <div className="meta">Nada encontrado.</div>}
        {items.map((it, i) => (
          <div key={it.id} id={`palette-${i}`} role="option" aria-selected={i === cursor} className={`menu-item ${i === cursor ? "sel" : ""}`}
            onMouseEnter={() => setCursor(i)} onClick={() => void runAt(i)}
            style={{ display: "flex", justifyContent: "space-between", gap: 12, background: i === cursor ? "var(--line)" : undefined }}>
            <span>{it.label}</span>
            <span className="meta">{it.keys ? <span className="kbd">{it.keys}</span> : it.group}</span>
          </div>
        ))}
        {mode === "all" && !query.trim() && <div className="meta" style={{ padding: 6 }}>Digite para buscar arquivos também.</div>}
        {mode === "files" && !files && <div className="meta" style={{ padding: 6 }}>Lendo os arquivos do workspace…</div>}
        {filesCut && <div className="meta" style={{ padding: 6 }}>Lista parcial (20 mil arquivos)</div>}
      </div>
      <div className="meta" style={{ padding: 6 }}>Ctrl+Shift+P abre isto de qualquer lugar.</div>
    </Modal>
  );
}
