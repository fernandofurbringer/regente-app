import { fmtTokens, fmtUsd, fmtUsdPt } from "./format";

/** Consumo de um painel: entrada e saída reais. O cache (re-cobrado a cada turno) não entra no número. */
export const paneUsageText = (tokIn: number, tokOut: number, costText: string) => `${fmtTokens(tokIn)} ent · ${fmtTokens(tokOut)} sai · ${costText}`;

/** Dica com o cache, sempre fora do total. */
export const cacheTip = (read: number, write: number) => `cache: ${fmtTokens(read)} lido / ${fmtTokens(write)} escrito — não conta`;

/** Missão e topo: entrada+saída (sem cache) e custo. */
export const usageTotalText = (tokens: number, costUsd: number) => `${fmtTokens(tokens)} ent+sai · ≈${fmtUsd(costUsd)}`;

/** Dica do topo: hoje · 7 dias · total em US$ e tokens sem cache (o cache não entra no número). */
export const hojeTip = (t: { today: { tokens: number; costUsd: number }; week: { tokens: number; costUsd: number }; tokens: number; costUsd: number }, daily: number | null) =>
  `Hoje ${fmtUsdPt(t.today.costUsd)} (${fmtTokens(t.today.tokens)} tok sem cache) · 7 dias ${fmtUsdPt(t.week.costUsd)} (${fmtTokens(t.week.tokens)} tok) · total ${fmtUsdPt(t.costUsd)} (${fmtTokens(t.tokens)} tok)`
  + (daily ? ` · orçamento diário US$ ${daily.toFixed(2).replace(".", ",")}` : "") + " · clique para o detalhe";
