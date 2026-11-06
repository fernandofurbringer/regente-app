import { useEffect, useState } from "react";
import { api, errMsg } from "../lib/ipc";
import { isRemote, safeSessionName } from "../lib/remote";
import { useStore } from "../state/store";

/** Sessões tmux que o Regente não criou, no servidor de um workspace remoto. [Anexar] abre um painel ligado a uma delas. */
export default function ExternalSessions({ wsId }: { wsId: string }) {
  const ws = useStore((s) => s.workspaces.find((w) => w.id === wsId));
  const remote = !!ws && isRemote(ws.path);
  const [names, setNames] = useState<string[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!remote) return;
    let alive = true;
    setNames(null);
    setErr(null);
    api.remoteSessions(wsId).then((n) => alive && setNames(n)).catch((e) => alive && setErr(errMsg(e)));
    return () => {
      alive = false;
    };
  }, [wsId, remote, tick]);

  if (!remote || !ws) return null;
  const path = ws.path;
  const attach = (name: string) =>
    api.paneSpawn({ workspaceId: wsId, kind: "terminal", cwd: path, name: `${name} (externa)`, attachSession: name })
      .catch((e) => useStore.getState().toastErr(e));

  return (
    <>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <h3>Sessões externas</h3>
        <button className="icon-btn" aria-label="Atualizar sessões externas" onClick={() => setTick((t) => t + 1)}>↻</button>
      </div>
      {err && <div className="empty" style={{ padding: 8 }}>{err}</div>}
      {!err && names && names.length === 0 && <div className="empty" style={{ padding: 8 }}>Nenhuma sessão externa.</div>}
      {names?.map((n) =>
        safeSessionName(n) ? (
          <div key={n} className="rail-item">
            <span>{n}</span>
            <button className="btn" aria-label={`Anexar à sessão ${n}`} onClick={() => attach(n)}>Anexar</button>
          </div>
        ) : (
          <div key={n} className="rail-item">
            <span className="meta">nome não suportado</span>
          </div>
        ),
      )}
    </>
  );
}
