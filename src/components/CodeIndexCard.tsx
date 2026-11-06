import { useEffect, useState } from "react";
import { api, errMsg, type CodeIndexStats } from "../lib/ipc";
import { useStore } from "../state/store";

/** "31 mil" a partir de 1000; abaixo disso, o número cheio. */
const milSimbolos = (n: number) => (n >= 1000 ? `${Math.round(n / 1000)} mil` : String(n));

/** Índice de código do workspace ativo: contagens e o interruptor do perfil (ajuste deste workspace). */
export default function CodeIndexCard() {
  const active = useStore((s) => s.active);
  const [st, setSt] = useState<CodeIndexStats | null>(null);
  useEffect(() => {
    let vivo = true;
    setSt(null);
    if (active) api.codeIndexStats(active).then((r) => { if (vivo) setSt(r); }).catch(() => { if (vivo) setSt(null); });
    return () => { vivo = false; };
  }, [active]);

  if (!active) {
    return <div className="card"><strong>Índice de código</strong><div className="meta">Escolha um workspace para ver o índice.</div></div>;
  }
  const alternar = async (ligado: boolean) => {
    const st0 = useStore.getState();
    try {
      await api.codeIndexSet(active, ligado);
      setSt(await api.codeIndexStats(active));
    } catch (e) {
      st0.toast(errMsg(e), "erro");
    }
  };
  return (
    <div className="card">
      <strong>Índice de código</strong>
      <small style={{ display: "block", color: "var(--fg3)" }}>Dá às IAs ferramentas para ver símbolos e buscar no código sem abrir arquivos inteiros.</small>
      {st == null ? <div className="meta" aria-live="polite">Lendo o índice…</div> : (
        <>
          <div>
            {st.sensivel ? "Desligado: workspace sensível."
              : `Índice: ${st.arquivos.toLocaleString("pt-BR")} arquivos, ${milSimbolos(st.simbolos)} símbolos${st.atualizadoMs ? `, atualizado há ${Math.max(0, Math.round((Date.now() - st.atualizadoMs) / 1000))} s` : ""}.`}
          </div>
          <label className="row"><input type="checkbox" checked={st.ligado} disabled={st.sensivel} onChange={(e) => void alternar(e.target.checked)} /> Usar o índice neste workspace</label>
        </>
      )}
    </div>
  );
}
