import { useState } from "react";
import { cardIds, restoreCards, restoreTotals } from "../lib/restoreCards";
import { api, errMsg, type PaneRow } from "../lib/ipc";
import { isRemote } from "../lib/remote";
import { useStore } from "../state/store";
import ExternalSessions from "./ExternalSessions";

/** Aviso único, no topo, com os painéis pendentes de todos os workspaces. */
export function RestoreBanner() {
  const restore = useStore((s) => s.restore);
  const detail = useStore((s) => s.restoreDetail);
  const wsIds = Object.keys(restore).filter((id) => restore[id]?.length);
  if (detail || !wsIds.length) return null;
  const { panes, workspaces } = restoreTotals(restore);
  return (
    <div className="restore-banner" role="status">
      <span>Restaurar {panes} {panes === 1 ? "painel" : "painéis"} em {workspaces} {workspaces === 1 ? "workspace" : "workspaces"}.</span>
      <span className="spacer" />
      <button className="btn primary" onClick={() => void useStore.getState().decideRestore(true)}>Restaurar tudo</button>
      <button className="btn" onClick={() => useStore.getState().patch({ restoreDetail: true, active: wsIds[0] })}>Escolher…</button>
      <button className="btn" onClick={() => void useStore.getState().decideRestore(false)}>Agora não</button>
      <button className="btn" onClick={() => void useStore.getState().discardRestore()}>Descartar</button>
    </div>
  );
}

/** Cartões de restauração: um por missão, com papel, conta, modelo e sessão de cada painel. Aparecem com "Escolher…" no banner. */
export default function RestorePrompt({ wsId }: { wsId: string }) {
  const rows = useStore((s) => s.restore[wsId]);
  const detail = useStore((s) => s.restoreDetail);
  const missions = useStore((s) => s.missions[wsId]);
  const ws = useStore((s) => s.workspaces.find((w) => w.id === wsId));
  const [picking, setPicking] = useState<string | null>(null); // cartão em "Escolher painéis"
  const [chosen, setChosen] = useState<Record<string, boolean>>({});
  if (!rows?.length || !detail) return null;
  const cards = restoreCards(rows, missions);

  // tira do aviso os painéis já decididos; sem nenhum, o aviso some
  const settle = (ids: string[]) => {
    const st = useStore.getState();
    const left = (st.restore[wsId] ?? []).filter((r) => !ids.includes(r.id));
    const restore = { ...st.restore };
    if (left.length) restore[wsId] = left;
    else delete restore[wsId];
    st.patch({ restore, restoreDetail: Object.keys(restore).length > 0 && st.restoreDetail });
  };
  // ids null = tudo que ainda sobra no aviso
  async function apply(ids: string[] | null, accept: boolean) {
    const st = useStore.getState();
    const target = ids ?? (st.restore[wsId] ?? []).map((r) => r.id);
    try {
      for (const r of await api.restoreApply(wsId, accept, ids ?? undefined)) st.upsertPane(r);
      settle(target);
      setPicking(null);
      setChosen({});
    } catch (e) {
      st.toast(errMsg(e), "erro");
    }
  }

  return (
    <div className="restore-panel" role="alertdialog" aria-label="Restaurar sessão">
      <div className="row"><b>Restaurar a sessão anterior?</b><span className="spacer" /></div>
      {cards.map((c) => {
        const picked = picking === c.key ? cardIds(c, chosen) : [];
        return (
          <div className="restore-card" key={c.key || "sem-missao"}>
            <div className="row"><b>{c.title}</b><span className="meta">{c.panes.length} {c.panes.length === 1 ? "painel" : "painéis"}</span></div>
            <ul>
              {c.panes.map((p: PaneRow) => (
                <li key={p.id}>
                  {picking === c.key && (
                    <input type="checkbox" aria-label={`Restaurar ${p.name}`} checked={chosen[p.id] ?? true}
                      onChange={(e) => setChosen((s) => ({ ...s, [p.id]: e.target.checked }))} />
                  )}
                  <span>{p.name}</span>
                  <span className="meta">
                    {p.role ?? "painel"} · {p.account ?? "conta padrão"} · {p.model ?? "modelo padrão"} · {p.sessionId ? `sessão ${p.sessionId.slice(0, 8)}` : "sem sessão"}
                  </span>
                </li>
              ))}
            </ul>
            <div className="row">
              {picking === c.key ? (
                <>
                  <button className="btn primary" disabled={picked.length === 0} onClick={() => apply(picked, true)}>Restaurar selecionados ({picked.length})</button>
                  <button className="btn" onClick={() => setPicking(null)}>Voltar</button>
                </>
              ) : (
                <>
                  <button className="btn primary" onClick={() => apply(cardIds(c, null), true)}>Restaurar missão inteira</button>
                  <button className="btn" onClick={() => { setChosen({}); setPicking(c.key); }}>Escolher painéis</button>
                </>
              )}
            </div>
          </div>
        );
      })}
      {ws && isRemote(ws.path) && (
        <div className="restore-card"><ExternalSessions wsId={wsId} /></div>
      )}
      <div className="row">
        <span className="spacer" />
        <button className="btn" onClick={() => void useStore.getState().decideRestore(false)}>Agora não</button>
        <button className="btn" onClick={() => void useStore.getState().discardRestore()}>Descartar</button>
        <button className="btn primary" onClick={() => apply(null, true)}>Restaurar tudo</button>
      </div>
    </div>
  );
}
