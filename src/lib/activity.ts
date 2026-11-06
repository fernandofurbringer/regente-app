import type { ExecRow, PaneRow } from "./ipc";

/** Subagente que mora na faixa do painel pai (ajuste inline): não entra na grade, na numeração nem nas contagens. */
export function isInlineSubagent(p: Pick<PaneRow, "kind">, subagentPanes: boolean): boolean {
  return p.kind === "subagent" && !subagentPanes;
}

const ATIVO = new Set<string>(["iniciando", "trabalhando", "pergunta"]);
const corta = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);
const dur = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return s < 60 ? `${s} s` : `${Math.round(s / 60)} min`;
};

/** Texto da faixa de atividade: subtarefas (quantas rodando) e o comando em andamento mais antigo com o tempo dele;
 *  sem comando em andamento, quantos comandos houve. null quando não há nada (a faixa some). */
export function activitySummary(
  subs: Pick<PaneRow, "status" | "name">[],
  execs: Pick<ExecRow, "status" | "command" | "description" | "startedAt" | "endedAt">[],
  now: number,
): string | null {
  const parts: string[] = [];
  if (subs.length) {
    const rodando = subs.filter((s) => ATIVO.has(s.status)).length;
    parts.push(`↳ ${subs.length} ${subs.length === 1 ? "subtarefa" : "subtarefas"}${rodando ? ` (${rodando} rodando)` : ""}`);
  }
  const vivos = execs.filter((e) => e.status === "rodando" || e.status === "fundo").sort((a, b) => a.startedAt - b.startedAt);
  // descrição vazia cai para o comando; quebras de linha viram espaço (a faixa é uma linha só)
  const rotulo = (e: { description: string | null; command: string }) => (e.description?.trim() || e.command).replace(/\s+/g, " ").trim();
  if (vivos.length) parts.push(`⚙ ${corta(rotulo(vivos[0]), 40)}`, dur(now - vivos[0].startedAt));
  else if (execs.length) parts.push(`⚙ ${execs.length} ${execs.length === 1 ? "comando" : "comandos"}`);
  return parts.length ? parts.join(" · ") : null;
}
