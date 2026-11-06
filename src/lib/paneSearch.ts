import type { PaneHit, PaneSearch } from "./ipc";

/** Selo de cada fonte de resultado. */
export const SOURCE_LABEL: Record<PaneHit["source"], string> = { terminal: "terminal", conversa: "conversa", historico: "no histórico" };

/** Frase de estado da busca (global e do painel). */
export function searchSummary(r: PaneSearch, q: string): string {
  const n = r.hits.length;
  let t = n === 0 ? `Nenhum resultado para "${q}".` : `${n}${r.truncated ? " ou mais" : ""} resultado(s) para "${q}".`;
  if (r.truncated) t += " Limite de 200 atingido: refine a busca.";
  if (r.diskLimited) t += " O histórico em disco foi lido só em parte (os trechos mais antigos ficaram de fora).";
  return t;
}
