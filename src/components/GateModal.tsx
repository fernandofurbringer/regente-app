import { useEffect, useState } from "react";
import { api, errCode, errMsg } from "../lib/ipc";
import { isAdjust, isTyping, optionLabel, parseOptions } from "../lib/gates";
import { goToPane } from "../lib/focus";
import { AgentMarkdown } from "../lib/markdown";
import { useStore } from "../state/store";
import Modal from "./Modal";

/** Gate mais antigo pendente. Aparece como faixa no topo (não modal); o diálogo completo abre por clique, Ctrl+Shift+G
 *  ou sozinho quando chega um gate e nenhum terminal nem campo está com o foco. Gates decididos em outro lugar saem sozinhos. */
export default function GateModal() {
  const gates = useStore((s) => s.gates);
  const open = useStore((s) => s.gateOpen);
  const panes = useStore((s) => s.panes);
  const missions = useStore((s) => s.missions);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const gate = gates[0];
  const toast = useStore((s) => s.toast);
  const removeGate = useStore((s) => s.removeGate);
  const patch = useStore((s) => s.patch);

  useEffect(() => { setNote(""); }, [gate?.id]);
  useEffect(() => {
    const st = useStore.getState();
    if (!st.gates[0]) {
      if (st.gateOpen) st.patch({ gateOpen: false });
      return;
    }
    if (!isTyping(document.activeElement)) st.patch({ gateOpen: true });
  }, [gate?.id]);

  if (!gate || !open) return null;
  const options = parseOptions(gate.optionsJson);
  const mission = Object.values(missions).flat().find((m) => m.id === gate.missionId)?.name;
  const pane = panes[gate.paneId]?.name ?? "Painel";
  const hasNote = note.trim().length > 0;
  const primary = options.find((o) => !isAdjust(o));

  async function answer(option: string) {
    if (busy || (isAdjust(option) && !hasNote)) return;
    setBusy(true);
    try {
      const reply = await api.gateAnswer(gate.id, option, hasNote ? note.trim() : undefined);
      removeGate(gate.id);
      if (reply === "pane_closed") toast("O painel já foi fechado: a decisão ficou registrada, mas não foi entregue.", "info");
    } catch (e) {
      // decidido por outro caminho (ex.: outra janela): sai da fila
      if (errCode(e) === "gate_closed") removeGate(gate.id);
      toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  return (
    // o gate bloqueia o maestro: clique fora não fecha; Esc e "Decidir depois" recolhem para a faixa
    <Modal label={`Gate: ${gate.title}`} backdropCloses={false} autoFocus={false} onClose={() => patch({ gateOpen: false })}>
      <h2>{gate.title}</h2>
      <p className="meta">{[mission, pane].filter(Boolean).join(" · ")}</p>
      {gates.length > 1 && <p className="meta" role="status">+{gates.length - 1} gate{gates.length > 2 ? "s" : ""} pendente{gates.length > 2 ? "s" : ""}</p>}
      <div className="gate-summary"><AgentMarkdown>{gate.summary}</AgentMarkdown></div>
      <label className="field">
        <span>Observação (opcional)</span>
        <textarea rows={3} value={note} onChange={(e) => setNote(e.target.value)} />
      </label>
      <div className="actions">
        <button className="btn" onClick={() => patch({ gateOpen: false })}>Decidir depois</button>
        {options.map((o) => (
          // a opção primária tem o foco ao abrir: Enter aprova só com o foco dentro do diálogo
          <button key={o} className={`btn ${isAdjust(o) ? "" : "primary"}`} autoFocus={o === primary}
            disabled={busy || (isAdjust(o) && !hasNote)} onClick={() => answer(o)}>
            {optionLabel(o)}
          </button>
        ))}
      </div>
      {options.some(isAdjust) && !hasNote && <p className="meta">Para pedir ajuste, escreva o que mudar na observação.</p>}
    </Modal>
  );
}

/** Faixa do gate no topo da área de trabalho: título, painel e as ações da opção primária. Não rouba o foco. */
export function GateStrip() {
  const gates = useStore((s) => s.gates);
  const open = useStore((s) => s.gateOpen);
  const panes = useStore((s) => s.panes);
  const [busy, setBusy] = useState(false);
  const gate = gates[0];
  if (!gate || open) return null;
  const options = parseOptions(gate.optionsJson);
  const primary = options.find((o) => !isAdjust(o));
  const pane = panes[gate.paneId]?.name ?? "Painel";
  const approve = async () => {
    if (!primary || busy) return;
    const st = useStore.getState();
    setBusy(true);
    try {
      const reply = await api.gateAnswer(gate.id, primary);
      st.removeGate(gate.id);
      if (reply === "pane_closed") st.toast("O painel já foi fechado: a decisão ficou registrada, mas não foi entregue.", "info");
    } catch (e) {
      if (errCode(e) === "gate_closed") st.removeGate(gate.id);
      st.toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="gate-strip" role="region" aria-label="Gate pendente">
      <button className="btn" title="Abrir a decisão completa (Ctrl+Shift+G)" onClick={() => useStore.getState().patch({ gateOpen: true })}>
        Gate: “{gate.title}”
      </button>
      <span className="meta">{pane}{gates.length > 1 ? ` · +${gates.length - 1} na fila` : ""}</span>
      <span className="spacer" />
      <button className="btn" onClick={() => goToPane(gate.paneId)}>Ver painel</button>
      {primary && <button className="btn primary" disabled={busy} onClick={approve}>{optionLabel(primary)}</button>}
      <button className="btn" onClick={() => useStore.getState().patch({ gateOpen: true })}>Ajustar…</button>
    </div>
  );
}
