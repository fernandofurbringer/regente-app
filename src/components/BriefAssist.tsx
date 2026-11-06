import { useState } from "react";
import { api, errMsg } from "../lib/ipc";
import { BRIEF_CHIPS, insertChip } from "../lib/briefing";
import { useStore } from "../state/store";

/** Chips de seções sob o briefing e "Me sugere". A sugestão só entra no briefing quando o usuário aceita. */
export default function BriefAssist({ brief, onChange }: { brief: string; onChange(text: string): void }) {
  const [busy, setBusy] = useState(false);
  const [suggestion, setSuggestion] = useState<string | null>(null);

  async function suggest() {
    setBusy(true);
    try {
      const r = await api.briefSuggest(brief);
      setSuggestion(r.text);
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="brief-assist" style={{ marginBottom: 10 }}>
      <div className="row" role="group" aria-label="Acrescentar seção ao briefing">
        {BRIEF_CHIPS.map((c) => (
          <button key={c.id} type="button" className="chip" onClick={() => onChange(insertChip(brief, c.line))}>{c.label}</button>
        ))}
        <button type="button" className="btn" disabled={busy || !brief.trim()} onClick={suggest}>{busy ? "Pensando…" : "Me sugere"}</button>
      </div>
      {suggestion && (
        <div className="card" role="region" aria-label="Sugestão de briefing">
          <span className="muted">Revise antes de usar. Nada foi gravado.</span>
          <pre style={{ whiteSpace: "pre-wrap" }}>{suggestion}</pre>
          <div className="row">
            <button type="button" className="btn primary" onClick={() => { onChange(suggestion); setSuggestion(null); }}>Usar esta sugestão</button>
            <button type="button" className="btn" onClick={() => setSuggestion(null)}>Descartar</button>
          </div>
        </div>
      )}
    </div>
  );
}
