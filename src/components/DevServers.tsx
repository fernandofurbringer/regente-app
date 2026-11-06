import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { api, errMsg, type PaneRow } from "../lib/ipc";
import { marcarParando, remover, type DevServer } from "../lib/devServers";
import { goToPane } from "../lib/focus";
import { browserUrl } from "../lib/links";
import { useStore } from "../state/store";
import "./DevServers.css";

const CTRL_C = "\x03";
const minutos = (since: number) => {
  const m = Math.floor((Date.now() - since) / 60_000);
  return m < 1 ? "há menos de 1 min" : `há ${m} min`;
};

/** Só painel de terminal recebe teclas do chip: agente e exec nunca recebem Ctrl+C (A1.7). */
function escreverTerminal(id: string, dado: string): Promise<void> {
  if (useStore.getState().panes[id]?.kind !== "terminal") return Promise.resolve();
  return api.paneWrite(id, dado);
}

/** Chip "Servidores (n)" da barra: os servidores locais do workspace ativo, anunciados pelos painéis, com as ações do A1.7. */
export default function DevServers() {
  const map = useStore((s) => s.devServers);
  const panes = useStore((s) => s.panes);
  const ativo = useStore((s) => s.active);
  const [aberto, setAberto] = useState(false);
  const [, tique] = useState(0);
  useEffect(() => {
    if (!aberto) return;
    const t = setInterval(() => tique((n) => n + 1), 30_000);
    return () => clearInterval(t);
  }, [aberto]);
  const lista = Object.values(map)
    .filter((e) => panes[e.paneId] && panes[e.paneId].workspaceId === ativo)
    .sort((a, b) => a.since - b.since);
  // sem servidor no workspace, o chip não ocupa a barra (ele aparece quando um painel anuncia um)
  if (!lista.length && !aberto) return null;
  return (
    <div className="devservers">
      <button className="btn" aria-expanded={aberto} onClick={() => setAberto((v) => !v)} title="Servidores de desenvolvimento neste workspace">
        Servidores ({lista.length})
      </button>
      {aberto && (
        <div className="devservers-pop" role="dialog" aria-label="Servidores de desenvolvimento deste workspace">
          {lista.length === 0 && <div className="meta">Nenhum servidor local anunciado pelos painéis deste workspace.</div>}
          {lista.map((e) => <Linha key={e.paneId} e={e} pane={panes[e.paneId]} fechar={() => setAberto(false)} />)}
        </div>
      )}
    </div>
  );
}

function Linha({ e, pane, fechar }: { e: DevServer; pane: PaneRow; fechar(): void }) {
  const toast = useStore((s) => s.toast);
  const falhou = (err: unknown) => toast(errMsg(err), "erro");
  const url = browserUrl(e.url);

  async function abrirInterno() {
    try {
      const row = await api.paneSpawn({ workspaceId: pane.workspaceId, missionId: pane.missionId, kind: "browser", cwd: pane.cwd, urlOrPath: url });
      goToPane(row.id);
      fechar();
    } catch (err) {
      falhou(err);
    }
  }

  // Parar: Ctrl+C e, 300 ms depois, outro (responde "Terminate batch job (Y/N)?" em qualquer idioma; no prompt não faz mal).
  // A entrada fica no chip até o painel ficar ocioso, ou 5 s depois (ponytail: sem olhar a saída de novo).
  async function parar() {
    try {
      await escreverTerminal(e.paneId, CTRL_C);
      useStore.getState().patch({ devServers: marcarParando(useStore.getState().devServers, e.paneId) });
      setTimeout(() => escreverTerminal(e.paneId, CTRL_C).catch(falhou), 300);
      setTimeout(() => {
        const st = useStore.getState();
        if (st.devServers[e.paneId]?.parando) st.patch({ devServers: remover(st.devServers, e.paneId) });
      }, 5000);
    } catch (err) {
      falhou(err);
    }
  }

  return (
    <div className="devservers-row">
      <div className="row" style={{ justifyContent: "space-between", gap: 8 }}>
        <a href={url} onClick={(ev) => { ev.preventDefault(); openUrl(url).catch(falhou); }}>{e.url}</a>
        <span className="meta">{pane.name} · {minutos(e.since)}</span>
      </div>
      <div className="row" style={{ gap: 6, flexWrap: "wrap" }}>
        <button className="btn" onClick={abrirInterno}>Abrir no navegador interno</button>
        <button className="btn" onClick={() => openUrl(url).catch(falhou)}>Abrir no navegador do sistema</button>
        <button className="btn" onClick={() => { goToPane(e.paneId); fechar(); }}>Ir ao painel</button>
        {pane.kind === "terminal" && <button className="btn danger" onClick={parar} disabled={e.parando}>{e.parando ? "Parando…" : "Parar"}</button>}
      </div>
    </div>
  );
}
