/** Sugestões do briefing: cada chip acrescenta uma linha com rótulo, sem repetir o que já está escrito. */
export interface BriefChip { id: string; label: string; line: string }

export const BRIEF_CHIPS: BriefChip[] = [
  { id: "objetivo", label: "Objetivo", line: "Objetivo: " },
  { id: "publico", label: "Público", line: "Público: " },
  { id: "stack", label: "Stack", line: "Stack: " },
  { id: "criterios", label: "Critérios de aceite", line: "Critérios de aceite:\n- " },
  { id: "prazo", label: "Prazo", line: "Prazo: " },
  { id: "restricoes", label: "Restrições", line: "Restrições: " },
];

/** Acrescenta `line` ao fim do briefing, em linha própria. Se o rótulo já aparece no início de uma linha, não muda nada. */
export function insertChip(brief: string, line: string): string {
  const label = line.split(":")[0];
  if (new RegExp(`^\\s*${label}\\s*:`, "m").test(brief)) return brief;
  const base = brief.replace(/\s+$/, "");
  return base ? `${base}\n${line}` : line;
}
