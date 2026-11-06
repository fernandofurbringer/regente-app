/** Avisos visíveis de uma vez; o resto espera na fila e entra quando um sai. */
export const TOAST_VISIBLE = 4;
/** Histórico do sino: os últimos avisos. */
export const TOAST_LOG_MAX = 20;

export function visibleToasts<T>(queue: T[]): { shown: T[]; hidden: number } {
  return { shown: queue.slice(0, TOAST_VISIBLE), hidden: Math.max(0, queue.length - TOAST_VISIBLE) };
}

/** Mais novo primeiro, no máximo `TOAST_LOG_MAX`. */
export function pushLog<T>(log: T[], t: T): T[] {
  return [t, ...log].slice(0, TOAST_LOG_MAX);
}
