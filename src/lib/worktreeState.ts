import type { WorktreeInfo, WorktreeState } from "./ipc";
import { agoText } from "./recent";

/** Idade do último commit no mesmo formato do Início ("há 14 min", "há 5 h", "há 3 d"; "agora" abaixo de 1 min); null sem commit. */
export function commitAgeText(at: number | null, now: number): string | null {
  return at === null ? null : agoText(at, now);
}

/** Linha da barra da missão adotada: branch (ou "livre (base)"), ↑↓ contra a base, alterados e idade do commit. */
export function stateLine(st: WorktreeState, now: number): string {
  const age = commitAgeText(st.lastCommitAt, now);
  return [
    st.branch ?? `livre (${st.base})`,
    st.branch ? `↑${st.ahead} ↓${st.behind} vs ${st.base}` : null,
    st.dirty === 0 ? "limpo" : `${st.dirty} ${st.dirty === 1 ? "alterado" : "alterados"}`,
    age ? `commit ${age}` : null,
  ].filter(Boolean).join(" · ");
}

const dirKey = (p: string) => p.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
/** Adotadas abertas cuja branch no git (`git worktree list`) difere da gravada: o `ambiente.sh` trocou por fora. */
export function adotadasComBranchMudada(
  ms: { id: string; adopted: boolean; status: string; worktreePath: string | null; branch: string | null }[],
  wts: WorktreeInfo[],
): string[] {
  const viva = new Map(wts.map((w) => [dirKey(w.path), w.branch]));
  return ms
    .filter((m) => m.adopted && m.status !== "arquivada" && m.worktreePath && viva.has(dirKey(m.worktreePath)) && viva.get(dirKey(m.worktreePath)) !== m.branch)
    .map((m) => m.id);
}

/** Chip da aba na faixa: o fim da branch do worktree adotado (`spec/4821-x` vira `4821-x`), ou "livre"; null para missão comum. */
export function branchChip(m: { adopted: boolean; branch: string | null }): string | null {
  return m.adopted ? (m.branch?.split("/").pop() ?? "livre") : null;
}

/** Fechar não toca no worktree: só pergunta se algum painel ainda trabalha/pergunta ou se há arquivo não salvo nele. */
export function fechaDireto(statuses: string[], naoSalvo: boolean): boolean {
  return !naoSalvo && !statuses.some((s) => s === "trabalhando" || s === "pergunta" || s === "acordando" || s === "iniciando");
}
