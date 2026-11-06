import { useState } from "react";
import { api, errMsg } from "../lib/ipc";
import { faixaContexto, rotuloProposta, textoContexto } from "../lib/contexto";
import { useStore } from "../state/store";
import "./ContextMeter.css";

/** Medidor de contexto do painel: barra fina no cabeçalho (verde < 50 %, âmbar até 75 %, vermelho acima). Some sem medida.
 *  Com proposta do motor (painel do usuário na fronteira), mostra um chip: um clique compacta ou recomeça. */
/** `semTab`: a cópia do cabeçalho fica fora do Tab (pode estar escondida na 2ª linha dos selos); a do menu ⋯ é a do teclado. */
export default function ContextMeter({ paneId, semTab = false }: { paneId: string; semTab?: boolean }) {
  const ctx = useStore((s) => s.paneCtx[paneId]);
  const proposta = useStore((s) => s.compactPropostas[paneId]);
  const [ocupado, setOcupado] = useState(false);
  if (!ctx) return null;
  const pct = Math.max(0, Math.min(100, Math.round(ctx.pct)));
  const faixa = faixaContexto(pct);
  const texto = textoContexto(pct, ctx.tokens, ctx.janela);
  const rotulo = rotuloProposta(proposta?.acao);
  const agir = async () => {
    if (!proposta) return;
    setOcupado(true);
    try {
      if (proposta.acao === "recomecar") await api.paneFreshStart(paneId);
      else await api.paneCompact(paneId);
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    } finally {
      setOcupado(false);
    }
  };
  return (
    <span className="ctx-meter-wrap">
      <span className="ctx-meter" data-faixa={faixa} role="meter" aria-label="Uso do contexto" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} title={texto}>
        <span className="ctx-meter-fill" style={{ width: `${pct}%` }} />
      </span>
      {rotulo && (
        <button type="button" className="ctx-chip" tabIndex={semTab ? -1 : undefined} disabled={ocupado} title={proposta?.motivo} onClick={agir}>
          {rotulo}
        </button>
      )}
    </span>
  );
}
