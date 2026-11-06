import { api, type PaneGroup, type PaneRow } from "./ipc";
import { groupedItems } from "./groups";
import { isInlineSubagent } from "./activity";
import { useStore } from "../state/store";

interface VisibleState {
  active: string | null;
  view: Record<string, string>;
  missionSel: Record<string, string | null>;
  panes: Record<string, PaneRow>;
  groups: Record<string, PaneGroup[]>;
  settings: { subagentPanes: boolean } | null;
}

/** Pura: painéis na ordem em que a grade os mostra (grupos antes), sem os fechados, os de grupo recolhido,
 *  os de outra missão e os subagentes inline (faixa do painel pai, quando `subagentPanes` é false).
 *  É a lista que o #N do cabeçalho, o Ctrl+N e a paleta numeram. */
export function shownPanesIn(panes: PaneRow[], groups: PaneGroup[], sel: string | null, subagentPanes = true): PaneRow[] {
  const open = panes.filter((p) => p.closedAt === null && !isInlineSubagent(p, subagentPanes)).sort((a, b) => a.ord - b.ord);
  return groupedItems(open, groups).flatMap((it) =>
    it.kind === "pane" && !it.hidden && (!sel || it.pane.missionId === sel) ? [it.pane] : [],
  );
}

/** Painéis à vista do workspace (ver `shownPanesIn`), lidos do store por padrão. */
export function shownPanes(wsId: string, s: VisibleState = useStore.getState()): PaneRow[] {
  const panes = Object.values(s.panes).filter((p) => p.workspaceId === wsId).sort((a, b) => a.ord - b.ord);
  return shownPanesIn(panes, s.groups[wsId] ?? [], s.missionSel[wsId] ?? null, s.settings?.subagentPanes ?? false);
}

/** Painéis à vista agora: os do workspace ativo na visão de terminais, respeitando o filtro de missão. */
export function visiblePaneIds(s: VisibleState): string[] {
  if (!s.active || (s.view[s.active] ?? "terminais") !== "terminais") return [];
  return shownPanes(s.active, s).map((p) => p.id);
}

/** Reporta ao Regente os painéis à vista (o aviso do Windows só sai dos que estão fora da tela). Só envia quando muda. */
export function startVisibilityReport(): () => void {
  let last = "";
  const send = () => {
    const ids = visiblePaneIds(useStore.getState());
    const key = ids.join(",");
    if (key === last) return;
    last = key;
    api.panesVisible(ids).catch(() => { last = ""; });
  };
  send();
  const unsub = useStore.subscribe(send);
  window.addEventListener("focus", send);
  window.addEventListener("blur", send);
  return () => {
    unsub();
    window.removeEventListener("focus", send);
    window.removeEventListener("blur", send);
  };
}
