import type { MemoryRow } from "./ipc";

/** Filtro da lista de memórias: tipo ("todos" = sem filtro) e texto (sem acento e sem caixa). */
export function filterMemories(list: MemoryRow[], kind: string, q: string): MemoryRow[] {
  const needle = q.trim().normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  return list.filter((m) => (kind === "todos" || m.kind === kind)
    && (!needle || m.text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase().includes(needle)));
}

/** Tipos de memória que o agente grava (fato, decisão, risco, checkpoint). */
export const MEMORY_KINDS = ["fato", "decisao", "risco", "checkpoint"] as const;
