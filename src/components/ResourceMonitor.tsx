import { useEffect, useState } from "react";
import { api, errMsg, type ResPane, type ResSnapshot } from "../lib/ipc";
import { fmtMem, isIdleClaude } from "../lib/resources";
import { useStore } from "../state/store";
import Modal from "./Modal";
import { terms } from "./TerminalPane";

/** Monitor de recursos: RAM e CPU de cada painel (árvore inteira), da interface e do Regente. Atualiza a cada 2 s. */
export default function ResourceMonitor() {
  const open = useStore((s) => s.resourcesOpen);
  const panes = useStore((s) => s.panes);
  const [snap, setSnap] = useState<ResSnapshot | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [expandedIds, setOpen1] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    const load = () => api.resourcesSnapshot().then((s) => { if (alive) { setSnap(s); setErr(null); } }).catch((e) => { if (alive) setErr(errMsg(e)); });
    load();
    const t = setInterval(load, 2000);
    return () => { alive = false; clearInterval(t); };
  }, [open]);

  if (!open) return null;
  /** CPU da primeira leitura ainda não existe: "—" em vez de 0 %. */
  const cpu = (v: number) => (snap?.cpuReady ? `${v.toFixed(0)} %` : "—");
  const close = () => useStore.getState().patch({ resourcesOpen: false });
  const toggle = (id: string) => setOpen1((prev) => { const n = new Set(prev); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  const hibernate = (id: string) => api.paneHibernate(id, terms.get(id)?.screen() ?? "");

  async function hibernateIdle() {
    if (!snap) return;
    setBusy(true);
    let n = 0;
    let freed = 0;
    let firstWhy: string | null = null;
    try {
      // só os ociosos, e só os que o servidor diz que podem hibernar agora (sem motivo para bloquear)
      for (const p of snap.panes) {
        const row = panes[p.paneId];
        if (!row || !isIdleClaude(row)) continue;
        const why = await api.paneHibernateReasons(p.paneId, true).catch((e) => [errMsg(e)]);
        if (why.length) {
          firstWhy ??= `${p.name}: ${why[0]}`;
          continue;
        }
        await hibernate(p.paneId);
        n++;
        freed += p.memMb;
      }
      const st = useStore.getState();
      if (n === 0) st.toast(firstWhy ? `Nenhum painel ocioso podia ser hibernado (${firstWhy}).` : "Nenhum painel Claude ocioso agora.", "info");
      else st.toast(`${n} ${n === 1 ? "painel hibernado" : "painéis hibernados"}, ~${fmtMem(freed)} liberados`, "ok");
    } catch (e) {
      useStore.getState().toastErr(e);
    } finally {
      setBusy(false);
    }
  }

  async function hibernateOne(p: ResPane) {
    try {
      const why = await api.paneHibernateReasons(p.paneId, true);
      if (why.length) {
        useStore.getState().toast(`${p.name} não hiberna: ${why[0]}`, "info");
        return;
      }
      await hibernate(p.paneId);
      useStore.getState().toast(`${p.name} hibernado, ~${fmtMem(p.memMb)} liberados`, "ok");
    } catch (e) {
      useStore.getState().toastErr(e);
    }
  }

  const rows = [...(snap?.panes ?? [])].sort((a, b) => b.memMb - a.memMb);
  const idle = rows.filter((p) => panes[p.paneId] && isIdleClaude(panes[p.paneId]));

  return (
    <Modal label="Monitor de recursos" onClose={close} style={{ width: "min(780px, 94vw)", maxHeight: "86vh", overflow: "auto" }}>
      <h2>Monitor de recursos</h2>
      {err && <div className="meta" role="alert">{err}</div>}
      {!snap ? <div className="empty">Medindo…</div> : (
        <>
          <p className="meta">
            Total do Regente: <strong>{fmtMem(snap.total.memMb)}</strong> · {snap.total.procs} processos · CPU {cpu(snap.total.cpuPct)}
          </p>
          <table className="res-table">
            <thead>
              <tr><th>Item</th><th>Status</th><th className="num">Memória (conjunto de trabalho)</th><th className="num">CPU</th><th className="num">Processos</th><th>Ações</th></tr>
            </thead>
            <tbody>
              <tr><td>Regente</td><td>—</td><td className="num">{fmtMem(snap.app.memMb)}</td><td className="num">{cpu(snap.app.cpuPct)}</td><td className="num">{snap.app.procs}</td><td /></tr>
              <tr><td>Interface (WebView2)</td><td>—</td><td className="num">{fmtMem(snap.ui.memMb)}</td><td className="num">{cpu(snap.ui.cpuPct)}</td><td className="num">{snap.ui.procs}</td><td /></tr>
              {rows.map((p) => {
                const expanded = expandedIds.has(p.paneId);
                return (
                  <tr key={p.paneId}>
                    <td>
                      <button className="btn" aria-expanded={expanded} disabled={p.children.length === 0} onClick={() => toggle(p.paneId)}>
                        {p.children.length ? (expanded ? "▾ " : "▸ ") : ""}{p.name}
                      </button>
                      {expanded && (
                        <ul className="res-children">
                          {p.children.map((c) => <li key={c.pid}>{c.name} (pid {c.pid}) · {fmtMem(c.memMb)} · CPU {cpu(c.cpuPct)}</li>)}
                        </ul>
                      )}
                    </td>
                    <td>{p.status}</td>
                    <td className="num">{fmtMem(p.memMb)}</td>
                    <td className="num">{cpu(p.cpuPct)}</td>
                    <td className="num">{p.procs}</td>
                    <td>{panes[p.paneId] && isIdleClaude(panes[p.paneId]) && <button className="btn" onClick={() => hibernateOne(p)}>Hibernar</button>}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </>
      )}
      <div className="actions">
        <button className="btn" disabled={busy || idle.length === 0} onClick={hibernateIdle}
          title={idle.length ? undefined : "Nenhum painel Claude ocioso agora"}>Hibernar ociosos</button>
        <button className="btn" onClick={close}>Fechar</button>
      </div>
    </Modal>
  );
}
