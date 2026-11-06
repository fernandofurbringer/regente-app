import { useEffect, useState } from "react";
import { api, type WhatsNew } from "../lib/ipc";
import Modal from "./Modal";

/** Novidades da versão recém-instalada: aparece uma vez, no primeiro boot dessa versão. Texto puro, nunca HTML. */
export default function WhatsNewModal() {
  const [n, setN] = useState<WhatsNew | null>(null);
  useEffect(() => {
    api.whatsNewGet().then(setN).catch(() => setN(null));
  }, []);
  if (!n) return null;
  const close = () => {
    setN(null);
    void api.whatsNewSeen().catch(() => undefined);
  };
  return (
    <Modal label="Novidades" onClose={close}>
      <h2>Novidades na versão {n.version}</h2>
      <pre style={{ whiteSpace: "pre-wrap" }}>{n.notes || "Atualização instalada."}</pre>
      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button className="btn" onClick={close}>Ok</button>
      </div>
    </Modal>
  );
}
