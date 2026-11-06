import { ask } from "@tauri-apps/plugin-dialog";
import { api, errMsg, type PaneStatus } from "../lib/ipc";
import { useStore } from "../state/store";
import { terms } from "./TerminalPane";

const RODANDO: PaneStatus[] = ["iniciando", "trabalhando", "ocioso", "pergunta"];

/**
 * Continua o painel na conta `conta`, retomando a conversa. Rodando: reinicia na conta nova (se parou no limite,
 * recebe "continue" ao acordar). Hibernado ou encerrado: a conta vale na próxima vez que ele voltar.
 */
export async function continueInAccount(paneId: string, conta: string): Promise<void> {
  const st = useStore.getState();
  const pane = st.panes[paneId];
  if (!pane) return;
  if (pane.status === "trabalhando") {
    const ok = await ask(`${pane.name} está trabalhando. Reiniciar agora na conta ${conta}? A conversa é retomada, mas o que estiver em andamento para.`, { title: "Continuar em outra conta", kind: "warning" });
    if (!ok) return;
  }
  const rodando = RODANDO.includes(pane.status);
  try {
    const frame = rodando ? terms.get(pane.id)?.screen() ?? "" : "";
    const row = await api.paneSwitchAccount(pane.id, conta, frame);
    st.upsertPane(row);
    st.toast(rodando ? `${pane.name} continua na conta ${conta}` : `${pane.name} passa para a conta ${conta} quando voltar`, "ok");
  } catch (e) {
    st.toast(errMsg(e), "erro");
  }
}
