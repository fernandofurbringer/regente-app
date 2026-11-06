import { useState } from "react";
import { errMsg } from "../../lib/ipc";
import { useStore } from "../../state/store";

/** Minutos sem foco para o digest "Enquanto você estava fora" aparecer ao voltar (0 = desligado). */
export default function AwaySection() {
  const settings = useStore((s) => s.settings);
  const [valor, setValor] = useState<string | null>(null);
  if (!settings) return null;
  const atual = settings.ausencia?.minutos ?? 10;
  const salvar = (texto: string) => {
    const n = Math.min(480, Math.max(0, Math.round(Number(texto) || 0)));
    setValor(null);
    if (n === atual) return;
    useStore.getState().saveSettings({ ...settings, ausencia: { minutos: n } }).catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  };
  return (
    <>
      <h3>Enquanto você estava fora</h3>
      <div className="card">
        <p className="meta">Ao voltar para o Regente depois deste tempo sem foco, um resumo mostra o que os agentes fizeram, o que espera resposta, os portões abertos e as entregas.</p>
        <label className="field"><span>Minutos sem foco antes do resumo (0 a 480; 0 desliga)</span>
          <input type="number" min={0} max={480} value={valor ?? atual} onChange={(e) => setValor(e.target.value)} onBlur={(e) => salvar(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") salvar((e.target as HTMLInputElement).value); }} />
        </label>
      </div>
    </>
  );
}
