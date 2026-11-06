import { useEffect, useState } from "react";
import { api, errMsg } from "../lib/ipc";
import { useStore } from "../state/store";
import Modal from "./Modal";

/** Por que um painel não hiberna (lista do backend). Usado no ⋯ do painel e nas configurações. */
export default function HibernateReasons({ paneId, name, onClose }: { paneId: string; name: string; onClose(): void }) {
  const [reasons, setReasons] = useState<string[] | null>(null);
  useEffect(() => {
    let alive = true;
    api.paneHibernateReasons(paneId)
      .then((r) => { if (alive) setReasons(r); })
      .catch((e) => { if (alive) { setReasons([]); useStore.getState().toast(errMsg(e), "erro"); } });
    return () => { alive = false; };
  }, [paneId]);
  return (
    <Modal label="Por que não hiberna" onClose={onClose}>
      <h2>Por que não hiberna</h2>
      <p className="meta">{name}</p>
      <p className="meta">Ao acordar, o cache estará frio: o primeiro turno relê o contexto.</p>
      {reasons === null ? <p>Carregando…</p> : reasons.length ? (
        <ul>{reasons.map((r) => <li key={r}>{r}</li>)}</ul>
      ) : (
        <p>Nenhum motivo: este painel pode hibernar.</p>
      )}
      <div className="actions"><button className="btn" onClick={onClose}>Fechar</button></div>
    </Modal>
  );
}
