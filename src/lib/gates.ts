/** Opções de um gate (JSON guardado no banco). Entrada inválida = nenhuma opção. */
export function parseOptions(json: string | null): string[] {
  try {
    const v: unknown = json ? JSON.parse(json) : [];
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

/** Opção de ajuste: exige uma observação antes de enviar. */
export const isAdjust = (option: string) => /ajust/i.test(option);

/** Onde o usuário digita: um gate que chega não abre o diálogo por cima disso (só a faixa). */
const TYPING = "input, textarea, select, [contenteditable='true'], .xterm, .monaco-editor";
export const isTyping = (el: Element | null) => !!el?.closest?.(TYPING);

/** Rótulo do botão: primeira letra maiúscula. */
export const optionLabel = (option: string) => option.charAt(0).toUpperCase() + option.slice(1);
