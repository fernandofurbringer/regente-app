/** Toast do foco para os painéis que avisaram com a janela fora de foco (do mais antigo ao mais novo). Botão para o mais
 *  novo e, com dois ou mais, para o anterior; os nomes de todos vão no texto. Painel que já fechou fica de fora. */
export function pendingNoticeToast(ids: string[], nameOf: (id: string) => string | undefined): { text: string; go: { id: string; label: string }; alt?: { id: string; label: string } } | null {
  const vivos = ids.filter((id) => nameOf(id) !== undefined).reverse();
  if (!vivos.length) return null;
  const nome = (id: string) => nameOf(id) ?? id;
  const [novo, anterior] = vivos;
  if (vivos.length === 1) return { text: `${nome(novo)} avisou enquanto o Regente estava fora de foco.`, go: { id: novo, label: "Ir para o painel" } };
  return {
    text: `${vivos.length} painéis avisaram enquanto o Regente estava fora de foco: ${vivos.map(nome).join(", ")}.`,
    go: { id: novo, label: `Ir para ${nome(novo)}` },
    alt: { id: anterior, label: `Ir para ${nome(anterior)}` },
  };
}
