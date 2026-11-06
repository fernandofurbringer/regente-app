import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactElement } from "react";
import { useShallow } from "zustand/react/shallow";
import { moveItem } from "../lib/grid";
import { colFrOf, colGutters, cssColGutter, cssPlace, cssRowGutter, planGrid, resizePair, resizeRow, rowGutters, templates, trackPx, type GridPrefs, type Slot } from "../lib/layout";
import { gridPrefsOf, setGridPrefs, useGridPrefs } from "../lib/layoutStore";
import Gutter from "./Gutter";
import { adoptedGroupId, groupedItems, groupHeads } from "../lib/groups";
import { shownPanesIn } from "../lib/visible";
import { isInlineSubagent } from "../lib/activity";
import { api, errMsg, type PaneGroup, type PaneRow } from "../lib/ipc";
import type { DragPayload } from "../lib/pointerDrag";
import { useStore } from "../state/store";
import BrowserPane from "./BrowserPane";
import ImagePane from "./ImagePane";
import VideoPane from "./VideoPane";
import SubagentPane from "./SubagentPane";
import PaneHeader from "./PaneHeader";
import TerminalPane from "./TerminalPane";
import ExecPane from "./ExecPane";
import ExecucoesPane from "./ExecucoesPane";
import SessionPicker, { continueLastSession } from "./SessionPicker";
import { spawnInActive } from "./settings/Settings";
import { readLastCli } from "../lib/lastAgent";
import { defaultAgentLabel } from "../lib/quickstart";
import { resolveShortcuts } from "../lib/shortcuts";
import { accountPeak } from "../lib/redline";
import { accountChoices, accountOption, spawnAccount, useLastAccount } from "../lib/lastAccount";

/** Grade sem painéis: três ações com a tecla de cada uma. A IA é a padrão (a última escolhida, senão a primeira instalada). */
function EmptyCards({ wsId }: { wsId: string }) {
  const detected = useStore((s) => s.detected);
  const providers = useStore((s) => s.providers);
  const keys = resolveShortcuts(useStore((s) => s.settings?.shortcuts));
  const agent = defaultAgentLabel(detected, readLastCli(), providers?.clis ?? []);
  // 2+ contas: escolhe antes de abrir; o padrão é a última usada nesta pasta (não a principal sempre)
  const extras = useStore((s) => s.settings?.accounts[agent.cli]);
  const limits = useStore((s) => s.limits);
  const peak = (c: string) => (limits ? accountPeak(limits, agent.cli, c) : null);
  const contas = accountChoices(extras, peak);
  const ultima = useLastAccount(wsId, contas.length ? agent.cli : null);
  const [escolha, setEscolha] = useState<string | null>(null);
  const conta = escolha ?? ultima ?? "principal";
  return (
    <div className="empty-cards">
      <div className="card">
        <strong>Painel de IA</strong>
        <p className="meta">{agent.label}, no workspace atual. <kbd>{keys.newPanel}</kbd></p>
        {contas.length > 0 && (
          <label className="field">
            <span>Conta</span>
            <select aria-label={`Conta do ${agent.label}`} value={conta} onChange={(e) => setEscolha(e.target.value)}>
              {contas.map((c) => <option key={c} value={c}>{accountOption(c, peak(c))}</option>)}
            </select>
          </label>
        )}
        <button className="btn primary" disabled={contas.length > 0 && ultima === undefined && escolha === null}
          onClick={() => void spawnInActive({ kind: "agent", cli: agent.cli, permission: "pend", account: spawnAccount(conta) })}>
          Abrir {agent.label}{contas.length > 0 ? ` (${conta})` : ""}
        </button>
      </div>
      <div className="card">
        <strong>Nova missão</strong>
        <p className="meta">Vários agentes com o mesmo pedido. <kbd>{keys.newMission}</kbd></p>
        <button className="btn" onClick={() => useStore.getState().patch({ dialog: "missao" })}>Nova missão</button>
      </div>
      <div className="card">
        <strong>Retomar última conversa</strong>
        <p className="meta">A mais recente desta pasta, de qualquer IA.</p>
        <button className="btn" onClick={() => void continueLastSession(wsId)}>Retomar última conversa</button>
      </div>
    </div>
  );
}

