import { useEffect, useState } from "react";
import { moveItem } from "../../lib/grid";
import { api, errMsg, type PriceEntry, type Prices } from "../../lib/ipc";
import { useStore } from "../../state/store";

const COLS: [keyof Omit<PriceEntry, "match">, string][] = [["input", "Entrada"], ["output", "Saída"], ["cacheRead", "Cache lido"], ["cacheWrite", "Cache escrito"]];

export default function PricesSection() {
  const [p, setP] = useState<Prices | null>(null);
  const toast = useStore((s) => s.toast);
  useEffect(() => { api.pricesGet().then(setP).catch((e) => toast(errMsg(e), "erro")); }, [toast]);
  if (!p) return null;
  const setRow = (i: number, patch: Partial<PriceEntry>) => setP({ models: p.models.map((m, j) => (j === i ? { ...m, ...patch } : m)) });
  const move = (i: number, to: number) => setP({ models: moveItem(p.models, i, to) });
  async function save() {
    // linha sem trecho casaria com qualquer modelo
    if (p!.models.some((m) => !m.match.trim())) return toast("Preencha o trecho do modelo em todas as linhas (ou remova a linha vazia).", "erro");
    try {
      await api.pricesSave({ models: p!.models.map((m) => ({ ...m, match: m.match.trim() })) });
      toast("Preços salvos", "ok");
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  return (
    <>
      <h3>Preços (US$ por 1M tokens)</h3>
      <p style={{ color: "var(--fg3)" }}>Vale a primeira linha cujo trecho aparece no nome do modelo; deixe as mais específicas em cima. Os custos mostrados com "≈" são estimativas.</p>
      <table className="tbl">
        <thead><tr><th>Trecho do modelo</th>{COLS.map(([, l]) => <th key={l}>{l}</th>)}<th /></tr></thead>
        <tbody>
          {p.models.map((m, i) => (
            <tr key={i}>
              <td><input aria-label="Trecho do modelo" value={m.match} onChange={(e) => setRow(i, { match: e.target.value })} /></td>
              {COLS.map(([k, l]) => (
                <td key={k}><input type="number" step="0.01" min={0} aria-label={l} value={m[k]} onChange={(e) => setRow(i, { [k]: Number(e.target.value) || 0 })} /></td>
              ))}
              <td style={{ whiteSpace: "nowrap" }}>
                <button className="icon-btn" aria-label="Subir linha" disabled={i === 0} onClick={() => move(i, i - 1)}>↑</button>
                <button className="icon-btn" aria-label="Descer linha" disabled={i === p.models.length - 1} onClick={() => move(i, i + 1)}>↓</button>
                <button className="icon-btn" aria-label="Remover linha" onClick={() => setP({ models: p.models.filter((_, j) => j !== i) })}>✕</button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="actions">
        <button className="btn" onClick={() => setP({ models: [...p.models, { match: "", input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }] })}>Adicionar linha</button>
        <button className="btn primary" onClick={save}>Salvar</button>
      </div>
    </>
  );
}
