/** Último modelo e esforço usados em cada IA (por CLI): base de "Novo painel" e do formulário. Fica no navegador do Regente. */
export interface AgentPick { model: string | null; effort: string | null }

const KEY = "regente.ultimoAgente";
const text = (v: unknown) => (typeof v === "string" && v ? v : null);

/** Lê o mapa salvo; texto inválido ou de outra forma vira vazio. */
export function parsePicks(raw: string | null): Record<string, AgentPick> {
  if (!raw) return {};
  try {
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object" || Array.isArray(v)) return {};
    const out: Record<string, AgentPick> = {};
    for (const [cli, p] of Object.entries(v)) {
      if (p && typeof p === "object") out[cli] = { model: text((p as Record<string, unknown>).model), effort: text((p as Record<string, unknown>).effort) };
    }
    return out;
  } catch {
    return {};
  }
}

export function readPicks(): Record<string, AgentPick> {
  try {
    return parsePicks(localStorage.getItem(KEY));
  } catch {
    return {};
  }
}

export function lastPickFor(cli: string | null): AgentPick | null {
  return cli ? readPicks()[cli] ?? null : null;
}

export function rememberPick(cli: string, pick: AgentPick): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...readPicks(), [cli]: pick }));
    rememberLastCli(cli);
  } catch {
    // sem armazenamento: vale só até fechar o Regente
  }
}

const CLI_KEY = "regente.ultimaIA";
const MISSION_KEY = "regente.ultimaMissao";

/** A IA escolhida por último (padrão de "Novo painel" e do assistente de missão); null se nunca escolheu. */
export function readLastCli(): string | null {
  try {
    return localStorage.getItem(CLI_KEY) || null;
  } catch {
    return null;
  }
}

export function rememberLastCli(cli: string): void {
  try {
    localStorage.setItem(CLI_KEY, cli);
  } catch {
    // sem armazenamento: vale só até fechar o Regente
  }
}

/** O que o assistente de missão lembra da última missão (sem o pedido nem o nome). */
export interface MissionMemory { mode: "livre" | "squad" | "agentico"; count: number; worktree: boolean; template: Record<string, unknown> }

/** Lê a memória da missão; texto inválido ou campo fora do esperado vira null (ou o campo cai no padrão). */
export function parseMission(raw: string | null): MissionMemory | null {
  if (!raw) return null;
  try {
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object" || Array.isArray(v)) return null;
    const o = v as Record<string, unknown>;
    const mode = o.mode === "livre" || o.mode === "squad" || o.mode === "agentico" ? o.mode : "livre";
    const count = typeof o.count === "number" && Number.isInteger(o.count) ? Math.min(64, Math.max(1, o.count)) : 2;
    const template = o.template && typeof o.template === "object" && !Array.isArray(o.template) ? (o.template as Record<string, unknown>) : {};
    return { mode, count, worktree: o.worktree === true, template };
  } catch {
    return null;
  }
}

export function readMission(): MissionMemory | null {
  try {
    return parseMission(localStorage.getItem(MISSION_KEY));
  } catch {
    return null;
  }
}

export function rememberMission(m: MissionMemory): void {
  try {
    localStorage.setItem(MISSION_KEY, JSON.stringify(m));
  } catch {
    // sem armazenamento: vale só até fechar o Regente
  }
}
