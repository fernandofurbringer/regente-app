/** Favoritos do catálogo: a chave diz o tipo e o id (`agent:<nome>`, `skill:<nome>`, `recipe:<id>`). */
export type FavKind = "agent" | "skill" | "recipe";

export const favKey = (kind: FavKind, id: string): string => `${kind}:${id}`;

/** Liga ou desliga um favorito; a lista não repete chaves. */
export function toggleFav(favs: string[], key: string): string[] {
  return favs.includes(key) ? favs.filter((k) => k !== key) : [...favs, key];
}

/** Favoritos primeiro, mantendo a ordem original dentro de cada grupo. */
export function sortFavFirst<T>(items: T[], keyOf: (item: T) => string, favs: string[]): T[] {
  const set = new Set(favs);
  return [...items.filter((i) => set.has(keyOf(i))), ...items.filter((i) => !set.has(keyOf(i)))];
}
