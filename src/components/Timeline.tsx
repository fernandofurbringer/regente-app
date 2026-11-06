import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, errMsg, type Checkpoint, type FileVersions } from "../lib/ipc";
import { langOf } from "../lib/review";
import { useStore } from "../state/store";
import Modal from "./Modal";
import { DiffView } from "./MissionReview";
import { editorTheme } from "../lib/theme";

const hora = (ms: number) => new Date(ms).toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/** Linha do tempo de checkpoints de uma missão ou workspace: o que cada um mudou, o diff e a volta para um deles. */
export default function Timeline() {
  const tl = useStore((s) => s.timeline);
  const theme = useStore((s) => s.settings?.theme);
  const toast = useStore((s) => s.toast);
  const [list, setList] = useState<Checkpoint[] | null>(null);
  const [sel, setSel] = useState<number | null>(null);
  const [file, setFile] = useState<string | null>(null);
  const [versions, setVersions] = useState<FileVersions | null>(null);
  const [busy, setBusy] = useState(false);
  const scope = tl?.scope ?? null;
  // um agente escrevendo agora teria o trabalho sobrescrito pelo restore
  const working = useStore((s) => Object.values(s.panes).some((p) => p.status === "trabalhando" && (tl?.mission ? p.missionId === scope : p.workspaceId === scope)));
  const close = () => useStore.getState().patch({ timeline: null });

  const load = () => {
    if (!scope) return;
    api.checkpointList(scope).then(setList, (e) => { toast(errMsg(e), "erro"); close(); });
  };

  useEffect(() => {
    setList(null);
    setSel(null);
    setFile(null);
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scope]);

  const cur = list?.find((c) => c.n === sel) ?? null;

  useEffect(() => {
    setVersions(null);
    if (!scope || sel === null || !file) return;
    let stale = false;
    api.checkpointDiff(scope, sel, file).then(
      (v) => { if (!stale) setVersions(v); },
      (e) => { if (!stale) toast(errMsg(e), "erro"); },
    );
    return () => { stale = true; };
  }, [scope, sel, file, toast]);

  const voltar = async (cp: Checkpoint) => {
    if (!scope || !tl) return;
    const quando = hora(cp.time).slice(0, 5);
    const pergunta = `${tl.mission ? "Voltar a missão" : "Voltar o workspace"} para o checkpoint das ${quando}? O estado atual vira um checkpoint de segurança.`;
    if (!(await ask(pergunta, { title: "Voltar para este checkpoint?", kind: "warning", okLabel: "Voltar", cancelLabel: "Cancelar" }))) return;
    setBusy(true);
    try {
      const r = await api.checkpointRestore(scope, cp.n);
      const desfazer = { label: "Desfazer", run: () => { api.checkpointRestore(scope, r.safety).then(() => toast("Desfeito.", "ok"), (e) => toast(errMsg(e), "erro")); } };
      if (r.falhas.length === 0) {
        toast("Arquivos voltaram ao checkpoint.", "ok", desfazer);
      } else {
        const lista = r.falhas.slice(0, 3).join("; ");
        toast(`${r.falhas.length} arquivo${r.falhas.length === 1 ? "" : "s"} não voltaram: ${lista}${r.falhas.length > 3 ? "…" : ""}`, "erro", desfazer);
      }
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      // a lista sempre recarrega: o checkpoint de segurança aparece mesmo quando a volta falhou no meio
      load();
      setBusy(false);
    }
  };

  return (
    <Modal label="Linha do tempo" onClose={close} style={{ width: "min(1100px, 96vw)", display: "flex", flexDirection: "column", gap: 10 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
        <h2>Linha do tempo</h2>
        <span className="meta">{tl?.title} · checkpoints guardam o trabalho dos agentes; o git do projeto não muda</span>
      </div>
      {list === null && <div className="meta">Lendo os checkpoints…</div>}
      {list !== null && list.length === 0 && <div className="empty">Nenhum checkpoint ainda. Eles aparecem quando um agente termina uma tarefa que mudou arquivos.</div>}
      {list !== null && list.length > 0 && (
        <div className="timeline">
          <div role="listbox" aria-label="Checkpoints" className="timeline-list">
            {list.map((c) => (
              <button key={c.n} role="option" aria-selected={c.n === sel} className={`rail-item ${c.n === sel ? "sel" : ""}`} onClick={() => { setSel(c.n); setFile(null); }}>
                <span className="meta">{hora(c.time)}</span>{" "}
                <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{c.pane || "agente"}</span>
                <span className="meta" style={{ whiteSpace: "nowrap" }}>{c.totalFiles} arq. +{c.added} −{c.removed}</span>
              </button>
            ))}
          </div>
          <div className="timeline-main">
            {!cur && <div className="meta">Escolha um checkpoint para ver o que mudou.</div>}
            {cur && (
              <>
                <div className="row" style={{ justifyContent: "space-between", gap: 8 }}>
                  <span className="meta">{hora(cur.time)} · {cur.pane || "agente"} · {cur.totalFiles} arquivo{cur.totalFiles === 1 ? "" : "s"}{cur.totalFiles > cur.files.length ? ` (mostrando ${cur.files.length})` : ""}</span>
                  <button className="btn danger" disabled={busy || working} title={working ? "Há um agente trabalhando nesta pasta" : undefined} onClick={() => voltar(cur)}>Voltar para aqui</button>
                </div>
                <div role="listbox" aria-label="Arquivos do checkpoint" className="timeline-files">
                  {cur.files.map((f) => (
                    <button key={f.path} role="option" aria-selected={f.path === file} className={`rail-item ${f.path === file ? "sel" : ""}`} onClick={() => setFile(f.path)} title={f.path}>
                      <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{f.path}</span>
                      <span className="meta" style={{ whiteSpace: "nowrap" }}>+{f.added} −{f.removed}</span>
                    </button>
                  ))}
                </div>
                <div style={{ height: "40vh", border: "1px solid var(--line)", borderRadius: 6, overflow: "hidden" }}>
                  {!file && <div className="meta" style={{ padding: 16 }}>Escolha um arquivo para ver o diff em relação ao checkpoint anterior.</div>}
                  {file && versions && !versions.binary && !versions.tooBig && (
                    <DiffView key={file} original={versions.base} modified={versions.current} language={langOf(file)} theme={editorTheme(theme)} />
                  )}
                  {file && versions?.binary && <div className="empty" style={{ padding: 16 }}>Arquivo binário: sem diff de texto.</div>}
                  {file && versions?.tooBig && <div className="empty" style={{ padding: 16 }}>Arquivo acima de 2 MB: sem diff de texto.</div>}
                  {file && !versions && <div className="meta" style={{ padding: 16 }}>Carregando o diff…</div>}
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}
