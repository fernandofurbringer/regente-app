import { api } from "./ipc";
import { useStore } from "../state/store";

/** Abre a saída inteira do painel (disco + memória) como texto somente leitura no editor de arquivos do workspace. */
export async function openPaneHistory(paneId: string, workspaceId: string): Promise<void> {
  const text = await api.paneHistory(paneId);
  const st = useStore.getState();
  st.patch({
    openFile: { ...st.openFile, [workspaceId]: { path: `historico-${paneId}.txt`, text } },
    view: { ...st.view, [workspaceId]: "arquivos" },
  });
}
