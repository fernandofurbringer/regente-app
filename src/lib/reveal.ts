import type { PaneGroup } from "./ipc";

/** O que esconde um painel na tela do workspace dele, e o que mudar para ele aparecer (puro; `goToPane` aplica). */
export interface Revelar {
  /** outra missão está filtrada: escolher a do painel (null = todas) */
  missao?: string | null;
  /** o grupo do painel está recolhido */
  abrirGrupo?: string;
  /** outro painel está em foco (os demais somem) */
  sairDoFoco?: boolean;
  /** a visão de arquivos está aberta no lugar dos terminais */
  terminais?: boolean;
}

export function revelar(
  pane: { id: string; missionId: string | null; groupId: string | null },
  st: { missionSel: string | null; groups: PaneGroup[]; focus: string | null; view: string | undefined },
): Revelar {
  const r: Revelar = {};
  if (st.missionSel && st.missionSel !== pane.missionId) r.missao = pane.missionId ?? null;
  const g = pane.groupId ? st.groups.find((x) => x.id === pane.groupId) : undefined;
  if (g?.collapsed) r.abrirGrupo = g.id;
  if (st.focus && st.focus !== pane.id) r.sairDoFoco = true;
  if ((st.view ?? "terminais") !== "terminais") r.terminais = true;
  return r;
}
