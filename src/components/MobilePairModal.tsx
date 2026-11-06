import { useEffect, useState } from "react";
import { api, errMsg, on } from "../lib/ipc";
import { useStore } from "../state/store";
import Modal from "./Modal";

interface PairRequest { deviceId: string; deviceName: string }

/** Um celular pareado por código só funciona depois de aprovado aqui. Uma pergunta por vez, na ordem. */
export default function MobilePairModal() {
  const [queue, setQueue] = useState<PairRequest[]>([]);
  useEffect(() => {
    const off = on<PairRequest>("mobile-pair-request", (r) => setQueue((q) => [...q.filter((x) => x.deviceId !== r.deviceId), r]));
    return () => {
      void off.then((u) => u());
    };
  }, []);

  const req = queue[0];
  if (!req) return null;

  async function decide(approve: boolean) {
    setQueue((q) => q.slice(1));
    try {
      await api.mobileApprove(req.deviceId, approve);
      useStore.getState().toast(approve ? "Celular aprovado." : "Celular recusado.", approve ? "ok" : "info");
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    }
  }

  return (
    <Modal label="Aprovar celular" onClose={() => decide(false)} backdropCloses={false}>
      <h2>Aprovar o celular «{req.deviceName}»?</h2>
      <p>Ele entrou com o código mostrado neste computador. Depois de aprovado, poderá ver a saída de todos os painéis e enviar texto aos painéis de IA.</p>
      <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
        <button className="btn" onClick={() => decide(false)}>Recusar</button>
        <button className="btn" onClick={() => decide(true)}>Aprovar</button>
      </div>
    </Modal>
  );
}
