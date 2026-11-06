/** Variáveis dos prompts salvos. Expandidas no cliente, na hora de colar (uma passada: valor colado nunca é expandido de novo). */
export const PROMPT_VARS = ["clipboard", "arquivo", "selecao", "workspace", "branch"] as const;
export type PromptVar = (typeof PROMPT_VARS)[number];
export type PromptValues = Partial<Record<PromptVar, string>>;

const TOKEN = /\{\{\s*([a-z]+)\s*\}\}/g;
const isVar = (name: string): name is PromptVar => (PROMPT_VARS as readonly string[]).includes(name);

/** Troca as variáveis conhecidas; sem valor vira vazio. Variável desconhecida fica como está. */
export function expandPrompt(text: string, values: PromptValues): string {
  return text.replace(TOKEN, (token, name: string) => (isVar(name) ? values[name] ?? "" : token));
}

/** Variáveis conhecidas que o texto usa. */
export function promptVarsUsed(text: string): PromptVar[] {
  const out = new Set<PromptVar>();
  for (const m of text.matchAll(TOKEN)) if (isVar(m[1])) out.add(m[1]);
  return [...out];
}
