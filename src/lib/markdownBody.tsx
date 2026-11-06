import Markdown from "react-markdown";
import { MD } from "./markdown";

/** Renderizador do AgentMarkdown. Fica em módulo próprio para carregar sob demanda (ver lib/markdown.tsx). */
export default function MarkdownBody({ children }: { children: string }) {
  return <Markdown components={MD} disallowedElements={["img"]} unwrapDisallowed>{children}</Markdown>;
}
