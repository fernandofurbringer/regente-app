import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { errMsg } from "../lib/ipc";
import { isRemote } from "../lib/remote";
import { useStore } from "../state/store";

export default function WorkspaceTabs() {
  const tabs = useStore((s) => s.tabs);
  const active = useStore((s) => s.active);
  const workspaces = useStore((s) => s.workspaces);
  const { setActive, closeTab, patch, toast } = useStore.getState();

  async function detach(id: string, name: string) {
    const label = `ws-${id}`;
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) return existing.setFocus();
    if (!(await closeTab(id))) return;
    const w = new WebviewWindow(label, { url: `index.html#/ws/${id}`, title: `Regente — ${name}`, width: 1280, height: 820, dragDropEnabled: true });
    // a janela não abriu: a aba volta
    w.once("tauri://error", (e) => {
      const s = useStore.getState();
      patch({ tabs: s.tabs.includes(id) ? s.tabs : [...s.tabs, id], active: id });
      toast(`Não foi possível abrir a janela: ${errMsg(e.payload)}`, "erro");
    });
  }

  return (
    <div className="tabs" role="tablist">
      <button className={`tab ${active === null ? "active" : ""}`} role="tab" aria-selected={active === null} onClick={() => setActive(null)}>Início</button>
      {tabs.map((id) => {
        const w = workspaces.find((x) => x.id === id);
        if (!w) return null;
        return (
          <div key={id} className={`tab ${active === id ? "active" : ""}`} role="tab" aria-selected={active === id}>
            <button className="icon-btn" onClick={() => setActive(id)} title={w.path}>
              {isRemote(w.path) && <span className="badge" aria-label="remoto (SSH)" title="workspace remoto (SSH)">SSH</span>} {w.name}
            </button>
            <button className="icon-btn x" aria-label="Abrir em janela própria" title="Abrir em janela própria" onClick={() => detach(id, w.name).catch((e) => toast(errMsg(e), "erro"))}>⧉</button>
            <button className="icon-btn x" aria-label={`Fechar ${w.name}`} onClick={() => closeTab(id)}>✕</button>
          </div>
        );
      })}
    </div>
  );
}
