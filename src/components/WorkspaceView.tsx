import { lazy, Suspense, useEffect, useState } from "react";
import { useStore } from "../state/store";
import FileTree from "./files/FileTree";
import EditorStatus from "./files/EditorStatus";
import MissionStrip from "./MissionStrip";
import PaneGrid from "./PaneGrid";
import RestorePrompt from "./RestorePrompt";
import MissionTasks from "./MissionTasks";
import SpawnSlots from "./SpawnSlots";
import { GitFooter, useGitState } from "./MissionBar";
import Gutter from "./Gutter";
import { GUTTER_PX } from "../lib/layout";
import { shellDrag, useShellPrefs } from "../lib/layoutStore";

// o editor (Monaco) só monta quando a visão de arquivos é aberta pela primeira vez; o módulo é carregado antes, com o
// app ocioso, para a primeira abertura não congelar a janela enquanto o Monaco é avaliado
const carregarEditor = () => import("./files/EditorArea");
const EditorArea = lazy(carregarEditor);
let editorPedido = false;
function preCarregarEditor() {
  if (editorPedido) return;
  editorPedido = true;
  const pedir = () => void carregarEditor().catch(() => { editorPedido = false; });
  const ric = (window as Window & { requestIdleCallback?: (cb: () => void, o?: { timeout: number }) => number }).requestIdleCallback;
  if (ric) ric(pedir, { timeout: 8000 });
  else window.setTimeout(pedir, 3000);
}

export default function WorkspaceView({ wsId, hidden }: { wsId: string; hidden: boolean }) {
  const view = useStore((s) => s.view[wsId] ?? "terminais");
  const [filesVisited, setFilesVisited] = useState(false);
  if (view === "arquivos" && !filesVisited) setFilesVisited(true);
  const ws = useStore((s) => s.workspaces.find((w) => w.id === wsId));
  const sel = useStore((s) => s.missionSel[wsId] ?? null);
  const mission = useStore((s) => (sel ? (s.missions[wsId] ?? []).find((m) => m.id === sel) : undefined));
  // um só estado git por workspace: o rodapé lê daqui
  const git = useGitState(wsId, mission ?? null, hidden);
  // a árvore da missão com worktree é o worktree dela (onde os agentes escrevem); "Projeto" volta para a pasta do workspace
  const [arvoreProjeto, setArvoreProjeto] = useState(false);
  const wt = mission?.worktreePath ?? null;
  const raizArvore = wt && !arvoreProjeto ? wt : ws?.path ?? "";
  const outraRaiz = wt ? (raizArvore === wt ? ws?.path ?? null : wt) : null;
  // "Revelar na árvore" de um arquivo da outra árvore: troca e repete o pedido quando a árvore nova montar
  useEffect(() => {
    if (!wt || !ws) return;
    const norm = (p: string) => p.replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase();
    const onRevelar = (ev: Event) => {
      const alvo = norm((ev as CustomEvent<string>).detail);
      const naMissao = alvo.startsWith(`${norm(wt)}/`);
      if (naMissao === !arvoreProjeto) return;
      setArvoreProjeto(!naMissao);
      window.setTimeout(() => window.dispatchEvent(new CustomEvent("regente:revelar", { detail: (ev as CustomEvent<string>).detail })), 150);
    };
    window.addEventListener("regente:revelar", onRevelar);
    return () => window.removeEventListener("regente:revelar", onRevelar);
  }, [wt, ws, arvoreProjeto]);
  const shell = useShellPrefs();
  useEffect(() => { if (!hidden) preCarregarEditor(); }, [hidden]);
  return (
    <div className="ws" style={{ display: hidden ? "none" : undefined }}>
      <div className="main">
        {/* missões numa faixa fina acima da grade: a largura toda fica para os painéis */}
        <MissionStrip wsId={wsId} />
        <RestorePrompt wsId={wsId} />
        <SpawnSlots wsId={wsId} />
        {sel && <MissionTasks missionId={sel} />}
        {/* a grade fica montada mesmo na visão de arquivos para preservar os terminais */}
        <div style={{ display: view === "terminais" ? "flex" : "none", flex: 1, minHeight: 0, flexDirection: "column" }}>
          <PaneGrid wsId={wsId} />
        </div>
        {ws && (
          <div className="files" style={{ display: view === "arquivos" ? "grid" : "none", gridTemplateColumns: `${shell.treePx}px ${GUTTER_PX}px minmax(0, 1fr)` }}>
            <div style={{ display: "flex", flexDirection: "column", minHeight: 0, background: "var(--glass)" }}>
              {wt && (
                <div className="seg tree-scope" role="group" aria-label="Qual árvore">
                  <button className={`tb-btn ${!arvoreProjeto ? "on" : ""}`} aria-pressed={!arvoreProjeto} title={wt} onClick={() => setArvoreProjeto(false)}>Missão</button>
                  <button className={`tb-btn ${arvoreProjeto ? "on" : ""}`} aria-pressed={arvoreProjeto} title={ws.path} onClick={() => setArvoreProjeto(true)}>Projeto</button>
                </div>
              )}
              <FileTree key={raizArvore} root={raizArvore} outraRaiz={outraRaiz} onOpen={(path) => useStore.getState().patch({ openFile: { ...useStore.getState().openFile, [wsId]: { path } } })} />
            </div>
            <Gutter axis="x" label="Largura da árvore de arquivos" {...shellDrag("treePx")} />
            {filesVisited && <Suspense fallback={<div className="empty">Carregando editor…</div>}><EditorArea wsId={wsId} /></Suspense>}
          </div>
        )}
        {/* barra de status: o editor à esquerda (só na visão de arquivos), o Git à direita */}
        <div className="statusbar">
          {view === "arquivos" && <EditorStatus wsId={wsId} />}
          <GitFooter wsId={wsId} mission={mission ?? null} git={git} hidden={hidden} />
        </div>
      </div>
    </div>
  );
}
