import type { PaneGroup, PaneRow } from "./ipc";

export type GridItem =
  | { kind: "group"; group: PaneGroup; count: number }
  | { kind: "pane"; pane: PaneRow; hidden: boolean };

/**
 * Pura: ordem da grade. Cada grupo vem em ordem, com os seus painéis em sequência logo depois do cabeçalho;
 * os painéis sem grupo vêm no fim. Painel de grupo recolhido vem com `hidden` (continua montado).
 */
export function groupedItems(panes: PaneRow[], groups: PaneGroup[]): GridItem[] {
  const byId = new Map(groups.map((g) => [g.id, g]));
  const members = new Map<string, PaneRow[]>();
  const loose: PaneRow[] = [];
  for (const p of panes) {
    if (p.groupId && byId.has(p.groupId)) members.set(p.groupId, [...(members.get(p.groupId) ?? []), p]);
    else loose.push(p);
  }
  const out: GridItem[] = [];
  for (const g of groups) {
    const ps = members.get(g.id) ?? [];
    out.push({ kind: "group", group: g, count: ps.length });
    for (const p of ps) out.push({ kind: "pane", pane: p, hidden: !!g.collapsed });
  }
  for (const p of loose) out.push({ kind: "pane", pane: p, hidden: false });
  return out;
}

/**
 * Pura: onde fica o cabeçalho de cada grupo: o primeiro painel VISÍVEL dele (não recolhido e no filtro da missão).
 * Grupo sem painel visível não tem cabeçalho na grade (vira linha fina, ver `PaneGrid`).
 */
export function groupHeads(items: GridItem[], visible: Set<string>): Map<string, string> {
  const heads = new Map<string, string>();
  for (const it of items) {
    if (it.kind !== "pane" || it.hidden || !it.pane.groupId || !visible.has(it.pane.id)) continue;
    if (!heads.has(it.pane.groupId)) heads.set(it.pane.groupId, it.pane.id);
  }
  return heads;
}

/**
 * Pura: ao soltar um painel sobre outro que está num grupo, o painel solto entra nesse grupo.
 * Devolve o id do grupo novo, ou `undefined` quando nada muda (alvo sem grupo).
 */
export function adoptedGroupId(dragged: PaneRow | undefined, target: PaneRow | undefined): string | undefined {
  if (!target?.groupId || dragged?.groupId === target.groupId) return undefined;
  return target.groupId;
}
