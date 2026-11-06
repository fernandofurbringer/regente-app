import { useEffect, useState } from "react";
import { api, errMsg, type ExecRow, type PaneRow } from "../lib/ipc";
import { useStore } from "../state/store";
import { useExecSaida } from "./ExecPane";

const SITUACAO: Record<ExecRow["status"], string> = { rodando: "rodando", fundo: "em segundo plano", ok: "ok", erro: "erro" };

function duracao(e: ExecRow, agora: number): string {
  const s = Math.max(0, Math.round(((e.endedAt ?? agora) - e.startedAt) / 1000));
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`;
}

/** Execuções de shell (Bash/PowerShell) de um painel do Claude: comando, situação, duração e saída. Só leitura. */
export default function ExecucoesPane({ pane }: { pane: PaneRow }) {
  const [lista, setLista] = useState<ExecRow[]>([]);
  const [agora, setAgora] = useState(() => Date.now());

  useEffect(() => {
    let stop = false;
    const load = () => {
      setAgora(Date.now());
      if (pane.parentId) api.execLista(pane.parentId).then((v) => { if (!stop) setLista(v); }).catch(() => {});
    };
    load();
    const timer = setInterval(load, 1000);
    return () => { stop = true; clearInterval(timer); };
  }, [pane.parentId]);

  if (lista.length === 0) return <div className="empty">Nenhuma execução de shell ainda.</div>;
  return (
    <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "flex", flexDirection: "column", gap: 6 }}>
      {[...lista].reverse().map((e) => <ExecItem key={e.id} exec={e} agora={agora} />)}
    </ul>
  );
}

export function ExecItem({ exec, agora }: { exec: ExecRow; agora: number }) {
  const [aberta, setAberta] = useState(false);
  // a saída só é lida com a linha aberta; ao abrir, lê do começo do que o anel ainda guarda
  const { texto } = useExecSaida(exec.parentPane, exec.id, aberta);
  const rodando = exec.status === "rodando" || exec.status === "fundo";

  return (
    <li className="meta" style={{ borderLeft: `3px solid var(--exec-${exec.status}, currentColor)`, paddingLeft: 8 }}>
      <button className="btn" aria-expanded={aberta} onClick={() => setAberta((v) => !v)} title={exec.command} style={{ fontFamily: "var(--mono, monospace)", textAlign: "left" }}>
        {exec.description ?? exec.command.slice(0, 80)}
      </button>
      <div>
        <span aria-label="situação">{SITUACAO[exec.status]}</span>
        {exec.exitCode !== null && <span> · código {exec.exitCode}</span>}
        <span> · {duracao(exec, agora)}</span>
        {exec.promotedPane && <span> · em painel próprio</span>}
        {rodando && !exec.promotedPane && (
          <button className="btn" style={{ marginLeft: 8 }} onClick={() => api.execPromover(exec.parentPane, exec.id).catch((e) => useStore.getState().toast(errMsg(e), "erro"))}>
            Abrir em painel
          </button>
        )}
      </div>
      {aberta && (
        <>
          <pre style={{ whiteSpace: "pre-wrap", maxHeight: 240, overflow: "auto" }}>{exec.command}</pre>
          <pre style={{ whiteSpace: "pre-wrap", maxHeight: 320, overflow: "auto" }}>{texto || "(sem saída)"}</pre>
        </>
      )}
    </li>
  );
}
