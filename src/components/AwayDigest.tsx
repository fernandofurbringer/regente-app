import { useEffect, useState } from "react";
import { api, errMsg, type AwayData } from "../lib/ipc";
import { buildDigest, digestText, type DigestSection } from "../lib/digest";
import { useStore } from "../state/store";
import Modal from "./Modal";
import "./AwayDigest.css";

/** "Enquanto você estava fora": o que mudou nas missões do workspace desde `digestSince`. Sem LLM, salvo o botão. */
export default function AwayDigest() {
  const since = useStore((s) => s.digestSince);
  const wsId = useStore((s) => s.active);
  const panes = useStore((s) => s.panes);
  const statusSince = useStore((s) => s.statusSince);
  const gates = useStore((s) => s.gates);
  const toast = useStore((s) => s.toast);
  const [dados, setDados] = useState<AwayData | null>(null);
  const [ia, setIa] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fechar = () => useStore.getState().patch({ digestSince: null });

  useEffect(() => {
    if (since === null || !wsId) return;
    setDados(null);
    setIa(null);
    api.awayData(wsId, since).then(setDados).catch((e) => toast(errMsg(e), "erro"));
  }, [since, wsId, toast]);

  if (since === null || !wsId) return null;
  const secoes: DigestSection[] = dados
    ? buildDigest({
        since,
        missions: dados.missions,
        panes: Object.values(panes).filter((p) => p.workspaceId === wsId).map((p) => ({ id: p.id, missionId: p.missionId, name: p.name, status: p.status })),
        statusSince,
        gates: gates.map((g) => ({ id: g.id, missionId: g.missionId, title: g.title })),
        handoffs: dados.handoffs,
      })
    : [];

  async function resumirComIa(ws: string) {
    setBusy(true);
    try {
      setIa(await api.awayResumir(ws, digestText(secoes)));
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal label="Enquanto você estava fora" onClose={fechar} style={{ width: "min(640px, 94vw)" }}>
      <div className="away">
        <div className="meta">Desde {new Date(since).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })}</div>
        {!dados && <div className="meta">Lendo as missões…</div>}
        {dados && secoes.length === 0 && <div className="empty">Nada novo nas missões desde que você saiu.</div>}
        {secoes.map((s) => (
          <section key={s.missionId} className="away-section">
            <h3>{s.name}</h3>
            <ul>
              {s.finished.map((x, i) => <li key={`f${i}`}>{x}</li>)}
              {s.waiting.map((x, i) => <li key={`w${i}`}>{x}</li>)}
              {s.gates.map((x, i) => <li key={`g${i}`}>Portão aberto: {x}</li>)}
              {s.handoffs.map((x, i) => <li key={`h${i}`}>Entrega: {x}</li>)}
              {s.files && <li>Arquivos: {s.files}</li>}
              {s.verify && <li>Verificação: {s.verify}</li>}
              {s.cost && <li>Custo: {s.cost}</li>}
            </ul>
          </section>
        ))}
        {ia && <pre className="away-ia">{ia}</pre>}
        <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
          {secoes.length > 0 && <button className="btn" disabled={busy} onClick={() => resumirComIa(wsId)}>{busy ? "Resumindo…" : "Resumir com IA"}</button>}
          <button className="btn primary" onClick={fechar}>Fechar</button>
        </div>
      </div>
    </Modal>
  );
}
