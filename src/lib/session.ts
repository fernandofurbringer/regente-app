/** Estado da janela principal que volta no dia seguinte: abas, workspace ativo, missão e visão de cada um. Fica no navegador do Regente. */
export interface SessionSnap {
  tabs: string[];
  active: string | null;
  missionSel: Record<string, string | null>;
  view: Record<string, "terminais" | "arquivos">;
}

const KEY = "regente.sessao";

/** Lê o estado salvo; texto inválido ou de outra forma vira null, e cada campo ruim cai no padrão. */
export function parseSession(raw: string | null): SessionSnap | null {
  if (!raw) return null;
  try {
    const v: unknown = JSON.parse(raw);
    if (!v || typeof v !== "object" || Array.isArray(v)) return null;
    const o = v as Record<string, unknown>;
    const tabs = Array.isArray(o.tabs) ? o.tabs.filter((t): t is string => typeof t === "string") : [];
    const missionSel: Record<string, string | null> = {};
    if (o.missionSel && typeof o.missionSel === "object" && !Array.isArray(o.missionSel)) {
      for (const [ws, id] of Object.entries(o.missionSel)) if (typeof id === "string" && id) missionSel[ws] = id;
    }
    const view: SessionSnap["view"] = {};
    if (o.view && typeof o.view === "object" && !Array.isArray(o.view)) {
      for (const [ws, mode] of Object.entries(o.view)) if (mode === "terminais" || mode === "arquivos") view[ws] = mode;
    }
    return {
      tabs,
      active: typeof o.active === "string" && tabs.includes(o.active) ? o.active : null,
      missionSel,
      view,
    };
  } catch {
    return null;
  }
}

export function readSession(): SessionSnap | null {
  try {
    return parseSession(localStorage.getItem(KEY));
  } catch {
    return null;
  }
}

export function writeSession(s: SessionSnap): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    // sem armazenamento: a sessão só vale até fechar o Regente
  }
}
