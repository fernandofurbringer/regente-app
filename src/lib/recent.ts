/** Último uso dos workspaces e das missões na Home: agrupamento e texto relativo. Funções puras (o "agora" entra por parâmetro). */

export type UseGroup = "Hoje" | "Ontem" | "Últimos 7 dias" | "Mais antigos";
export const USE_GROUP_ORDER: UseGroup[] = ["Hoje", "Ontem", "Últimos 7 dias", "Mais antigos"];
const DAY = 86_400_000;

/** Em que grupo cai um uso em `lastUsedMs`, contando dias pelo calendário local de `now`. */
export function useGroup(lastUsedMs: number, now: Date): UseGroup {
  const startToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (lastUsedMs >= startToday) return "Hoje";
  if (lastUsedMs >= startToday - DAY) return "Ontem";
  if (lastUsedMs >= startToday - 7 * DAY) return "Últimos 7 dias";
  return "Mais antigos";
}

/** Agrupa pelo último uso, mais recente primeiro dentro de cada grupo; grupos vazios somem. */
export function groupByLastUse<T>(items: T[], lastUsed: (item: T) => number, now: Date): [UseGroup, T[]][] {
  const groups = new Map<UseGroup, T[]>();
  for (const item of [...items].sort((a, b) => lastUsed(b) - lastUsed(a))) {
    const g = useGroup(lastUsed(item), now);
    groups.set(g, [...(groups.get(g) ?? []), item]);
  }
  return USE_GROUP_ORDER.filter((g) => groups.has(g)).map((g) => [g, groups.get(g)!]);
}

/** "há 12 min", "há 3 h", "há 2 d"; menos de um minuto vira "agora". */
export function agoText(ms: number, now: number): string {
  const min = Math.floor((now - ms) / 60_000);
  if (min < 1) return "agora";
  if (min < 60) return `há ${min} min`;
  const h = Math.floor(min / 60);
  if (h < 24) return `há ${h} h`;
  return `há ${Math.floor(h / 24)} d`;
}
