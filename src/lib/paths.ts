/** A path as the terminal should receive it: wrapped in quotes when it has a space. */
export function quoted(path: string): string {
  return /\s/.test(path) ? `"${path}"` : path;
}

/** Texto colado num terminal quando arquivos são soltados: caminhos citados, separados por espaço, sem Enter. */
export function formatDroppedPaths(paths: string[]): string {
  return paths.map(quoted).join(" ");
}
