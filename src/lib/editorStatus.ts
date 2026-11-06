/** O que a barra de status mostra do editor de um workspace. Publicado pelo EditorArea; null sem arquivo no editor. */
export interface EditorStatus {
  ln: number;
  col: number;
  /** caracteres selecionados (0 sem seleção) */
  sel: number;
  /** linhas que a seleção toca (0 sem seleção) */
  selLinhas: number;
  tabSize: number;
  insertSpaces: boolean;
  eol: "LF" | "CRLF";
  /** nome da linguagem para gente ("C#", "TypeScript") */
  lang: string;
  /** tamanho do arquivo no disco (texto salvo, em UTF-8) */
  bytes: number;
  dirty: boolean;
  readOnly: boolean;
}

/** "Ln 12, Col 5", com "(3 selecionados)" ou "(40 selecionados em 3 linhas)" quando há seleção. */
export function posTexto(s: Pick<EditorStatus, "ln" | "col" | "sel" | "selLinhas">): string {
  const base = `Ln ${s.ln}, Col ${s.col}`;
  if (!s.sel) return base;
  return s.selLinhas > 1 ? `${base} (${s.sel} selecionados em ${s.selLinhas} linhas)` : `${base} (${s.sel} selecionados)`;
}

/** "Espaços: 2" ou "Tamanho da tabulação: 4", como o VS Code. */
export function indentTexto(s: Pick<EditorStatus, "tabSize" | "insertSpaces">): string {
  return s.insertSpaces ? `Espaços: ${s.tabSize}` : `Tamanho da tabulação: ${s.tabSize}`;
}

/** Bytes do texto em UTF-8 (o que vai para o disco). */
export function bytesUtf8(text: string): number {
  return new TextEncoder().encode(text).length;
}
