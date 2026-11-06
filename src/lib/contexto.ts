/** Faixa do medidor de contexto: bom < 50 %, médio até 75 %, ruim acima. */
export function faixaContexto(pct: number): "bom" | "medio" | "ruim" {
  return pct > 75 ? "ruim" : pct >= 50 ? "medio" : "bom";
}

/** "Contexto 62% (124 mil de 200 mil tokens)"; sem tokens ou janela, só a porcentagem. */
export function textoContexto(pct: number, tokens: number | null, janela: number | null): string {
  const p = Math.max(0, Math.min(100, Math.round(pct)));
  const mil = (n: number) => `${Math.round(n / 1000)} mil`;
  const detalhe = tokens != null && janela ? ` (${mil(tokens)} de ${mil(janela)} tokens)` : "";
  return `Contexto ${p}%${detalhe}`;
}

/** Texto do chip de proposta do medidor: "Compactar" ou "Recomeçar com memória". Outra ação: nenhum chip. */
export function rotuloProposta(acao: string | null | undefined): string | null {
  if (acao === "compactar") return "Compactar";
  if (acao === "recomecar") return "Recomeçar com memória";
  return null;
}
