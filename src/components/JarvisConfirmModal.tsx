import { api, errMsg, type JarvisConfirm } from "../lib/ipc";
import { useStore } from "../state/store";
import Modal from "./Modal";

/** Confirmação de uma ação pedida pelo celular ou pelo app (gate, escrita num painel, missão). Uma por vez, na ordem; Cancelar não faz nada. */
export default function JarvisConfirmModal() {
  const queue = useStore((s) => s.jarvisConfirms);
  const c = queue[0];
  if (!c) return null;
  // who asked: the phone (its device name is text, rendered as text), or the app itself
  const who = c.source === "celular" ? `O celular «${c.device || "Celular"}»` : "Um pedido do Regente";
  const toast = (text: string, kind: "erro" | "ok" | "info" = "erro") => useStore.getState().toast(text, kind);
  const next = () => useStore.getState().patch({ jarvisConfirms: useStore.getState().jarvisConfirms.slice(1) });

  async function confirm(item: JarvisConfirm) {
    next();
    try {
      if (item.kind === "gate") {
        const reply = await api.gateAnswer(item.gateId, item.decision, item.note ?? undefined);
        if (reply === "pane_closed") toast("O painel já foi fechado: a decisão ficou registrada, mas não foi entregue.", "info");
      } else if (item.kind === "write") {
        await api.jarvisWriteConfirmed(item.paneId, item.text, item.enter);
      } else {
        const input = item.input as unknown as Parameters<typeof api.missionCreate>[0];
        await api.missionCreate(input);
        useStore.getState().loadMissions(String(input.workspaceId));
        toast("Missão aberta.", "ok");
      }
    } catch (e) {
      toast(errMsg(e));
    }
  }

  return (
    <Modal label="Confirmar ação" onClose={next} backdropCloses={false}>
      <h2>Confirmar</h2>
      {c.kind === "gate" && (
        <>
          <p>{who} quer responder o gate «{c.title}» com «{c.decision}». Confirmar?</p>
          {c.note && <p className="meta">Nota: {c.note}</p>}
        </>
      )}
      {c.kind === "write" && (
        <>
          <p>{who} quer digitar no painel «{c.paneName}»:</p>
          <pre className="jarvis-text">{c.text}</pre>
          <p className="meta">{c.enter ? "Com Enter no fim (envia)." : "Sem Enter no fim (não envia)."}</p>
        </>
      )}
      {c.kind === "mission" && (
        <>
          <p>{who} quer abrir a missão «{c.name}» (modo {c.mode}) com o brief:</p>
          <pre className="jarvis-text">{c.briefExcerpt}</pre>
        </>
      )}
      <p className="meta">Workspace: {c.workspace || "—"}{queue.length > 1 ? ` · mais ${queue.length - 1} aguardando` : ""}</p>
      <div className="actions">
        <button className="btn" onClick={next}>Cancelar</button>
        <button className="btn primary" onClick={() => confirm(c)}>Confirmar</button>
      </div>
    </Modal>
  );
}
