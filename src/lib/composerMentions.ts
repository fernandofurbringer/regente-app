/** Menções (`@arquivo:…`) para o compositor de um painel, mesmo que ele ainda não esteja aberto. Quem monta o compositor as recebe. */
const queued = new Map<string, string[]>();
export const MENTION_EVENT = "regente:composer-mention";

export function queueMention(paneId: string, text: string): void {
  queued.set(paneId, [...(queued.get(paneId) ?? []), text]);
  // fora do navegador (testes) não há janela para avisar: a fila já está guardada
  if (typeof window !== "undefined") window.dispatchEvent(new CustomEvent(MENTION_EVENT, { detail: { paneId } }));
}

/** Entrega e esquece as menções pendentes do painel, na ordem em que chegaram. */
export function takeMentions(paneId: string): string[] {
  const out = queued.get(paneId) ?? [];
  queued.delete(paneId);
  return out;
}
