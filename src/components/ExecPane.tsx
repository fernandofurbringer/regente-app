import { useEffect, useRef, useState } from "react";
import { api, type ExecRow, type PaneRow } from "../lib/ipc";
import { findLocalhost } from "../lib/links";
import { registrar } from "../lib/devServers";
import { useStore } from "../state/store";

/** Teto da saída mantida na tela (espelha o anel do servidor, 2 MB). */
const TELA_MAX = 2 * 1024 * 1024;
/** Execução em segundo plano sem saída por este tempo: o painel para de ler. */
const SEM_SAIDA_MS = 60_000;
/** Intervalo de leitura depois de `SEM_SAIDA_MS` sem saída (a execução ainda não acabou). */
const SEM_SAIDA_POLL_MS = 5_000;
const enc = new TextEncoder();

/**
 * Saída de uma execução, lida de forma incremental: cada chamada pega só o que é novo (a partir da posição já
 * recebida). Enquanto a execução roda ou está em segundo plano, relê a cada 1 s; ao acabar, faz a última leitura.
 */
export function useExecSaida(parentId: string | null | undefined, execId: string | null | undefined, ativo: boolean) {
  const [exec, setExec] = useState<ExecRow | null>(null);
  const [texto, setTexto] = useState("");
  const pos = useRef(0);
  const rodando = !exec || exec.status === "rodando" || exec.status === "fundo";
  // em segundo plano sem nenhuma saída por 60 s: para de perguntar (não há o que mostrar)
  const [semSaida, setSemSaida] = useState(false);
  const abriuEm = useRef(0);

  useEffect(() => {
    pos.current = 0;
    abriuEm.current = Date.now();
    setExec(null);
    setTexto("");
    setSemSaida(false);
  }, [parentId, execId]);

  useEffect(() => {
    if (!parentId || !execId || !ativo) return;
    let stop = false;
    const load = () => api.execGet(parentId, execId, pos.current).then((v) => {
      if (stop || !v) return;
      setExec(v.exec);
      pos.current = v.de + enc.encode(v.exec.output).length;
      if (v.exec.output) setTexto((t) => (t + v.exec.output).slice(-TELA_MAX));
      if (v.exec.status === "fundo" && !v.exec.output && pos.current === 0 && Date.now() - abriuEm.current > SEM_SAIDA_MS) setSemSaida(true);
    }).catch(() => {});
    load();
    if (!rodando) return () => { stop = true; };
    // sem saída há 60 s em segundo plano: a leitura desacelera para 5 s, mas continua até a execução acabar
    const timer = setInterval(load, semSaida ? SEM_SAIDA_POLL_MS : 1000);
    return () => { stop = true; clearInterval(timer); };
  }, [parentId, execId, ativo, rodando, semSaida]);

  return { exec, texto, semSaida };
}

/** Execução promovida a painel próprio: saída ao vivo enquanto roda, "Fixar" para não fechar sozinha. Só leitura. */
export default function ExecPane({ pane }: { pane: PaneRow }) {
  const { exec, texto, semSaida } = useExecSaida(pane.parentId, pane.urlOrPath, true);
  const rodando = !exec || exec.status === "rodando" || exec.status === "fundo";
  const [fixado, setFixado] = useState(false);

  // servidor local anunciado na saída nova: acende o pulso do navegador, como no terminal
  const lido = useRef(0);
  useEffect(() => {
    const novo = texto.length > lido.current && rodando ? texto.slice(lido.current) : "";
    lido.current = texto.length;
    const url = novo ? findLocalhost(novo.slice(-4096)) : null;
    if (!url) return;
    const st = useStore.getState();
    if (st.readyUrl[pane.workspaceId] !== url) st.patch({ readyUrl: { ...st.readyUrl, [pane.workspaceId]: url } });
    // painel exec entra no mesmo registro, com "Abrir" e "Ir ao painel" só (A1.7)
    st.patch({ devServers: registrar(st.devServers, { paneId: pane.id, url, since: Date.now(), source: "exec" }) });
  }, [texto, rodando, pane.workspaceId, pane.id]);

  const fixar = () => {
    const next = !fixado;
    setFixado(next);
    void api.execFixar(pane.id, next).catch(() => setFixado(!next));
  };

  return (
    <div className="exec" style={{ display: "flex", flexDirection: "column", gap: 6, height: "100%" }}>
      <div className="row" style={{ gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <code title="Comando" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: "100%" }}>{exec?.command ?? "…"}</code>
        <span className="meta" aria-label="situação">
          {exec ? (exec.status === "rodando" ? "rodando" : exec.status === "fundo" ? "em segundo plano" : exec.status === "ok" ? "concluído" : "erro") : "carregando"}
          {exec?.exitCode != null && ` · código ${exec.exitCode}`}
        </span>
        <button className="btn" aria-pressed={fixado} onClick={fixar} title="Mantém o painel aberto depois do sucesso">
          {fixado ? "Fixado" : "Fixar"}
        </button>
      </div>
      <pre style={{ whiteSpace: "pre-wrap", margin: 0, flex: 1, overflow: "auto" }}>{texto || (semSaida ? "Sem saída disponível para este comando em segundo plano." : rodando ? "Aguardando saída…" : "(sem saída)")}</pre>
    </div>
  );
}
