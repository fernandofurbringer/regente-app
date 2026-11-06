import { fmtUsd } from "./format";

export const NO_COST_TITLE = "esta IA não grava uso localmente";

/** Custo de um painel no cabeçalho. Sem medição local mostra "custo n/d", nunca um $0,00 que engana. */
export function costLabel(opts: { cliCost: string | null | undefined; remoteNoCost: boolean; usd: number }): { text: string; title?: string } {
  if (opts.cliCost === null) return { text: "custo n/d", title: NO_COST_TITLE };
  if (opts.remoteNoCost) return { text: "custo n/d", title: "por ssh, o custo só é lido para o Claude" };
  return { text: `≈${fmtUsd(opts.usd)}` };
}
