/** Aba vizinha da ativa (Ctrl+PageDown = +1, Ctrl+PageUp = -1), dando a volta nas pontas. Sem ativa (ou ativa fora da
 *  lista), vai para a primeira (+1) ou a última (-1); sem abas, null. */
export function vizinha(paths: string[], ativa: string | null, passo: 1 | -1): string | null {
  if (!paths.length) return null;
  const i = ativa === null ? -1 : paths.indexOf(ativa);
  if (i < 0) return paths[passo === 1 ? 0 : paths.length - 1];
  return paths[(i + passo + paths.length) % paths.length];
}

export type Fechar = "outros" | "salvos" | "todos";

/** Caminhos que "Fechar outros/salvos/todos" fecham, na ordem das abas; `alvo` é a aba do menu (fica em "outros"). */
export function paraFechar(kind: Fechar, docs: { path: string; dirty: boolean }[], alvo: string): string[] {
  if (kind === "outros") return docs.filter((d) => d.path !== alvo).map((d) => d.path);
  if (kind === "salvos") return docs.filter((d) => !d.dirty).map((d) => d.path);
  return docs.map((d) => d.path);
}
