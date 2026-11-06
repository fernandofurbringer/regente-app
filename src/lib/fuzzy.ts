/** Busca difusa da paleta: as letras da busca aparecem no texto na mesma ordem. Sem deps. */

const norm = (s: string) => s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
const BOUNDARY = /[\s/\\:._-]/;

/** Nota de quão bem `query` casa com `text` (maior é melhor), ou null se alguma letra não aparece. Busca vazia = 0. */
export function fuzzyScore(query: string, text: string): number | null {
  const q = norm(query.trim()).replace(/\s+/g, "");
  if (!q) return 0;
  const t = norm(text);
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const ch of q) {
    const at = t.indexOf(ch, from);
    if (at < 0) return null;
    score += 1;
    if (at === 0 || BOUNDARY.test(t[at - 1])) score += 3; // início de palavra
    if (at === prev + 1) score += 2; // letras seguidas
    score -= (at - from) * 0.1; // pulos grandes pesam
    prev = at;
    from = at + 1;
  }
  return score - t.length * 0.01; // com o mesmo casamento, texto mais curto ganha
}

/** Filtra e ordena por nota (estável para empates). Busca vazia devolve tudo na ordem original. */
export function filterByQuery<T>(items: T[], query: string, textOf: (item: T) => string): T[] {
  if (!query.trim()) return items;
  return items
    .map((item, i) => ({ item, i, score: fuzzyScore(query, textOf(item)) }))
    .filter((x): x is { item: T; i: number; score: number } => x.score !== null)
    .sort((a, b) => b.score - a.score || a.i - b.i)
    .map((x) => x.item);
}
