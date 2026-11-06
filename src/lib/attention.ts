import type { GateOpen, MissionAttention, PaneRow } from "./ipc";

export interface AttentionChip { text: string; tone: "erro" | "concluido" | "pergunta"; explain: string }

const REASON: Record<string, AttentionChip> = {
  pergunta: { text: "pergunta", tone: "pergunta", explain: "um painel está fazendo uma pergunta" },
  permissao: { text: "permissão", tone: "pergunta", explain: "um painel pede permissão para uma ação" },
  falha: { text: "falha", tone: "erro", explain: "um painel parou com erro" },
};

/** Pura: chips do rail para o estado de atenção (vazio se a missão está tranquila). */
export function attentionChips(a?: MissionAttention | null): AttentionChip[] {
  if (!a) return [];
  const out: AttentionChip[] = [];
  if (a.blocked) out.push({ text: "bloqueada", tone: "erro", explain: "gate aberto ou um painel esperando sua resposta" });
  if (a.ready) out.push({ text: "pronta para integrar", tone: "concluido", explain: "todos os ajudantes entregaram e nenhum painel trabalha" });
  for (const r of a.reasons) if (REASON[r]) out.push(REASON[r]);
  return out;
}

/** Pura: estado zerado (missão saiu da atenção); o store tira a missão em vez de guardá-la. */
export function isCleared(a: MissionAttention): boolean {
  return !a.blocked && !a.ready && a.reasons.length === 0;
}

/** Pura: tooltip do estado, uma linha por motivo. */
export function attentionTooltip(a?: MissionAttention | null): string {
  return attentionChips(a).map((c) => `${c.text}: ${c.explain}`).join("\n");
}

/** Pura: painéis que esperam você (pergunta ou erro, ou com gate aberto), o que está há mais tempo no estado primeiro.
 *  `since`: ms em que cada painel entrou no status atual. Gate aberto conta desde a criação dele. */
export function waitingPanes(panes: PaneRow[], since: Record<string, number>, gates: GateOpen[] = []): PaneRow[] {
  const gateAt = new Map<string, number>();
  for (const g of gates) if (g.decidedAt === null) gateAt.set(g.paneId, Math.min(gateAt.get(g.paneId) ?? Infinity, g.createdAt));
  const key = (p: PaneRow) => Math.min(since[p.id] ?? Infinity, gateAt.get(p.id) ?? Infinity);
  return panes
    .filter((p) => p.closedAt === null && (p.status === "pergunta" || p.status === "erro" || gateAt.has(p.id)))
    .sort((a, b) => {
      const ka = key(a);
      const kb = key(b);
      return ka === kb ? 0 : ka < kb ? -1 : 1;
    });
}

/** Pura: "45s" ou "2m10" (minutos e segundos com dois dígitos). */
export function elapsedText(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}`;
}

/** Pura: chip de texto do cabeçalho para os estados que pedem atenção ou mostram trabalho; null para os demais. */
export function paneStateChip(status: string, since: number | undefined, now: number): { text: string; tone: "pergunta" | "erro" | "hibernado" | "trabalhando" } | null {
  if (status === "pergunta") return { text: "Aguardando resposta", tone: "pergunta" };
  if (status === "erro") return { text: "Erro", tone: "erro" };
  if (status === "hibernado") return { text: "Hibernado", tone: "hibernado" };
  if (status === "trabalhando") return { text: since === undefined ? "Trabalhando" : `Trabalhando ${elapsedText(now - since)}`, tone: "trabalhando" };
  return null;
}

/** Pura: agente em trabalho ou esperando sua resposta; fechar o painel encerra a conversa em curso (B-14). */
export function paneBusy(p: PaneRow): boolean {
  return p.kind === "agent" && p.closedAt === null && (p.status === "trabalhando" || p.status === "pergunta");
}

/** Pura: pergunta antes de fechar um painel ocupado (✕, menu e paleta usam esta regra); null = fecha direto. */
export function closeQuestion(p: PaneRow): string | null {
  if (!paneBusy(p)) return null;
  return `Fechar "${p.name}"? Ele está ${p.status === "pergunta" ? "esperando sua resposta" : "trabalhando agora"}.`;
}

/** Pura: painéis abertos ocupados (paneBusy) ou com gate aberto; é o N do aviso ao fechar o app (B-07). */
export function busyCount(panes: PaneRow[], gates: GateOpen[]): number {
  const gated = new Set(gates.filter((g) => g.decidedAt === null).map((g) => g.paneId));
  return panes.filter((p) => p.closedAt === null && (paneBusy(p) || gated.has(p.id))).length;
}

/** Pura: fechar o app pede confirmação só quando há agente ocupado ou gate aberto. */
export function needsCloseConfirm(panes: PaneRow[], gates: GateOpen[]): boolean {
  return busyCount(panes, gates) > 0;
}
