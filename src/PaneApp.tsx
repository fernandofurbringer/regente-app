import { useEffect, useRef } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import PaneHeader from "./components/PaneHeader";
import { PaneContent } from "./components/PaneGrid";
import ConversationExport from "./components/ConversationExport";
import { ModelSwitchDialog } from "./components/ModelSwitch";
import Toasts from "./components/Toasts";
import { errMsg } from "./lib/ipc";
import { useStore } from "./state/store";
import { isInlineSubagent } from "./lib/activity";
import { useAppEvents, useFileDrop } from "./appEvents";

function PaneWindow({ wsId, paneId }: { wsId: string; paneId: string }) {
  useEffect(() => { useStore.getState().loadPanes(wsId); }, [wsId]);
  const pane = useStore((s) => s.panes[paneId]);
  // número = posição na grade do workspace
  const index = useStore((s) => Object.values(s.panes).filter((p) => p.workspaceId === wsId && !isInlineSubagent(p, s.settings?.subagentPanes ?? false)).sort((a, b) => a.ord - b.ord).findIndex((p) => p.id === paneId));
  const loaded = useRef(false);
  useEffect(() => {
    // o painel sumiu (fechado em outra janela): fecha esta
    if (pane) loaded.current = true;
    else if (loaded.current) getCurrentWindow().close().catch(() => {});
  }, [pane]);
  if (!pane) return <div className="empty">Carregando…</div>;
  return (
    <div className="app">
      <section className="pane" data-pane-id={pane.id} style={{ flex: 1, borderRadius: 0 }}>
        <PaneHeader pane={pane} index={index} standalone />
        <div className="pane-body"><PaneContent pane={pane} /></div>
      </section>
      <ConversationExport />
      <ModelSwitchDialog />
      <Toasts />
    </div>
  );
}

/** Janela avulsa de um painel (#/pane/<ws>/<pane>). Não usa atalhos nem o relatório de visibilidade. */
export default function PaneApp() {
  useAppEvents();
  useFileDrop();
  // sem atalhos aqui: useAppShortcuts (paleta) ficaria fora desta janela
  const hash = window.location.hash;
  useEffect(() => {
    useStore.getState().boot(false).catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  }, [hash]);
  const [wsId, paneId] = hash.slice(7).split("/");
  return <PaneWindow wsId={wsId} paneId={paneId} />;
}
