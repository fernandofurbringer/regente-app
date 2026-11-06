import type { PaneRow } from "./ipc";

export interface RestoreCard { key: string; missionId: string | null; title: string; panes: PaneRow[] }

/**
 * Pura: um cartão por missão, na ordem em que os painéis chegaram (ord).
 * Painéis sem missão formam o cartão "Sem missão" (key ""). Missão que sumiu do rail também vira "Missão".
 */
export function restoreCards(rows: PaneRow[], missions: { id: string; name: string }[] = []): RestoreCard[] {
  const byKey = new Map<string, PaneRow[]>();
  for (const r of rows) {
    const k = r.missionId ?? "";
    byKey.set(k, [...(byKey.get(k) ?? []), r]);
  }
  return [...byKey.entries()].map(([key, panes]) => ({
    key,
    missionId: key || null,
    title: key ? (missions.find((m) => m.id === key)?.name ?? "Missão") : "Sem missão",
    panes,
  }));
}

/** Pura: os ids que o cartão aplica; sem escolha (null) vale o cartão inteiro. */
export function cardIds(card: RestoreCard, chosen: Record<string, boolean> | null): string[] {
  return card.panes.filter((p) => (chosen ? chosen[p.id] ?? true : true)).map((p) => p.id);
}

/** Totais do banner de restauração: painéis pendentes e em quantos workspaces. */
export function restoreTotals(restore: Record<string, PaneRow[]>): { panes: number; workspaces: number } {
  const counts = Object.values(restore).map((rows) => rows.length).filter((n) => n > 0);
  return { panes: counts.reduce((a, b) => a + b, 0), workspaces: counts.length };
}
