import { useEffect, useState } from "react";
import { errMsg } from "../../lib/ipc";
import { clampIdleMinutes, clampResidentFloor } from "../../lib/hibernate";
import { useStore } from "../../state/store";
import HibernateReasons from "../HibernateReasons";

export default function HibernateSection() {
  return (
    <>
      <h3>Hibernação</h3>
      <HibernateCard />
    </>
  );
}

/** Hibernação automática: experimental, desligada por padrão. Só painéis Claude ociosos. */
function HibernateCard() {
  const settings = useStore((s) => s.settings);
  const panes = useStore((s) => s.panes);
  const active = useStore((s) => s.active);
  const [why, setWhy] = useState<{ id: string; name: string } | null>(null);
  if (!settings) return null;
  const ah = settings.autoHibernate;
  const saveAh = (p: Partial<typeof ah>) => useStore.getState().saveSettings({ ...settings, autoHibernate: { ...ah, ...p } })
    .catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  const alive = ["iniciando", "trabalhando", "ocioso", "acordando"];
  const candidates = Object.values(panes).filter((p) => p.workspaceId === active && p.kind === "agent" && p.cli === "claude" && p.closedAt === null && alive.includes(p.status));
  return (
    <div className="card">
      <strong>Hibernação automática <span className="chip">experimental</span></strong>
      <p className="meta">Hiberna painéis Claude ociosos e acorda quando preciso. Pode haver atraso ao acordar.</p>
      <label className="row"><input type="checkbox" checked={ah.enabled} onChange={(e) => saveAh({ enabled: e.target.checked })} /> Hibernar painéis Claude ociosos (experimental)</label>
      <label className="field"><span>Minutos ociosos antes de hibernar (1 a 1440)</span>
        <NumberField min={1} max={1440} value={ah.idleMinutes} clamp={clampIdleMinutes} onSave={(v) => saveAh({ idleMinutes: v })} />
      </label>
      <label className="field"><span>Painéis que nunca hibernam (mínimo residente, 0 a 16)</span>
        <NumberField min={0} max={16} value={ah.residentFloor} clamp={clampResidentFloor} onSave={(v) => saveAh({ residentFloor: v })} />
      </label>
      <span className="meta">Painéis Claude abertos agora:</span>
      {candidates.length === 0 ? <div className="meta">Nenhum painel Claude aberto neste workspace.</div> : (
        <ul style={{ listStyle: "none", padding: 0, margin: "4px 0", display: "grid", gap: 4 }}>
          {candidates.map((p) => (
            <li key={p.id} className="row">
              <span>{p.name}</span>
              <button className="btn" onClick={() => setWhy({ id: p.id, name: p.name })}>Por que não hiberna</button>
            </li>
          ))}
        </ul>
      )}
      {why && <HibernateReasons paneId={why.id} name={why.name} onClose={() => setWhy(null)} />}
    </div>
  );
}

/** Número com rascunho local: digita à vontade e só limita e salva ao sair do campo (ou Enter). */
function NumberField({ min, max, value, clamp, onSave }: { min: number; max: number; value: number; clamp(v: number): number; onSave(v: number): void }) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const v = clamp(Number(text));
    setText(String(v));
    if (v !== value) onSave(v);
  };
  return (
    <input type="number" min={min} max={max} value={text} onChange={(e) => setText(e.target.value)} onBlur={commit}
      onKeyDown={(e) => { if (e.key === "Enter") commit(); }} />
  );
}
