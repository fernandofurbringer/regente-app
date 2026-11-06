import type { AgentDef } from "./ipc";

/** busca sem acento e sem caixa */
export const norm = (s: string) => s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

export interface AgentFilter { q: string; source: string; onlyRole: boolean }

/** Filtra os agentes: texto (nome e descrição), origem ("todos" = sem filtro) e "tem tarefa" (função definida). */
export function filterAgents(agents: AgentDef[], f: AgentFilter): AgentDef[] {
  const q = norm(f.q.trim());
  return agents.filter((a) => (!q || norm(`${a.name} ${a.description}`).includes(q))
    && (f.source === "todos" || a.source === f.source)
    && (!f.onlyRole || !!a.role));
}

/** Marca de instalação: "…" enquanto carrega, "✓" ou "—". */
export const installMark = (on: boolean | undefined) => (on === undefined ? "…" : on ? "✓" : "—");
