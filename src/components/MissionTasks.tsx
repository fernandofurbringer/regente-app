import { useEffect, useState } from "react";
import { api, errMsg, on, type Handoff, type TaskItem } from "../lib/ipc";
import { goToPane } from "../lib/focus";
import { AgentMarkdown } from "../lib/markdown";
import { useStore } from "../state/store";

const GROUPS: { status: string; label: string }[] = [
  { status: "a_fazer", label: "A fazer" },
  { status: "fazendo", label: "Fazendo" },
  { status: "feito", label: "Feito" },
];

/** Estados do plano (`planexec`) nos grupos do placar; "substituida" fica de fora. */
const GRUPO_DO_PLANO: Record<string, string> = { pendente: "a_fazer", bloqueada: "a_fazer", em_execucao: "fazendo", concluida: "feito" };
const grupoDo = (status: string) => GRUPO_DO_PLANO[status] ?? status;

/** Placar read-only das tarefas da missão (o piloto/maestro as escreve pelo MCP), e as entregas dos ajudantes. */
export default function MissionTasks({ missionId }: { missionId: string }) {
  const [tasks, setTasks] = useState<TaskItem[] | null>(null);
  const [plano, setPlano] = useState<string | null>(null);
  const ws = useStore((s) => Object.keys(s.missions).find((w) => s.missions[w].some((m) => m.id === missionId)));

  useEffect(() => {
    let alive = true;
    const load = () => {
      api.planGet(missionId).then((p) => { if (alive) setPlano(p.arquivo); }).catch(() => {});
      return api.tasksList(missionId)
        .then((t) => { if (alive) setTasks(t); })
        .catch((e) => useStore.getState().toast(errMsg(e), "erro"));
    };
    setTasks(null);
    load();
    const sub = on<{ missionId: string }>("tasks-changed", (e) => { if (e.missionId === missionId) load(); });
    return () => {
      alive = false;
      sub.then((u) => u());
    };
  }, [missionId]);

  const placar = tasks === null ? <div className="tasks" aria-live="polite">Carregando tarefas…</div>
    : !tasks.length ? <div className="tasks" aria-live="polite"><span className="meta">Nenhuma tarefa ainda.</span></div>
    : (
      <section className="tasks" aria-label="Tarefas da missão" aria-live="polite">
        <div className="row">
          {GROUPS.map((g) => (
            <span key={g.status} className="chip">{g.label} {tasks.filter((t) => grupoDo(t.status) === g.status).length}</span>
          ))}
          {tasks.some((t) => t.status === "bloqueada") && (
            <button className="btn" onClick={() => api.planResume(missionId).catch((e) => useStore.getState().toast(errMsg(e), "erro"))}>Retomar plano</button>
          )}
          {plano && ws && (
            <button className="btn" onClick={() => {
              const st = useStore.getState();
              st.patch({ openFile: { ...st.openFile, [ws]: { path: plano } }, view: { ...st.view, [ws]: "arquivos" } });
            }}>Abrir plano</button>
          )}
        </div>
        {GROUPS.map((g) => {
          const list = tasks.filter((t) => grupoDo(t.status) === g.status);
          return list.length ? (
            <div key={g.status}>
              <h4>{g.label}</h4>
              <ul>{list.map((t) => <li key={t.id}>{t.title}</li>)}</ul>
            </div>
          ) : null;
        })}
      </section>
    );

  return (
    <>
      {placar}
      <MissionHandoffs missionId={missionId} />
    </>
  );
}

/** Entregas da missão: autor, hora, resumo em Markdown, arquivos e artefato que abrem no editor. Atualiza com o evento `handoff`. */
function MissionHandoffs({ missionId }: { missionId: string }) {
  const [rows, setRows] = useState<Handoff[]>([]);
  const panes = useStore((s) => s.panes);
  const ws = useStore((s) => Object.keys(s.missions).find((w) => s.missions[w].some((m) => m.id === missionId)));

  useEffect(() => {
    let alive = true;
    const load = () => api.handoffList(missionId)
      .then((r) => { if (alive) setRows(r); })
      .catch((e) => useStore.getState().toast(errMsg(e), "erro"));
    setRows([]);
    load();
    const sub = on<{ missionId: string }>("handoff", (e) => { if (e.missionId === missionId) load(); });
    return () => {
      alive = false;
      sub.then((u) => u());
    };
  }, [missionId]);

  if (!rows.length) return null;

  // a marcação de visto é da missão inteira (o backend não guarda por entrega)
  const markSeen = () => api.missionMarkSeen(missionId)
    .then(() => { if (ws) return useStore.getState().loadUnseen(ws); })
    .then(() => api.handoffList(missionId))
    .then(setRows)
    .catch((e) => useStore.getState().toast(errMsg(e), "erro"));

  return (
    <section className="tasks entregas" aria-label="Entregas da missão">
      <div className="row">
        <h4 style={{ margin: "6px 0 2px" }}>Entregas</h4>
        {rows.some((h) => !h.seen) && <button className="btn" onClick={markSeen}>Marcar como visto</button>}
      </div>
      <ul style={{ listStyle: "none", padding: 0, margin: 0 }}>
        {rows.map((h) => (
          <Entrega key={h.id} h={h} paneName={panes[h.fromPane]?.name ?? "Painel encerrado"} ws={ws} />
        ))}
      </ul>
    </section>
  );
}

type ArtifactRef = { path: string; format?: string; lines?: number };

function filesOf(json: string | null): string[] {
  try {
    const f = JSON.parse(json ?? "null");
    return Array.isArray(f) ? f.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function artifactOf(json: string | null): ArtifactRef | null {
  try {
    const a = JSON.parse(json ?? "null")?.artifact;
    return a && typeof a.path === "string" ? { path: a.path, format: a.format, lines: a.lines } : null;
  } catch {
    return null;
  }
}

function Entrega({ h, paneName, ws }: { h: Handoff; paneName: string; ws: string | undefined }) {
  const painelAberto = useStore((s) => !!s.panes[h.fromPane]);
  const files = filesOf(h.filesJson);
  const artifact = artifactOf(h.dataJson);
  const hora = new Date(h.createdAt).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" });
  const detalhe = artifact ? [artifact.format, artifact.lines != null ? `${artifact.lines} linhas` : null].filter(Boolean).join(", ") : "";
  // abre o arquivo no editor do workspace da missão
  const abrir = (path: string) => {
    if (!ws) return;
    const st = useStore.getState();
    st.patch({ openFile: { ...st.openFile, [ws]: { path } }, view: { ...st.view, [ws]: "arquivos" } });
  };
  return (
    <li style={{ padding: "6px 0", borderTop: "1px solid var(--line)" }}>
      <div className="row">
        <strong>{paneName}</strong>
        <span className="meta">{hora}</span>
        {!h.seen && <span className="chip" data-s="concluido">novo</span>}
        {painelAberto && <button className="btn" onClick={() => goToPane(h.fromPane)}>Ir ao painel</button>}
      </div>
      <AgentMarkdown>{h.summary}</AgentMarkdown>
      {artifact && (
        <p className="meta">
          Artefato: {artifact.path}{detalhe ? ` (${detalhe})` : ""} <button className="btn" onClick={() => abrir(artifact.path)}>Abrir</button>
        </p>
      )}
      {files.length > 0 && (
        <ul style={{ listStyle: "none", padding: 0, margin: "4px 0 0" }}>
          {files.map((f) => (
            <li key={f}><button className="btn" title="Abrir no editor" onClick={() => abrir(f)}>{f}</button></li>
          ))}
        </ul>
      )}
    </li>
  );
}
