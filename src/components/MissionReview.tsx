import { ask } from "@tauri-apps/plugin-dialog";
import type { editor } from "monaco-editor";
import LazyDiffEditor from "./LazyDiffEditor";
import { useEffect, useMemo, useRef, useState } from "react";
import { api, errMsg, type ChangedFile, type FileVersions, type Mission } from "../lib/ipc";
import { absOf, acceptedOf, langOf, reviewKeyAction, revertWarning, sameFile, storeAccepted, unreviewed, type Accepted } from "../lib/review";
import { useStore } from "../state/store";
import Modal from "./Modal";
import { editorTheme } from "../lib/theme";

const STATUS: Record<ChangedFile["status"], string> = { A: "novo", M: "alterado", D: "apagado" };

/**
 * Diff de um arquivo. Os modelos ficam com o Monaco até o editor sair (keepCurrent*): o próprio editor não os descarta
 * antes do widget, que lançava "TextModel got disposed before DiffEditorWidget model got reset". Descartados depois.
 */
export function DiffView({ original, modified, language, theme }: { original: string; modified: string; language: string; theme: string }) {
  const models = useRef<{ original: editor.ITextModel; modified: editor.ITextModel } | null>(null);
  useEffect(() => () => {
    // depois do desmonte do editor: o cleanup do pai pode rodar antes do filho
    const m = models.current;
    setTimeout(() => { m?.original.dispose(); m?.modified.dispose(); }, 0);
  }, []);
  return (
    <LazyDiffEditor
      original={original}
      modified={modified}
      language={language}
      theme={theme}
      keepCurrentOriginalModel
      keepCurrentModifiedModel
      onMount={(ed) => {
        const m = ed.getModel();
        if (m) models.current = { original: m.original, modified: m.modified };
      }}
      options={{ readOnly: true, renderSideBySide: true, originalEditable: false, minimap: { enabled: false } }}
    />
  );
}

