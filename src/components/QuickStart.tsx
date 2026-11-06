import { open } from "@tauri-apps/plugin-dialog";
import { api, errMsg } from "../lib/ipc";
import { defaultAgentLabel, ensureWorkspace, firstMissionInput, visibleCards, type QuickCard } from "../lib/quickstart";
import { readLastCli } from "../lib/lastAgent";
import { useStore, wsOf } from "../state/store";

/** Cards de início rápido na Home vazia (primeira execução). Fechar um card grava o id em `quickstartDismissed`. */
export default function QuickStart() {
  const settings = useStore((s) => s.settings);
  const agent = defaultAgentLabel(useStore((s) => s.detected), readLastCli(), useStore((s) => s.providers)?.clis ?? []);
  if (!settings) return null;
  const { patch, toast, saveSettings } = useStore.getState();
  const cards = visibleCards(settings.quickstartDismissed ?? [], agent.label);

  const dismiss = (id: string) =>
    saveSettings({ ...settings, quickstartDismissed: [...(settings.quickstartDismissed ?? []), id] }).catch((e) => toast(errMsg(e), "erro"));

  // sem pasta aberta, os cards que precisam de uma pedem a pasta antes (cancelar não faz nada)
  const pickFolder = () => open({ directory: true, title: "Escolher pasta de trabalho" }).then((d) => (typeof d === "string" ? d : null));
  async function run(card: QuickCard) {
    const st = useStore.getState();
    switch (card.id) {
      case "pasta": {
        const dir = await pickFolder();
        if (dir) await st.openWorkspace(dir);
        return;
      }
      case "primeira": {
        const ws = await ensureWorkspace(wsOf(st.active), pickFolder, (p) => st.openWorkspace(p));
        if (!ws) return;
        const m = await api.missionCreate(firstMissionInput(ws.id, ws.path, agent.cli));
        await st.loadMissions(ws.id);
        st.selectMission(ws.id, m.id);
        return;
      }
      case "squad": {
        const ws = await ensureWorkspace(wsOf(st.active), pickFolder, (p) => st.openWorkspace(p));
        if (!ws) return;
        patch({ dialog: "missao", missionPrefill: { mode: "squad", brief: "" } });
        return;
      }
      case "voz":
        patch({ settingsOpen: true, settingsSection: "voz" });
        return;
      case "celular":
        patch({ settingsOpen: true, settingsSection: "celular" });
        return;
      case "atalhos":
        patch({ settingsOpen: true, settingsSection: "atalhos" });
        return;
    }
  }

  return (
    <section aria-label="Início rápido" style={{ marginTop: 16 }}>
      <h2>Início rápido</h2>
      {cards.length === 0 && (
        <p className="meta">Os cards foram fechados. Para vê-los de novo, use a paleta de comandos (Ctrl+Shift+P): "Mostrar cards de início".</p>
      )}
      {cards.map((c) => (
        <div key={c.id} className="card">
          <div className="row" style={{ justifyContent: "space-between" }}>
            <strong>{c.title}</strong>
            <button className="icon-btn" aria-label={`Fechar card ${c.title}`} onClick={() => dismiss(c.id)}>✕</button>
          </div>
          <p className="meta">{c.text}</p>
          <button className="btn" onClick={() => run(c).catch((e) => toast(errMsg(e), "erro"))}>{c.action}</button>
        </div>
      ))}
    </section>
  );
}
