/** Missões com painel vivo: o celular mostra só elas por padrão. Painel hibernado não conta como em uso. */
export function missionsInUse<M extends { id: string }>(missions: M[], panes: { missionId: string | null; status: string }[]): M[] {
  const live = new Set(panes.filter((p) => p.missionId !== null && p.status !== "hibernado").map((p) => p.missionId as string));
  return missions.filter((m) => live.has(m.id));
}
