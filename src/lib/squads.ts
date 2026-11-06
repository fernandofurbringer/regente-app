import type { FormerPane, SquadDef } from "./ipc";

/** Um passo por linha; linhas vazias somem. */
export const stepsToText = (steps: string[]) => steps.join("\n");
export const textToSteps = (text: string) => text.split("\n").map((s) => s.trim()).filter(Boolean);

/** Id de cópia para editar um original: `<id>-meu`, depois `<id>-meu-2`, … sem colidir com os ids existentes. */
export function copyId(id: string, taken: string[]): string {
  const base = `${id}-meu`;
  let out = base;
  for (let n = 2; taken.includes(out); n++) out = `${base}-${n}`;
  return out;
}

/** Quantos gates a squad tem (uma fase com título de gate = um gate). */
export const gateCount = (s: Pick<SquadDef, "phases">) => s.phases.filter((p) => !!p.gate).length;

/** Crase escrita como escape JSON (o texto literal, sem a crase): nunca fecha uma cerca de código. */
const BACKTICK_ESCAPE = ["\\", "u0060"].join("");

/**
 * Prompt do Squad Former: a análise das sessões recentes vira uma proposta de squad.
 * O JSON entra cercado por ```json, para o modelo ler sem confundir com o texto.
 */
export function squadFormerPrompt(panes: FormerPane[]): string {
  return [
    "Analise as 20 sessões recentes abaixo e proponha um squad (nome, intake, roster com agentes do catálogo — use agent_list —, fases e gates). Salve com squad_save e resuma o que criou.",
    "",
    "```json",
    // nomes de painel com crase quebrariam a cerca do bloco: a crase vira escape unicode (JSON continua válido)
    JSON.stringify(panes, null, 2).replace(/`/g, BACKTICK_ESCAPE),
    "```",
  ].join("\n");
}
