import type { SessionInfo } from "./ipc";

/** Tamanho do arquivo de sessão em texto curto (KB ou MB). */
export function fmtSessionSize(bytes: number): string {
  return bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** Filtra pela prévia ou pelo nome da IA (sem diferença de maiúsculas). Busca vazia = todas. */
export function filterSessions(list: SessionInfo[], query: string): SessionInfo[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter((s) => (s.prompt ?? "").toLowerCase().includes(q) || s.cli.toLowerCase().includes(q));
}

/** A conversa mais recente da pasta, de qualquer IA (ou null se não há nenhuma). */
export function latestSession(list: SessionInfo[]): SessionInfo | null {
  return list.reduce<SessionInfo | null>((best, s) => (!best || s.modifiedMs > best.modifiedMs ? s : best), null);
}
