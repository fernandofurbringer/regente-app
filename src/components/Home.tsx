import { ask, open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { fmtUsd } from "../lib/format";
import { api, errMsg, type RecentMission } from "../lib/ipc";
import { isRemote } from "../lib/remote";
import { agoText, groupByLastUse } from "../lib/recent";
import { useStore } from "../state/store";
import QuickStart from "./QuickStart";
import RemoteOpenDialog from "./RemoteOpenDialog";

const STATUS: Record<string, string> = { ativa: "ativa", pronta: "pronta", bloqueada: "bloqueada", concluida: "concluída" };

export default function Home() {
  const workspaces = useStore((s) => s.workspaces);
  const openWorkspace = useStore((s) => s.openWorkspace);
  const { patch, toast, closeTab, selectMission } = useStore.getState();
  const [remoteOpen, setRemoteOpen] = useState(false);
  const [recent, setRecent] = useState<RecentMission[] | null>(null);

  useEffect(() => {
    api.recentMissions(10).then(setRecent).catch((e) => toast(errMsg(e), "erro"));
  }, [toast]);

  async function pick() {
    const dir = await open({ directory: true, title: "Escolher pasta de trabalho" });
    if (typeof dir === "string") await openWorkspace(dir);
  }
  async function remove(id: string, name: string) {
    if (!(await ask(`Remover ${name} da lista? Os terminais abertos dele serão fechados.`, { title: "Remover workspace", kind: "warning" }))) return;
    try {
      if (useStore.getState().tabs.includes(id) && !(await closeTab(id))) return;
      await api.workspaceRemove(id);
      patch({ workspaces: useStore.getState().workspaces.filter((w) => w.id !== id) });
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  /** Abre o workspace da missão e seleciona a missão; painéis hibernados acordam ao focar, como sempre. */
  async function resume(m: RecentMission) {
    const ws = await openWorkspace(m.workspacePath);
    if (ws) selectMission(ws.id, m.id);
  }

  const now = Date.now();
  const groups = groupByLastUse(workspaces, (w) => w.lastOpenedAt, new Date(now));

  return (
    <div className="home">
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 16 }}>
        <h1>Workspaces</h1>
        <div className="row">
          <button className="btn" onClick={() => setRemoteOpen(true)}>Abrir remoto</button>
          <button className="btn primary" onClick={pick}>Abrir pasta</button>
        </div>
      </div>
      {remoteOpen && <RemoteOpenDialog onClose={() => setRemoteOpen(false)} />}

      {recent && recent.length > 0 && (
        <section aria-label="Continuar de onde parei" style={{ marginBottom: 20 }}>
          <h2>Continuar de onde parei</h2>
          {recent.map((m) => (
            <div key={m.id} className="card" style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
              <div>
                <strong>{m.name} <span className="badge">{STATUS[m.status] ?? m.status}</span></strong>
                <small style={{ display: "block", color: "var(--fg3)" }}>
                  {m.workspaceName} · {m.livePanes} painel(is) vivo(s) · {fmtUsd(m.costUsd)}
                  {m.unseen > 0 && ` · ${m.unseen} entrega(s) nova(s)`}
                  {m.openGates > 0 && ` · ${m.openGates} gate(s) aberto(s)`}
                  {` · ${agoText(m.lastActivity, now)}`}
                </small>
              </div>
              <button className="btn primary" aria-label={`Retomar ${m.name}`} onClick={() => resume(m).catch((e) => toast(errMsg(e), "erro"))}>Retomar</button>
            </div>
          ))}
        </section>
      )}

      {workspaces.length === 0 && (
        <>
          <div className="empty">Nenhum workspace ainda. Abra uma pasta de projeto para começar.</div>
          <QuickStart />
        </>
      )}
      {groups.map(([group, list]) => (
        <section key={group} aria-label={`Workspaces: ${group}`}>
          <h3 className="meta" style={{ margin: "12px 0 6px" }}>{group}</h3>
          <div className="ws-list">
            {list.map((w) => (
              <div key={w.id} className="row">
                <button className="ws-card" onClick={() => openWorkspace(w.path)}>
                  <div>
                    <strong>{w.name} {isRemote(w.path) && <span className="badge" title="workspace remoto (SSH)">SSH</span>}</strong>
                    <small>{w.path} · aberto em {new Date(w.lastOpenedAt).toLocaleString("pt-BR")}</small>
                  </div>
                </button>
                <button className="icon-btn" aria-label={`Remover ${w.name} da lista`} onClick={() => remove(w.id, w.name)}>✕</button>
              </div>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