/** Missão filtrada sem painéis abertos: restaura os dela que estão pendentes, ou abre um painel nela. */
function MissionEmpty({ wsId, missionId, style }: { wsId: string; missionId: string | null; style?: CSSProperties }) {
  const pending = useStore(useShallow((s) => (s.restore[wsId] ?? []).filter((r) => r.missionId === missionId)));
  async function restore() {
    const st = useStore.getState();
    const ids = pending.map((r) => r.id);
    try {
      for (const r of await api.restoreApply(wsId, true, ids)) st.upsertPane(r);
      // o estado é relido depois do await: o restore pode ter mudado enquanto a chamada rodava
      const now = useStore.getState();
      const left = (now.restore[wsId] ?? []).filter((r) => !ids.includes(r.id));
      const restore = { ...now.restore };
      if (left.length) restore[wsId] = left;
      else delete restore[wsId];
      now.patch({ restore, restoreDetail: Object.keys(restore).length > 0 && now.restoreDetail });
    } catch (e) {
      st.toast(errMsg(e), "erro");
    }
  }
  return (
    <div className="empty" style={style}>
      Esta missão não tem painéis abertos.{" "}
      {pending.length > 0 && <button className="btn" onClick={() => void restore()}>Restaurar</button>}{" "}
      <button className="btn" onClick={() => useStore.getState().patch({ dialog: "painel" })}>Abrir painel nela</button>
    </div>
  );
}

export function PaneContent({ pane }: { pane: PaneRow }) {
  if (pane.kind === "browser") return <BrowserPane pane={pane} />;
  if (pane.kind === "image") return <ImagePane pane={pane} />;
  if (pane.kind === "video") return <VideoPane pane={pane} />;
  if (pane.kind === "subagent") return <SubagentPane pane={pane} />;
  if (pane.kind === "exec") return <ExecPane pane={pane} />;
  if (pane.kind === "execucoes") return <ExecucoesPane pane={pane} />;
  return <TerminalPane pane={pane} />;
}

/** Cabeçalho de grupo: nome editável, recolher/expandir, desfazer (os painéis continuam abertos). */
function GroupBar({ wsId, group, count }: { wsId: string; group: PaneGroup; count: number }) {
  const [name, setName] = useState(group.name);
  useEffect(() => setName(group.name), [group.name]);
  const toast = useStore((s) => s.toast);
  const save = () => {
    const n = name.trim();
    if (!n || n === group.name) return setName(group.name);
    useStore.getState().groupPatch(wsId, group.id, { name: n }).catch((e) => { setName(group.name); toast(errMsg(e), "erro"); });
  };
  const toggle = () => useStore.getState().groupPatch(wsId, group.id, { collapsed: !group.collapsed }).catch((e) => toast(errMsg(e), "erro"));
  const undo = () => useStore.getState().groupDelete(wsId, group.id).catch((e) => toast(errMsg(e), "erro"));
  return (
    <div className="pane-group-bar" data-drop="group" data-group-id={group.id} title="Solte um painel aqui para entrar no grupo">
      <button type="button" className="ghost" aria-expanded={!group.collapsed} aria-label={group.collapsed ? "Expandir grupo" : "Recolher grupo"} onClick={toggle}>
        {group.collapsed ? "▸" : "▾"}
      </button>
      <input aria-label="Nome do grupo" value={name} maxLength={40} onChange={(e) => setName(e.target.value)}
        onBlur={save} onKeyDown={(e) => { if (e.key === "Enter") e.currentTarget.blur(); if (e.key === "Escape") { setName(group.name); e.currentTarget.blur(); } }} />
      <span className="muted">{count} {count === 1 ? "painel" : "painéis"}{group.collapsed ? " · recolhido" : ""}</span>
      <button type="button" className="ghost" aria-label="Desfazer grupo" title="Desfazer grupo (os painéis continuam abertos)" onClick={undo}>×</button>
    </div>
  );
}

// memo: arrastar, focar ou mudar outro painel não re-renderiza este
const PaneCell = memo(function PaneCell({ pane, index, hidden, gridColumn, gridRow, detached, group, count }: {
  pane: PaneRow; index: number; hidden: boolean; gridColumn?: string; gridRow?: string; detached: boolean;
  /** grupo aberto cujo cabeçalho fica no topo deste painel (o primeiro do grupo) */
  group?: PaneGroup; count: number;
}) {
  // zona de reordenar (data-drop="pane"): o arrasto do cabeçalho solta aqui
  return (
    <section id={`pane-${pane.id}`} aria-label={pane.name} className="pane" data-drop="pane" data-pane-id={pane.id}
      style={{ display: hidden ? "none" : undefined, gridColumn, gridRow }}>
      {group && <GroupBar wsId={pane.workspaceId} group={group} count={count} />}
      <PaneHeader pane={pane} index={index} />
      <div className="pane-body">{detached ? <div className="empty">Aberto em janela própria.</div> : <PaneContent pane={pane} />}</div>
    </section>
  );
});

