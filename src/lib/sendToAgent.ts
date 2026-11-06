import { api, type PaneRow } from "./ipc";

/** Limite do texto enviado por um atalho de "Corrigir isto" ou seleção (o compositor não corta). */
export const SEND_MAX = 4 * 1024;

/** Corta o texto no teto em caracteres (com marca) — o envio de contexto nunca passa de `max`. */
export function capText(text: string, max = SEND_MAX): string {
  const MARK = "\n[cortado]";
  if (text.length <= max) return text;
  return text.slice(0, max - MARK.length) + MARK;
}

/** Estados em que um painel de IA ainda recebe texto (fechado, hibernado ou em fechamento não recebe; `pergunta` espera a sua resposta). */
const RECEBE = ["iniciando", "trabalhando", "ocioso", "acordando", "concluido", "erro"];

/** Cola o texto no painel de IA (bracketed paste; o backend limpa). Com `enter`, o agente recebe e começa. */
export function sendToAgent(paneId: string, text: string, enter = false): Promise<void> {
  return api.promptPaste(paneId, text, enter);
}

/** Painéis de IA que recebem texto: do mesmo workspace e, se a missão for dada, dessa missão. Mais recente por último. */
export function agentTargets(panes: PaneRow[], workspaceId: string, missionId: string | null): PaneRow[] {
  return panes
    .filter((p) => p.kind === "agent" && p.workspaceId === workspaceId && p.missionId === missionId && RECEBE.includes(p.status))
    .sort((a, b) => a.ord - b.ord);
}

const KEY = "regente.ultimoDestino";

/** Último painel escolhido como destino (fica no navegador do Regente). */
export function readLastTarget(): string | null {
  try {
    return localStorage.getItem(KEY) || null;
  } catch {
    return null;
  }
}

export function rememberLastTarget(paneId: string): void {
  try {
    localStorage.setItem(KEY, paneId);
  } catch {
    // sem armazenamento: vale até fechar o Regente
  }
}

/** Destino pré-selecionado: o último usado, se ainda estiver na lista; senão o primeiro. */
export function defaultTarget(targets: PaneRow[], last: string | null): string | null {
  return targets.find((p) => p.id === last)?.id ?? targets[0]?.id ?? null;
}