/** Revisão das mudanças de uma missão: lista dos arquivos, diff lado a lado e aceitar ou reverter cada um. */
export default function MissionReview() {
  const id = useStore((s) => s.reviewMission);
  const mission = useStore((s) => (id ? Object.values(s.missions).flat().find((m) => m.id === id) : undefined));
  // o guardado vale até o primeiro aceite da sessão; depois, o da store (fora do seletor: não pode devolver objeto novo)
  const stored = useStore((s) => (id ? s.reviewed[id] : undefined));
  // lido do navegador uma vez por missão (não a cada render)
  const persisted = useMemo<Accepted>(() => (id ? acceptedOf(id) : {}), [id]);
  const accepted: Accepted = stored ?? persisted;
  const theme = useStore((s) => s.settings?.theme);
  const [files, setFiles] = useState<ChangedFile[] | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [versions, setVersions] = useState<FileVersions | null>(null);
  const [busy, setBusy] = useState(false);
  const close = () => useStore.getState().patch({ reviewMission: null });

  const list = files ?? [];
  const pending = unreviewed(list, accepted);
  const isAccepted = (f: ChangedFile) => accepted[f.path] === f.fingerprint;
  const cur = list.find((f) => f.path === sel) ?? null;

  const load = async (m: Mission) => {
    const fresh = await api.missionDiffFiles(m.id);
    setFiles(fresh);
    setSel((c) => (c && fresh.some((f) => f.path === c) ? c : fresh[0]?.path ?? null));
    return fresh;
  };

  useEffect(() => {
    if (!mission) return;
    setFiles(null);
    setSel(null);
    load(mission).catch((e) => { useStore.getState().toast(errMsg(e), "erro"); close(); });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mission?.id]);

  useEffect(() => {
    if (!mission || !sel) return setVersions(null);
    let stale = false;
    setVersions(null);
    api.missionFileVersions(mission.id, sel)
      .then((v) => { if (!stale) setVersions(v); })
      .catch((e) => { if (!stale) useStore.getState().toast(errMsg(e), "erro"); });
    return () => { stale = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mission?.id, sel]);

  /** grava como aceitos (com o hash atual do conteúdo) e atualiza a revisão na sessão; só guarda caminhos ainda mudados */
  function saveAccepted(fs: ChangedFile[]) {
    if (!mission) return;
    const live = new Set(list.map((f) => f.path));
    const next: Accepted = {};
    for (const [p, h] of Object.entries(accepted)) if (live.has(p)) next[p] = h;
    for (const f of fs) next[f.path] = f.fingerprint;
    storeAccepted(mission.id, next);
    const st = useStore.getState();
    st.patch({ reviewed: { ...st.reviewed, [mission.id]: next } });
  }

  function accept(f: ChangedFile) {
    saveAccepted([f]);
    // o próximo arquivo ainda não aceito
    const next = list.find((x) => x.path !== f.path && accepted[x.path] !== x.fingerprint);
    if (next) setSel(next.path);
  }

  async function revert(f: ChangedFile) {
    const st = useStore.getState();
    const root = mission!.worktreePath ?? "";
    if ((st.dirty[mission!.workspaceId] ?? []).some((p) => sameFile(p, absOf(root, f.path)))) {
      return st.toast("Salve ou descarte as alterações desse arquivo no editor antes de reverter.", "erro");
    }
    if (!(await ask(`Reverter "${f.path}" para a base? ${revertWarning(f)}`, { title: "Reverter arquivo", kind: "warning" }))) return;
    setBusy(true);
    try {
      await api.missionRevertFile(mission!.id, f.path);
      st.toast("Arquivo revertido", "ok");
      // a revisão guarda o conteúdo novo do arquivo (o revertido já está aceito)
      const fresh = await load(mission!);
      const nf = fresh.find((x) => x.path === f.path);
      if (nf) saveAccepted([nf]);
    } catch (e) {
      st.toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  // atalhos: j/k navegam, a aceita, r reverte (com a confirmação); fora de campos de texto e do editor
  useEffect(() => {
    if (!id || !mission) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey || e.repeat || busy || list.length === 0) return;
      if ((e.target as HTMLElement).closest?.("input, textarea, select, [contenteditable='true']")) return;
      const act = reviewKeyAction(e.key);
      if (!act) return;
      e.preventDefault();
      const i = Math.max(0, list.findIndex((f) => f.path === sel));
      if (act === "next") setSel(list[(i + 1) % list.length].path);
      else if (act === "prev") setSel(list[(i - 1 + list.length) % list.length].path);
      else if (cur && act === "accept") accept(cur);
      else if (cur && act === "revert") void revert(cur);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (!id || !mission) return null;

  return (
    <Modal label="Revisar mudanças" onClose={close} style={{ width: "min(1100px, 96vw)", display: "flex", flexDirection: "column", gap: 10 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
        <h2>Revisar mudanças</h2>
        <span className="meta">{mission.name} · {mission.branch} → {mission.baseBranch} · j/k navegam, a aceita, r reverte</span>
      </div>
      {files === null && <div className="meta">Lendo as mudanças da missão…</div>}
      {files !== null && list.length === 0 && <div className="empty">Nenhuma mudança nesta missão em relação a {mission.baseBranch}.</div>}
      {list.length > 0 && (
        <div style={{ display: "grid", gridTemplateColumns: "minmax(180px, 260px) 1fr", gap: 12, minHeight: 420 }}>
          <div role="listbox" aria-label="Arquivos alterados" style={{ display: "flex", flexDirection: "column", gap: 2, overflowY: "auto", maxHeight: "60vh" }}>
            <div className="row" style={{ justifyContent: "space-between", gap: 8 }}>
              <span className="meta">{list.length - pending.length} de {list.length} aceitos</span>
              {pending.length === 0 && mission ? (
                <button className="btn primary" disabled={busy} onClick={() => useStore.getState().patch({ reviewMission: null, gitDialog: { kind: "commit", wsId: mission.workspaceId, missionId: mission.id } })}>Commitar agora</button>
              ) : (
                <button className="btn" disabled={busy || pending.length === 0} onClick={() => saveAccepted(pending)}>Aceitar todos ({pending.length})</button>
              )}
            </div>
            {list.map((f) => (
              <button key={f.path} role="option" aria-selected={f.path === sel} className={`rail-item ${f.path === sel ? "sel" : ""}`}
                title={`${f.path} (${STATUS[f.status]})`} onClick={() => setSel(f.path)} style={{ textAlign: "left" }}>
                <span className="chip">{f.status}</span>{" "}
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", flex: 1, minWidth: 0 }}>{f.path}</span>
                <span className="meta" style={{ whiteSpace: "nowrap" }}>+{f.add} −{f.del}</span>
                {isAccepted(f) && <span className="meta"> ✓</span>}
              </button>
            ))}
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 8, minWidth: 0 }}>
            <div style={{ height: "56vh", border: "1px solid var(--line)", borderRadius: 6, overflow: "hidden" }}>
              {cur && versions && !versions.binary && !versions.tooBig && (
                <DiffView key={cur.path} original={versions.base} modified={versions.current} language={langOf(cur.path)} theme={editorTheme(theme)} />
              )}
              {cur && versions?.binary && <div className="empty" style={{ padding: 16 }}>Arquivo binário: sem diff de texto.</div>}
              {cur && versions?.tooBig && <div className="empty" style={{ padding: 16 }}>Arquivo acima de 2 MB: sem diff de texto.</div>}
              {cur && !versions && <div className="meta" style={{ padding: 16 }}>Carregando o diff…</div>}
            </div>
            {cur && (
              <div className="row" style={{ gap: 8, justifyContent: "flex-end" }}>
                <button className="btn danger" disabled={busy} onClick={() => revert(cur)}>Reverter arquivo</button>
                <button className="btn primary" disabled={busy || isAccepted(cur)} onClick={() => accept(cur)}>
                  {isAccepted(cur) ? "Aceito" : "Aceitar"}
                </button>
              </div>
            )}
          </div>
        </div>
      )}
    </Modal>
  );
}
