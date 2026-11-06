import { expandPrompt, type PromptValues } from "./promptVars";
import { capText } from "./sendToAgent";

/** Arquivo colado com `@arquivo+:` só entra inteiro até este tamanho; acima, vai só o caminho. */
export const FILE_MAX = 8 * 1024;
/** Últimas linhas do terminal que `@terminal:` leva. */
export const TERMINAL_LINES = 80;
const BLOCK_MAX = 8 * 1024;
export const DIFF_MAX = 30 * 1024;

/** O que a expansão precisa do mundo (o compositor entrega; o teste entrega falsos). Sem valor, a menção fica como texto. */
export interface MentionEnv {
  readFile(path: string): Promise<string | null>;
  selection(): string;
  diff(): Promise<string>;
  /** último bloco de erro: do painel `ref` ou, sem ref, de qualquer terminal do workspace */
  lastError(ref: string | null): string | null;
  /** últimas linhas do terminal `ref`; null se o painel não existe */
  terminalTail(ref: string): string | null;
  /** texto do prompt salvo; null se não existe */
  prompt(name: string): Promise<string | null>;
  promptValues(text: string): Promise<PromptValues>;
}

const PATH_END = String.raw`\S*[^\s.,;:)\]]`;
// `(?<![\w@])`: nada de menção no meio de palavra ou e-mail. Palavras-chave exigem fronteira depois (`@erros` não expande).
const MENTION = new RegExp(
  String.raw`(?<![\w@])@(arquivo\+?:${PATH_END}|terminal:${PATH_END}|url:${PATH_END}|prompt:${PATH_END}|erro(?::${PATH_END})?|seleção|selecao|diff)(?![\p{L}\p{N}_])`,
  "gu",
);

/** Caminho da menção fica no workspace: sem absoluto (`/x`, `C:\x`) e sem segmento `..`. */
export function inScope(p: string): boolean {
  return !/^([a-zA-Z]:|[\\/])/.test(p) && !p.split(/[\\/]/).includes("..");
}

/** Cerca o texto; se ele já tem cercas, usa quatro crases. */
export function fence(text: string, lang = ""): string {
  const f = text.includes("```") ? "````" : "```";
  return `${f}${lang}\n${text}\n${f}`;
}

/** Troca as menções do compositor pelo conteúdo que elas dizem. Uma passada: o texto colado nunca é expandido de novo.
 *  Menção desconhecida ou sem valor fica como está. */
export async function expandMentions(text: string, env: MentionEnv): Promise<string> {
  const matches = [...text.matchAll(MENTION)];
  if (!matches.length) return text;
  const out: string[] = [];
  let last = 0;
  for (const m of matches) {
    const at = m.index ?? 0;
    out.push(text.slice(last, at));
    out.push((await expandOne(m[1], env)) ?? m[0]);
    last = at + m[0].length;
  }
  out.push(text.slice(last));
  return out.join("");
}

async function expandOne(body: string, env: MentionEnv): Promise<string | null> {
  if (body === "seleção" || body === "selecao") {
    const s = env.selection().trim();
    return s ? fence(capText(s, BLOCK_MAX)) : "(nenhuma seleção no editor)";
  }
  if (body === "diff") {
    const d = (await env.diff()).trim();
    return d ? fence(capText(d, DIFF_MAX), "diff") : "(sem alterações no workspace)";
  }
  if (body === "erro" || body.startsWith("erro:")) {
    const e = env.lastError(body === "erro" ? null : body.slice(5));
    return e === null ? null : fence(capText(e, BLOCK_MAX));
  }
  if (body.startsWith("terminal:")) {
    const t = env.terminalTail(body.slice(9));
    return t === null ? null : fence(capText(t, BLOCK_MAX));
  }
  if (body.startsWith("url:")) return body.slice(4);
  if (body.startsWith("arquivo+:") || body.startsWith("arquivo:")) {
    const full = body.startsWith("arquivo+:");
    const path = body.slice(full ? 9 : 8);
    if (!inScope(path)) return `@${body} (fora do workspace)`;
    if (!full) return path;
    const content = await env.readFile(path);
    if (content === null) return `(não consegui ler: ${path})`;
    return content.length <= FILE_MAX ? `${path}:\n${fence(content)}` : `${path} (grande demais para colar; o agente lê pelo caminho)`;
  }
  if (body.startsWith("prompt:")) {
    const p = await env.prompt(body.slice(7));
    if (p === null) return null;
    return expandPrompt(p, await env.promptValues(p));
  }
  return null;
}

