import { terms } from "../components/TerminalPane";
import type { PaneRow } from "./ipc";
import { useStore } from "../state/store";
import { revelar } from "./reveal";

const toast = (text: string) => useStore.getState().toast(text, "erro");

/** Foco no painel: o terminal recebe o foco e o Regente marca o painel ativo (ver TerminalPane). */
export function focusPane(p: PaneRow) {
  const t = terms.get(p.id);
  if (t) t.focus();
  else useStore.getState().patch({ activePane: p.id });
}

/** Leva ao painel e dá o foco ao terminal: troca de workspace e tira o que o esconde (outra missão filtrada, grupo
 *  recolhido, outro painel em foco, visão de arquivos). Usado pelo clique no aviso e pelo "ir para o painel". */
export function goToPane(id: string) {
  const st = useStore.getState();
  const pane = st.panes[id];
  if (!pane) return toast("Esse painel não está mais aberto.");
  const ws = pane.workspaceId;
  if (st.active !== ws) st.setActive(ws);
  const r = revelar(pane, { missionSel: st.missionSel[ws] ?? null, groups: st.groups[ws] ?? [], focus: st.focus[ws] ?? null, view: st.view[ws] });
  if (r.missao !== undefined) st.selectMission(ws, r.missao);
  if (r.abrirGrupo) void st.groupPatch(ws, r.abrirGrupo, { collapsed: false }).catch(() => {});
  if (r.sairDoFoco || r.terminais) {
    const cur = useStore.getState();
    st.patch({
      ...(r.sairDoFoco ? { focus: { ...cur.focus, [ws]: null } } : {}),
      ...(r.terminais ? { view: { ...cur.view, [ws]: "terminais" } } : {}),
    });
  }
  // o workspace novo monta a grade antes do foco
  setTimeout(() => focusPane(pane), 50);
}
