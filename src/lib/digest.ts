// "Enquanto você estava fora" (A1.6, RM-43): monta o resumo por missão sem LLM, a partir do estado da loja e dos dados
// que `away_data` devolve. Função pura: o componente junta as fontes e chama `buildDigest`.

export interface DigestMission {
  id: string;
  name: string;
  status: string;
  files: number;
  add: number;
  del: number;
  costUsd: number;
  verify: { resultado: { ok: boolean; durationMs: number }; em: number } | null;
}
export interface DigestHandoff { missionId: string | null; fromPane: string; summary: string; createdAt: number }
export interface DigestPane { id: string; missionId: string | null; name: string; status: string }
export interface DigestGate { id: string; missionId: string | null; title: string }

export interface DigestInput {
  since: number;
  missions: DigestMission[];
  panes: DigestPane[];
  /** quando cada painel entrou no status atual (ms) */
  statusSince: Record<string, number>;
  gates: DigestGate[];
  handoffs: DigestHandoff[];
}

export interface DigestSection {
  missionId: string;
  name: string;
  /** agentes que terminaram (ou pararam com erro) desde a saída */
  finished: string[];
  /** perguntas esperando resposta agora */
  waiting: string[];
  gates: string[];
  handoffs: string[];
  files: string | null;
  cost: string | null;
  verify: string | null;
}

const DONE = new Set(["ocioso", "concluido", "erro"]);

/** Uma seção por missão que tenha algo a mostrar. Ordem: a das missões como vieram. */
export function buildDigest(input: DigestInput): DigestSection[] {
  const { since, panes, statusSince, gates, handoffs } = input;
  const nomePainel = (id: string) => panes.find((p) => p.id === id)?.name ?? "agente";
  const secoes: DigestSection[] = [];
  for (const m of input.missions) {
    const deMissao = panes.filter((p) => p.missionId === m.id);
    const finished = deMissao
      .filter((p) => DONE.has(p.status) && (statusSince[p.id] ?? 0) >= since)
      .map((p) => `${p.name} ${p.status === "erro" ? "parou com erro" : "terminou"}`);
    const waiting = deMissao.filter((p) => p.status === "pergunta").map((p) => `${p.name} aguarda resposta`);
    const gateTitulos = gates.filter((g) => g.missionId === m.id).map((g) => g.title);
    const entregas = handoffs
      .filter((h) => h.missionId === m.id && h.createdAt >= since)
      .map((h) => `${nomePainel(h.fromPane)}: ${h.summary}`);
    const verificacao = m.verify && m.verify.em >= since ? m.verify : null;
    const files = m.files > 0 ? `${m.files} arq. +${m.add} −${m.del}` : null;
    if (!finished.length && !waiting.length && !gateTitulos.length && !entregas.length && !files && !verificacao) continue;
    secoes.push({
      missionId: m.id,
      name: m.name,
      finished,
      waiting,
      gates: gateTitulos,
      handoffs: entregas,
      files,
      cost: m.costUsd > 0 ? `US$ ${m.costUsd.toFixed(2)} (total da missão)` : null,
      verify: verificacao ? `${verificacao.resultado.ok ? "✔" : "✘"} ${Math.round(verificacao.resultado.durationMs / 1000)} s` : null,
    });
  }
  return secoes;
}

/** Texto corrido das seções (o que vai ao "Resumir com IA" e ao "Copiar"). */
export function digestText(secoes: DigestSection[]): string {
  if (!secoes.length) return "Nada novo nas missões desde que você saiu.";
  return secoes
    .map((s) => {
      const linhas = [
        ...s.finished.map((x) => `- ${x}`),
        ...s.waiting.map((x) => `- ${x}`),
        ...s.gates.map((x) => `- portão aberto: ${x}`),
        ...s.handoffs.map((x) => `- entrega: ${x}`),
        ...(s.files ? [`- arquivos: ${s.files}`] : []),
        ...(s.verify ? [`- verificação: ${s.verify}`] : []),
        ...(s.cost ? [`- custo: ${s.cost}`] : []),
      ];
      return `${s.name}\n${linhas.join("\n")}`;
    })
    .join("\n\n");
}
