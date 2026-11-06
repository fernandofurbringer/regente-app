import { useEffect, useState } from "react";
import { api, type PaneRow, type SubagentItem, type SubagentView } from "../lib/ipc";

/** Painel de um subagente do Claude: a conversa dele, lida do transcrito a cada segundo enquanto trabalha. Só leitura. */
export default function SubagentPane({ pane }: { pane: PaneRow }) {
  const [view, setView] = useState<SubagentView | null>(null);
  const running = pane.status === "trabalhando" || pane.status === "iniciando";

  useEffect(() => {
    let stop = false;
    const load = () => api.subagentItems(pane.id).then((v) => { if (!stop) setView(v); }).catch(() => {});
    load(); // uma vez ao montar e ao mudar de estado (a última leitura pega o fim)
    if (!running) return () => { stop = true; };
    const timer = setInterval(load, 1000);
    return () => { stop = true; clearInterval(timer); };
  }, [pane.id, running]);

  const items = view?.items ?? [];
  return (
    <div className="subagent">
      {view?.description && <p className="meta" title="Descrição do subagente">{view.description}</p>}
      {pane.status === "erro" && <p className="meta">O painel pai terminou antes deste subagente.</p>}
      {/* aberto enquanto trabalha; recolhido ao concluir */}
      <details open={running}>
        <summary>Conversa ({items.length})</summary>
        {items.length === 0
          ? <div className="empty">Aguardando o transcrito do subagente…</div>
          : items.map((item, i) => <SubagentRow key={i} item={item} />)}
      </details>
    </div>
  );
}

export function SubagentRow({ item }: { item: SubagentItem }) {
  if (item.kind === "tool") return <p className="meta">Ferramenta {item.name} ({item.status})</p>;
  return (
    <div className="subagent-msg">
      <strong>{item.role === "voce" ? "Você" : "Claude"}</strong>
      <div style={{ whiteSpace: "pre-wrap" }}>{item.text}</div>
    </div>
  );
}
