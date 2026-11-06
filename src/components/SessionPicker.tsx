import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errMsg, type SessionInfo } from "../lib/ipc";
import { filterSessions, fmtSessionSize, latestSession } from "../lib/sessions";
import { isRemote } from "../lib/remote";
import { useStore } from "../state/store";
import Modal from "./Modal";

const CLI_LABEL: Record<string, string> = { claude: "Claude", codex: "Codex" };

/** Lista as sessões anteriores da pasta do workspace e reabre uma delas num painel novo. */
function SessionList({ wsId, onResumed }: { wsId: string; onResumed?: () => void }) {
  const [list, setList] = useState<SessionInfo[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [cur, setCur] = useState(0);
  const shown = list ? filterSessions(list, query) : [];
  const box = useRef<HTMLDivElement>(null);
  // a marcada acompanha ↑/↓ mesmo quando sai da área visível
  useEffect(() => {
    box.current?.querySelector<HTMLElement>('[aria-current="true"]')?.scrollIntoView?.({ block: "nearest" });
  }, [cur]);

  useEffect(() => {
    let alive = true;
    api.sessionsList(wsId)
      .then((l) => alive && setList(l))
      .catch((e) => {
        if (alive) setList([]);
        useStore.getState().toast(errMsg(e), "erro");
      });
    return () => {
      alive = false;
    };
  }, [wsId]);

  async function open(s: SessionInfo) {
    setBusy(s.id);
    try {
      await api.sessionResume(wsId, s.cli, s.id);
      onResumed?.();
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    } finally {
      setBusy(null);
    }
  }

  // ↑/↓ mudam a sessão marcada (da busca ou da lista); Enter na busca retoma a marcada
  function onKey(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key === "ArrowDown") setCur((c) => Math.min(c + 1, shown.length - 1));
    else if (e.key === "ArrowUp") setCur((c) => Math.max(c - 1, 0));
    else if (e.key === "Enter" && e.target instanceof HTMLInputElement && shown[cur]) void open(shown[cur]);
    else return;
    e.preventDefault();
  }

  return (
    <div ref={box} onKeyDown={onKey}>
      {list === null && <span className="muted">Procurando sessões desta pasta…</span>}
      {list?.length === 0 && <span className="muted">Nenhuma sessão anterior nesta pasta.</span>}
      {!!list?.length && (
        <input type="search" aria-label="Buscar sessão" placeholder="Buscar pela prévia ou pela IA" value={query}
          onChange={(e) => { setQuery(e.target.value); setCur(0); }} />
      )}
      {list && list.length > 0 && shown.length === 0 && <span className="muted">Nenhuma sessão com “{query.trim()}”.</span>}
      {shown.map((s, i) => (
        <div key={`${s.cli}:${s.id}`} className="row" tabIndex={0} aria-current={i === cur ? "true" : undefined}
          style={{ justifyContent: "space-between", alignItems: "center", background: i === cur ? "var(--bg3)" : undefined }}>
          <span>
            <strong>{CLI_LABEL[s.cli] ?? s.cli}</strong>{" · "}
            {new Date(s.modifiedMs).toLocaleString("pt-BR")}{" · "}
            {fmtSessionSize(s.size)}{" · "}
            {s.prompt ?? <span className="muted">sem prévia</span>}
          </span>
          <button className="btn" disabled={busy === s.id} onClick={() => open(s)}>Retomar</button>
        </div>
      ))}
    </div>
  );
}

/** Workspace vazio: as sessões anteriores aparecem na própria área de painéis. */
export default function SessionPicker({ wsId }: { wsId: string }) {
  const ws = useStore((s) => s.workspaces.find((w) => w.id === wsId));
  if (!ws || isRemote(ws.path)) return null;
  return (
    <div className="session-picker" style={{ marginTop: 16, maxWidth: 720 }}>
      <h4>Retomar sessão anterior</h4>
      <SessionList wsId={wsId} />
    </div>
  );
}

/** Mesma lista num diálogo, para retomar mesmo com painéis abertos (paleta: "Retomar sessão anterior…"). */
export function SessionPickerDialog({ wsId }: { wsId: string }) {
  const ws = useStore((s) => s.workspaces.find((w) => w.id === wsId));
  const close = () => useStore.getState().patch({ sessionPicker: null });
  if (!ws || isRemote(ws.path)) return null;
  return (
    <Modal label="Retomar sessão anterior" onClose={close} style={{ maxWidth: 720 }}>
      <h4>Retomar sessão anterior</h4>
      <SessionList wsId={wsId} onResumed={close} />
    </Modal>
  );
}

/** Retoma a conversa mais recente da pasta do workspace (um passo, sem abrir a lista). */
export async function continueLastSession(wsId: string): Promise<void> {
  const st = useStore.getState();
  const ws = st.workspaces.find((w) => w.id === wsId);
  if (!ws) return st.toast("Abra um workspace primeiro.", "erro");
  if (isRemote(ws.path)) return st.toast("Sessões anteriores não estão disponíveis em workspace remoto.", "erro");
  try {
    const last = latestSession(await api.sessionsList(ws.id));
    if (!last) return st.toast("Nenhuma sessão anterior nesta pasta.", "info");
    await api.sessionResume(ws.id, last.cli, last.id);
  } catch (e) {
    st.toast(errMsg(e), "erro");
  }
}
