import { useState } from "react";
import { api, errMsg, type PaneRow } from "../lib/ipc";
import { clearErrorRing } from "../lib/paneErrors";
import { defaultAgentCli, defaultAgentLabel } from "../lib/quickstart";
import { agentTargets, defaultTarget, readLastTarget, rememberLastTarget, sendToAgent } from "../lib/sendToAgent";
import { readLastCli } from "../lib/lastAgent";
import { useStore, wsOf, type SendTo } from "../state/store";
import Modal from "./Modal";
import { terms } from "./TerminalPane";

/** "Enviar ao agente": escolhe o painel de IA da missão, revisa o texto e cola (ou envia com Enter). */
export default function SendToAgentDialog() {
  const sendTo = useStore((s) => s.sendTo);
  if (!sendTo) return null;
  return <SendToForm key={`${sendTo.title}:${sendTo.text.length}`} req={sendTo} />;
}

function SendToForm({ req }: { req: SendTo }) {
  const panes = useStore((s) => s.panes);
  const detected = useStore((s) => s.detected);
  const clis = useStore((s) => s.providers?.clis ?? []);
  const all = Object.values(panes);
  const targets: PaneRow[] = agentTargets(all, req.workspaceId, req.missionId);
  // painéis esperando a sua resposta: aparecem, desabilitados, para você saber por que não são destino
  const waiting = all.filter((p) => p.kind === "agent" && p.workspaceId === req.workspaceId && p.missionId === req.missionId && p.status === "pergunta");
  const [text, setText] = useState(req.text);
  const [picked, setPicked] = useState<string | null>(() => defaultTarget(targets, readLastTarget()));
  const close = () => useStore.getState().patch({ sendTo: null });
  const openCli = defaultAgentCli(detected, readLastCli());
  const openLabel = defaultAgentLabel(detected, readLastCli(), clis).label;

  // depois de um envio que saiu: fecha, lembra o destino e zera o erro que originou o envio
  function done(paneId: string, enter: boolean) {
    rememberLastTarget(paneId);
    const name = useStore.getState().panes[paneId]?.name ?? "painel";
    useStore.getState().toast(enter ? `Enviado a ${name}` : `Colado em ${name} (sem Enter)`, "ok");
    if (req.fromPane) clearErrorRing(req.fromPane);
    close();
    const t = terms.get(paneId);
    if (t) t.focus();
    else useStore.getState().patch({ activePane: paneId });
  }

  async function send(enter: boolean) {
    if (!picked || !text.trim()) return;
    try {
      await sendToAgent(picked, text, enter);
      done(picked, enter);
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    }
  }

  // sem painel vivo: abre a IA padrão nesta missão e envia para ela
  async function openAndSend(enter: boolean) {
    const ws = wsOf(req.workspaceId);
    if (!ws || !openCli || !text.trim()) return;
    try {
      const row = await api.paneSpawn({ workspaceId: ws.id, cwd: ws.path, missionId: req.missionId, kind: "agent", cli: openCli, permission: "pend" });
      useStore.getState().upsertPane(row);
      await sendToAgent(row.id, text, enter);
      done(row.id, enter);
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    }
  }

  return (
    <Modal label="Enviar ao agente" onClose={close} style={{ width: "min(640px, 92vw)", display: "flex", flexDirection: "column", gap: 10 }}>
      <h2>{req.title}</h2>
      {targets.length === 0 && waiting.length === 0 ? (
        <p className="meta">Nenhum painel de IA vivo nesta missão. Abra um painel de IA primeiro.</p>
      ) : (
        <div role="radiogroup" aria-label="Painel de IA destino" className="row" style={{ flexWrap: "wrap", gap: 6 }}>
          {targets.map((p) => (
            <label key={p.id} className="chip" style={{ cursor: "pointer" }}>
              <input type="radio" name="destino" checked={picked === p.id} onChange={() => setPicked(p.id)} /> {p.name}
            </label>
          ))}
          {waiting.map((p) => (
            <label key={p.id} className="chip" title="Aguardando sua resposta: responda no painel primeiro" style={{ opacity: 0.6 }}>
              <input type="radio" name="destino" disabled /> {p.name} <span className="meta">(aguardando sua resposta)</span>
            </label>
          ))}
        </div>
      )}
      {targets.length === 0 && openCli && (
        <div className="row" style={{ gap: 8 }}>
          <button className="btn" onClick={() => void openAndSend(false)} disabled={!text.trim()}>Abrir {openLabel} nesta missão e colar</button>
          <button className="btn" onClick={() => void openAndSend(true)} disabled={!text.trim()}>Abrir {openLabel} nesta missão e enviar</button>
        </div>
      )}
      <textarea aria-label="Texto a enviar" rows={8} value={text} onChange={(e) => setText(e.target.value)} style={{ fontFamily: "var(--mono)", fontSize: 12 }} />
      <div className="actions">
        <button className="btn" onClick={close}>Cancelar</button>
        <button className="btn" disabled={!picked || !text.trim()} onClick={() => void send(false)}>Colar sem Enter</button>
        <button className="btn primary" disabled={!picked || !text.trim()} onClick={() => void send(true)}>Enviar</button>
      </div>
    </Modal>
  );
}
