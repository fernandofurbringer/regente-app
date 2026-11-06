/** Anúncio curto, uma vez, quando o painel passa a hibernado (não na restauração nem na montagem). */
export function hibernatedAnnouncement(prev: string | undefined, next: string, name: string): string {
  return next === "hibernado" && prev !== undefined && prev !== "hibernado" ? `Painel ${name} hibernado` : "";
}

/** Texto do quadro congelado para o leitor de tela sob demanda (aria-describedby): as últimas linhas com conteúdo. */
export function frameDetail(frame: string, maxLines = 40): string {
  const lines = frame.replace(/\s+$/, "").split("\n");
  const kept = lines.slice(Math.max(0, lines.length - maxLines));
  const text = kept.join("\n").trim();
  return text ? `Última tela: ${text}` : "A última tela não tem texto.";
}
