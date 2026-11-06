import { useMemo, useState } from "react";
import { api, errMsg, type PaneHit } from "../lib/ipc";
import { useStore } from "../state/store";
import { isInlineSubagent } from "../lib/activity";
import { searchSummary, SOURCE_LABEL } from "../lib/paneSearch";
import { openPaneHistory } from "../lib/paneHistory";
import Modal from "./Modal";
import { goToPane } from "./CommandPalette";
import { terms } from "./TerminalPane";

/** Busca em todos os painéis abertos (Ctrl+Shift+F). Resultados por painel; clicar leva ao painel e realça o termo no terminal. */
export default function PaneSearch() {
  const open = useStore((s) => s.paneSearch);
  const panes = useStore((s) => s.panes);
  const subagentPanes = useStore((s) => s.settings?.subagentPanes ?? false);
  const [query, setQuery] = useState("");
  const [searched, setSearched] = useState<string | null>(null);
  const [hits, setHits] = useState<PaneHit[]>([]);
  const [truncated, setTruncated] = useState(false);
  const [diskLimited, setDiskLimited] = useState(false);
  const [busy, setBusy] = useState(false);

  // um painel pode ter vários trechos: agrupa na ordem em que a busca os devolveu
  const groups = useMemo(() => {
    const byPane = new Map<string, PaneHit[]>();
    for (const h of hits) byPane.set(h.paneId, [...(byPane.get(h.paneId) ?? []), h]);
    return [...byPane.entries()];
  }, [hits]);

  if (!open) return null;
  const close = () => useStore.getState().patch({ paneSearch: false });

  async function run() {
    const q = query.trim();
    if (!q || busy) return;
    setBusy(true);
    try {
      const r = await api.searchPanes(q);
      setHits(r.hits);
      setTruncated(r.truncated);
      setDiskLimited(r.diskLimited);
      setSearched(q);
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  function open1(h: PaneHit) {
    const term = searched ?? query.trim();
    if (h.source === "historico") {
      // o texto antigo não está mais no xterm: abre o histórico completo do painel no editor
      close();
      goToPane(h.paneId);
      void openPaneHistory(h.paneId, h.workspaceId).catch((e) => useStore.getState().toast(errMsg(e), "erro"));
      return;
    }
    close();
    goToPane(h.paneId);
    // o terminal recebe o foco em goToPane (50 ms); o realce vem depois dele
    setTimeout(() => terms.get(h.paneId)?.find(term), 120);
  }

  return (
    <Modal label="Buscar em todos os painéis" onClose={close} style={{ width: "min(680px, 94vw)", display: "flex", flexDirection: "column", gap: 8 }}>
      <h2>Buscar em todos os painéis</h2>
      <div className="row" style={{ gap: 6 }}>
        <input autoFocus placeholder="texto a procurar" aria-label="Texto a procurar nos painéis" value={query} style={{ flex: 1 }}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void run(); } }} />
        <button className="btn" disabled={!query.trim() || busy} onClick={() => void run()}>{busy ? "Buscando…" : "Buscar"}</button>
      </div>
      <div className="meta" role="status">
        {searched === null && "Busca nos buffers dos painéis, no histórico em disco e nas conversas do Claude, sem diferenciar maiúsculas."}
        {searched !== null && searchSummary({ hits, truncated, diskLimited }, searched)}
      </div>
      <div style={{ maxHeight: "55vh", overflowY: "auto", display: "flex", flexDirection: "column", gap: 10 }}>
        {groups.map(([paneId, list]) => {
          const pane = panes[paneId];
          const index = pane ? Object.values(panes).filter((p) => p.workspaceId === pane.workspaceId && !isInlineSubagent(p, subagentPanes)).sort((a, b) => a.ord - b.ord).findIndex((p) => p.id === paneId) + 1 : 0;
          return (
            <section key={paneId} aria-label={`Resultados em ${list[0].paneName}`}>
              <div className="meta"><strong>#{index} {list[0].paneName}</strong> · {list.length}</div>
              {list.map((h, i) => (
                <button key={`${h.source}-${i}`} className="menu-item" style={{ display: "flex", gap: 8, width: "100%", textAlign: "left" }} onClick={() => open1(h)}>
                  <span className="badge">{SOURCE_LABEL[h.source]}</span>
                  <span style={{ fontFamily: "var(--mono)", fontSize: 12, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{h.excerpt}</span>
                </button>
              ))}
            </section>
          );
        })}
      </div>
    </Modal>
  );
}
