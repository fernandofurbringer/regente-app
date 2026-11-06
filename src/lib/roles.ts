/** Rótulo do papel de um painel no cabeçalho. Sem papel (terminal comum) = "". */
export function roleLabel(role: string | null, agent?: string | null): string {
  if (!role) return "";
  if (role === "piloto") return "PILOTO";
  if (role === "ajudante") return "AJUDANTE";
  if (role === "maestro") return "MAESTRO";
  if (role.startsWith("chefe:")) return `CHEFE · ${role.slice(6).toUpperCase()}`;
  if (role === "agente" && agent) return agent.toUpperCase();
  return "";
}

/** Piloto e maestro comandam a missão: o cabeçalho mostra o custo agregado dela. */
export function isOrchestrator(role: string | null): boolean {
  return role === "piloto" || role === "maestro";
}
