import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { api, type ExecRow, type PaneRow, type SubagentView } from "../lib/ipc";
import { activitySummary } from "../lib/activity";
import { useStore } from "../state/store";
import { ExecItem } from "./ExecucoesPane";
import { SubagentRow } from "./SubagentPane";
import "./PaneActivity.css";

const ATIVO = ["iniciando", "trabalhando", "pergunta"];

/** Faixa de atividade de um painel do Claude: subtarefas (subagentes inline) e comandos de shell. Só aparece quando há
 *  algo; clique ou Ctrl+Shift+J abre a gaveta sobre o terminal. Os comandos só são relidos com o pai ativo ou a gaveta aberta. */
export default function PaneActivity({ pane }: { pane: PaneRow }) {
  const inline = useStore((s) => !(s.settings?.subagentPanes ?? false));
  const subs = useStore(useShallow((s) => (inline ? Object.values(s.panes).filter((p) => p.parentId === pane.id && p.kind === "subagent" && p.closedAt === null) : [])));
  const open = useStore((s) => !!s.activityOpen[pane.id]);
  const [execs, setExecs] = useState<ExecRow[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const ativo = ATIVO.includes(pane.status);
  // comando ainda rodando (inclusive em segundo plano com o Claude ocioso): continua relendo para a faixa não mentir
  const comandoVivo = execs.some((e) => e.status === "rodando" || e.status === "fundo");

  useEffect(() => {
    let stop = false;
    const load = () => {
      setNow(Date.now());
      api.execLista(pane.id).then((v) => { if (!stop) setExecs(v); }).catch(() => {});
    };
    load(); // uma leitura ao montar e a cada mudança de estado do pai
    if (!ativo && !open && !comandoVivo) return () => { stop = true; };
    const t = setInterval(load, 1000);
    return () => { stop = true; clearInterval(t); };
  }, [pane.id, ativo, open, comandoVivo]);

  // a gaveta não cresce sem fim: as que rodam e as 8 concluídas mais recentes; o resumo segue contando todas
  const rodando = subs.filter((s) => ATIVO.includes(s.status));
  const concluidas = subs.filter((s) => !ATIVO.includes(s.status)).sort((a, b) => b.createdAt - a.createdAt);
  const visiveis = [...rodando, ...concluidas.slice(0, 8)];
  const escondidas = concluidas.length - Math.min(8, concluidas.length);

  const resumo = activitySummary(subs, execs, now);
  if (!resumo) return null;
  const toggle = () => {
    const st = useStore.getState();
    st.patch({ activityOpen: { ...st.activityOpen, [pane.id]: !open } });
  };
  const algoRodando = subs.some((s) => ATIVO.includes(s.status)) || comandoVivo;
  return (
    <>
      <button type="button" className="pane-activity" aria-expanded={open} title="Subtarefas e comandos deste painel (Ctrl+Shift+J)" onClick={toggle}>
        <span className="pane-activity-dot" data-ativo={algoRodando || undefined} />
        <span className="pane-activity-text">{resumo}</span>
        <span className="pane-activity-chev" aria-hidden="true">{open ? "▾" : "▴"}</span>
      </button>
      {open && (
        <div className="pane-activity-drawer" role="region" aria-label="Atividade do painel">
          {subs.length > 0 && <h4>Subtarefas</h4>}
          {visiveis.map((s) => <SubtarefaItem key={s.id} pane={s} />)}
          {escondidas > 0 && <div className="meta">e mais {escondidas} {escondidas === 1 ? "concluída" : "concluídas"} antes</div>}
          {execs.length > 0 && <h4>Comandos</h4>}
          {execs.length > 0 && (
            <ul className="pane-activity-execs">
              {[...execs].reverse().map((e) => <ExecItem key={e.id} exec={e} agora={now} />)}
            </ul>
          )}
        </div>
      )}
    </>
  );
}

/** Uma subtarefa: nome, estado e a conversa (aberta enquanto trabalha), lida do transcrito como no painel próprio. */
function SubtarefaItem({ pane }: { pane: PaneRow }) {
  const [view, setView] = useState<SubagentView | null>(null);
  const rodando = ATIVO.includes(pane.status);
  useEffect(() => {
    let stop = false;
    const load = () => api.subagentItems(pane.id).then((v) => { if (!stop) setView(v); }).catch(() => {});
    load();
    if (!rodando) return () => { stop = true; };
    const t = setInterval(load, 1000);
    return () => { stop = true; clearInterval(t); };
  }, [pane.id, rodando]);
  const items = view?.items ?? [];
  return (
    <details className="pane-activity-sub" open={rodando}>
      <summary>
        <span className="dot" data-s={pane.status} /> {pane.name} <span className="meta">{view?.description ?? ""}</span>
      </summary>
      {items.length === 0 ? <div className="meta">Aguardando o transcrito…</div> : items.map((it, i) => <SubagentRow key={i} item={it} />)}
    </details>
  );
}