/** Grupo recolhido, vazio ou sem painel visível: linha fina de largura total (também é zona para soltar painéis). */
function GroupCell({ wsId, group, count, members, hasVisible, gridColumn, gridRow }: { wsId: string; group: PaneGroup; count: number; members: PaneRow[]; hasVisible: boolean; gridColumn?: string; gridRow?: string }) {
  const asking = members.filter((p) => p.status === "pergunta").length;
  const failed = members.filter((p) => p.status === "erro").length;
  const note = group.collapsed ? "" : count === 0 ? "Grupo vazio. Arraste um painel para cá." : hasVisible ? "" : "Nenhum painel desta missão neste grupo.";
  return (
    <div className="pane pane-group-cell" data-drop="group" data-group-id={group.id} style={{ gridColumn, gridRow }}>
      <GroupBar wsId={wsId} group={group} count={count} />
      {(asking > 0 || failed > 0 || note) && (
        <p className="meta pane-group-note">
          {asking > 0 && <span className="pane-group-ask">● {asking} {asking === 1 ? "pergunta" : "perguntas"} </span>}
          {failed > 0 && <span className="pane-group-fail">● {failed} {failed === 1 ? "erro" : "erros"} </span>}
          {note}
        </p>
      )}
    </div>
  );
}

export default function PaneGrid({ wsId }: { wsId: string }) {
  const sel = useStore((s) => s.missionSel[wsId] ?? null);
  // todos os painéis do workspace ficam montados; o filtro da missão só esconde (terminais não reiniciam)
  // subagente inline mora na faixa do painel pai: fora da grade
  const panes = useStore(useShallow((s) => Object.values(s.panes).filter((p) => p.workspaceId === wsId && !isInlineSubagent(p, s.settings?.subagentPanes ?? false)).sort((a, b) => a.ord - b.ord)));
  const groups = useStore((s) => s.groups[wsId] ?? EMPTY_GROUPS);
  const focused = useStore((s) => s.focus[wsId] ?? null);
  const detached = useStore((s) => s.detached);
  const items = useMemo(() => groupedItems(panes, groups), [panes, groups]);
  // mesma lista que o Ctrl+N e a paleta numeram: o #N do cabeçalho é a posição nela
  const shownList = useMemo(() => shownPanesIn(panes, groups, sel), [panes, groups, sel]);
  const shown = shownList.map((p) => p.id);
  const prefs = useGridPrefs(wsId);
  const gridRef = useRef<HTMLDivElement>(null);
  /** começo de um arrasto: faixas resolvidas (px) do grid e as preferências daquele momento */
  const arrasto = useRef<{ cols: number[]; rows: number[]; prefs: GridPrefs } | null>(null);
  const idsRef = useRef(panes.map((p) => p.id));
  idsRef.current = panes.map((p) => p.id);

  const drop = useCallback((from: string, target: string) => {
    if (from === target) return;
    const list = idsRef.current; // ordem nova calculada sobre o workspace inteiro
    const next = moveItem(list, list.indexOf(from), list.indexOf(target));
    next.forEach((id, i) => useStore.getState().patchPane(id, { ord: i }));
    api.paneReorder(next).catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  }, []);

  // arrasto de painel (cabeçalho) solto numa célula, num cabeçalho de grupo ou no fundo da grade
  useEffect(() => {
    const onDrop = (e: Event) => {
      const { kind, data } = (e as CustomEvent<DragPayload>).detail;
      const el = e.target as Element;
      const st = useStore.getState();
      if (kind !== "pane" || st.panes[data]?.workspaceId !== wsId) return;
      const group = el.closest("[data-group-id]")?.getAttribute("data-group-id");
      if (group) return void st.paneSetGroup(data, group).catch((err) => st.toast(errMsg(err), "erro"));
      const target = el.closest("[data-pane-id]")?.getAttribute("data-pane-id");
      if (target && st.panes[target]?.workspaceId === wsId) {
        // soltar sobre um painel de grupo: o painel solto entra no grupo dele
        const adopt = adoptedGroupId(st.panes[data], st.panes[target]);
        if (adopt) void st.paneSetGroup(data, adopt).catch((err) => st.toast(errMsg(err), "erro"));
        return drop(data, target);
      }
      if (el.closest('[data-drop="ungroup"]')) void st.paneSetGroup(data, null).catch((err) => st.toast(errMsg(err), "erro"));
    };
    document.addEventListener("regente-drop", onDrop);
    return () => document.removeEventListener("regente-drop", onDrop);
  }, [wsId, drop]);

  if (panes.length === 0) {
    return (
      <div className="grid">
        <EmptyCards wsId={wsId} />
        <SessionPicker wsId={wsId} />
      </div>
    );
  }

  const indexOf = new Map(shownList.map((p, i) => [p.id, i]));
  const counts = new Map<string, number>();
  for (const p of panes) if (p.groupId) counts.set(p.groupId, (counts.get(p.groupId) ?? 0) + 1);
  // o que pode aparecer: o painel focado, ou os à vista (missão, grupos abertos)
  const candidatos = new Set(focused && shown.includes(focused) ? [focused] : shown);
  // o cabeçalho do grupo vai no primeiro painel visível dele; recolhido ou sem painel visível: linha cheia própria
  const heads = groupHeads(items, candidatos);
  const slots: Slot[] = [];
  if (shown.length === 0) slots.push({ kind: "full", key: "vazio" });
  for (const it of items) {
    if (it.kind === "group") {
      if (it.group.collapsed || !heads.has(it.group.id)) slots.push({ kind: "full", key: `g-${it.group.id}` });
    } else if (candidatos.has(it.pane.id)) slots.push({ kind: "pane", id: it.pane.id });
  }
  const plan = planGrid(slots, focused, prefs);
  const place = new Map(plan.placed.map((p) => [p.key, cssPlace(p)]));
  const tpl = templates(plan, prefs);
  const cells: ReactElement[] = [];
  for (const it of items) {
    if (it.kind === "group") {
      const key = `g-${it.group.id}`;
      const at = place.get(key);
      if (at) {
        const members = panes.filter((p) => p.groupId === it.group.id);
        cells.push(<GroupCell key={key} wsId={wsId} group={it.group} count={it.count} members={members} hasVisible={!it.group.collapsed && it.count > 0} {...at} />);
      }
      continue;
    }
    const p = it.pane;
    const at = place.get(p.id);
    const g = heads.get(p.groupId ?? "") === p.id ? groups.find((x) => x.id === p.groupId) : undefined;
    cells.push(<PaneCell key={p.id} pane={p} index={indexOf.get(p.id) ?? 0} hidden={!at} gridColumn={at?.gridColumn} gridRow={at?.gridRow}
      detached={!!detached[p.id]} group={g} count={g ? counts.get(g.id) ?? 0 : 0} />);
  }
  // divisores: o arrasto parte dos tamanhos resolvidos no começo e salva no fim
  const inicio = () => {
    const el = gridRef.current;
    if (!el) return;
    const cs = getComputedStyle(el);
    arrasto.current = { cols: trackPx(cs.gridTemplateColumns), rows: trackPx(cs.gridTemplateRows), prefs: gridPrefsOf(wsId) };
  };
  const fim = () => {
    arrasto.current = null;
    setGridPrefs(wsId, gridPrefsOf(wsId));
  };
  const gutters = [
    ...colGutters(plan).map((g) => (
      <Gutter key={`c${g.c}-${g.rowFrom}`} axis="x" label="Largura das colunas" style={cssColGutter(g)} onStart={inicio} onEnd={fim}
        onMove={(dx) => {
          const a = arrasto.current;
          if (a) setGridPrefs(wsId, { ...a.prefs, colFr: resizePair(colFrOf(a.prefs, plan.cols), g.c, a.cols[2 * g.c] ?? 0, a.cols[2 * g.c + 2] ?? 0, dx) }, false);
        }}
        onReset={() => setGridPrefs(wsId, { ...gridPrefsOf(wsId), colFr: [] })} />
    )),
    ...rowGutters(plan).map((g) => (
      <Gutter key={`r${g.r}-${g.colFrom}`} axis="y" label="Altura das linhas" style={cssRowGutter(g)} onStart={inicio} onEnd={fim}
        onMove={(dy) => {
          const a = arrasto.current;
          if (a) setGridPrefs(wsId, { ...a.prefs, rowPx: resizeRow(a.prefs.rowPx, g.r, a.rows[2 * g.r] ?? 0, dy) }, false);
        }}
        onReset={() => {
          const cur = gridPrefsOf(wsId);
          const rowPx = { ...cur.rowPx };
          delete rowPx[String(g.r)];
          setGridPrefs(wsId, { ...cur, rowPx });
        }} />
    )),
  ];
  return (
    <div ref={gridRef} className="grid" data-drop="ungroup" style={{ gap: 0, gridTemplateColumns: tpl.columns, gridTemplateRows: tpl.rows }}>
      {shown.length === 0 && <MissionEmpty wsId={wsId} missionId={sel} style={place.get("vazio")} />}
      {cells}
      {gutters}
    </div>
  );
}

const EMPTY_GROUPS: PaneGroup[] = [];
