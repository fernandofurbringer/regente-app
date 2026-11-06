import { useEffect, useState } from "react";
import { api, errMsg, type PromptDef } from "../../lib/ipc";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

/** Prompts salvos: nome, texto com variáveis e a opção de enviar Enter. Salvos em data/prompts.json. */
export default function PromptsSection() {
  const [rows, setRows] = useState<PromptDef[] | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    api.promptsList().then(setRows).catch((e) => { toast(e); setRows([]); });
  }, []);

  const edit = (i: number, patch: Partial<PromptDef>) => setRows((r) => r?.map((x, j) => (j === i ? { ...x, ...patch } : x)) ?? r);

  async function save() {
    if (!rows || busy) return;
    setBusy(true);
    try {
      setRows(await api.promptsSave(rows));
      useStore.getState().toast("Prompts salvos", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h3>Prompts</h3>
      <p className="meta">
        Textos prontos para colar no painel (paleta: "prompt: nome", ou menu ⋯ do painel). Variáveis, expandidas na hora de colar:
        {" "}<span className="kbd">{"{{clipboard}}"}</span> <span className="kbd">{"{{arquivo}}"}</span> <span className="kbd">{"{{selecao}}"}</span>{" "}
        <span className="kbd">{"{{workspace}}"}</span> <span className="kbd">{"{{branch}}"}</span>.
        Sem Enter, o texto fica na linha de entrada para você revisar.
      </p>
      {rows === null && <div className="meta">Carregando…</div>}
      {rows?.map((p, i) => (
        <fieldset key={p.id || `novo-${i}`} style={{ marginBottom: 10 }}>
          <div className="row" style={{ justifyContent: "space-between" }}>
            <input aria-label={`Nome do prompt ${i + 1}`} placeholder="nome" value={p.name} onChange={(e) => edit(i, { name: e.target.value })} style={{ flex: 1 }} />
            <button className="btn" onClick={() => setRows((r) => r?.filter((_, j) => j !== i) ?? r)}>remover</button>
          </div>
          <textarea aria-label={`Texto do prompt ${i + 1}`} rows={4} value={p.text} onChange={(e) => edit(i, { text: e.target.value })}
            style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }} />
          <label className="row" style={{ gap: 6 }}>
            <input type="checkbox" checked={p.sendEnter} onChange={(e) => edit(i, { sendEnter: e.target.checked })} /> enviar com Enter
          </label>
        </fieldset>
      ))}
      <div className="actions">
        <button className="btn" disabled={!rows} onClick={() => setRows((r) => [...(r ?? []), { id: "", name: "", text: "", sendEnter: false }])}>novo prompt</button>
        <button className="btn" disabled={!rows || busy} onClick={() => void save()}>salvar</button>
      </div>
    </>
  );
}
