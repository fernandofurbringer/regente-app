import { useState } from "react";
import Modal from "./Modal";

/** "Instalar" que mostra o comando exato (ao lado e no title) e pede confirmação antes de rodar. */
export default function InstallButton({ label, command, disabled, disabledReason, onConfirm }: {
  label: string; command: string; disabled?: boolean; disabledReason?: string; onConfirm(): void;
}) {
  const [ask, setAsk] = useState(false);
  return (
    <span className="row" style={{ alignItems: "center", gap: 8, minWidth: 0 }}>
      <button type="button" className="btn" disabled={disabled} title={disabled ? disabledReason : command} onClick={() => setAsk(true)}>Instalar</button>
      <code title={command} style={{ fontFamily: "var(--mono, monospace)", fontSize: 11, opacity: 0.8, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{command}</code>
      {ask && (
        <Modal label={`Confirmar instalação do ${label}`} onClose={() => setAsk(false)}>
          <h2>Instalar {label}</h2>
          <p>Vai rodar no terminal: <code style={{ fontFamily: "var(--mono, monospace)" }}>{command}</code></p>
          <div className="actions">
            <button type="button" className="btn" onClick={() => setAsk(false)}>Cancelar</button>
            <button type="button" className="btn primary" onClick={() => { setAsk(false); onConfirm(); }}>Rodar</button>
          </div>
        </Modal>
      )}
    </span>
  );
}
