/** Último texto do agente numa conversa em Markdown (`transcript.rs`: turnos "## Você" e "## Claude").
 *  Vai do último "## Claude" até o próximo "## Você" (ou o fim); os blocos de ferramenta `<details>` saem.
 *  Null se não há resposta do agente. */
export function lastAgentReply(md: string): string | null {
  const head = "## Claude\n\n";
  const start = md.lastIndexOf(head);
  if (start < 0) return null;
  const body = md.slice(start + head.length);
  const next = body.search(/^## Você\n\n/m);
  const text = (next < 0 ? body : body.slice(0, next)).replace(/<details>[\s\S]*?<\/details>\s*/g, "").trim();
  return text || null;
}
