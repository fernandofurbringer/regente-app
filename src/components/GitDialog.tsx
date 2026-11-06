import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { canCommit, DEFAULT_BASE, GIT_CODE, sourceNote } from "../lib/gitflow";
import { api, errMsg, type GitEntry } from "../lib/ipc";
import { useStore } from "../state/store";
import Modal from "./Modal";

interface Target { wsId: string; missionId: string | null }

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const close = () => useStore.getState().patch({ gitDialog: null });

/** Commit e abertura de PR, no workspace ou na missão escolhida (worktree). Aberto pelo rail, pela paleta. */
export default function GitDialog() {
  const g = useStore((s) => s.gitDialog);
  if (!g) return null;
  const t: Target = { wsId: g.wsId, missionId: g.missionId };
  return g.kind === "commit" ? <CommitDialog t={t} /> : <PrDialog t={t} />;
}

/** A missão do alvo (para o título e a branch), ou null no workspace. */
function useMissionOf(t: Target) {
  return useStore((s) => (t.missionId ? (s.missions[t.wsId] ?? []).find((m) => m.id === t.missionId) ?? null : null));
}

function CommitDialog({ t }: { t: Target }) {
  const mission = useMissionOf(t);
  const [entries, setEntries] = useState<GitEntry[] | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const [msg, setMsg] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const msgRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    api.gitStatus(t.wsId, t.missionId)
      .then(async (es) => {
        setEntries(es);
        setPicked(es.map((e) => e.path));
        // sem IA configurada a sugestão é local e grátis: já vem preenchida (com IA, o botão segue sendo o caminho)
        const pre = es.length ? await api.gitCommitPrefill(es.map((e) => e.path)).catch(() => null) : null;
        // o pré-preenchimento nunca apaga o que já foi digitado
        if (pre) { setMsg((m) => m || pre); setNote(sourceNote("modelo", null)); }
        // o foco vai para a mensagem só se o usuário ainda não foi para outro lugar do diálogo
        const at = document.activeElement;
        if (!at || at === document.body || at.getAttribute("role") === "dialog") msgRef.current?.focus();
      })
      .catch((e) => { toast(e); close(); });
  }, [t.wsId, t.missionId]);

  const toggle = (path: string) => setPicked((p) => (p.includes(path) ? p.filter((x) => x !== path) : [...p, path]));

  async function suggest() {
    setBusy(true);
    try {
      const r = await api.gitCommitSuggest(t.wsId, t.missionId, picked);
      setMsg(r.message);
      setNote(sourceNote(r.source, r.aiError));
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  /** commita; `then: "pr"` encadeia o diálogo de PR (ele já abre preenchido) */
  async function commit(then?: "pr") {
    setBusy(true);
    try {
      const hash = await api.gitCommit(t.wsId, t.missionId, picked, msg.trim());
      useStore.getState().toast(`Commit ${hash} criado`, "ok");
      if (then === "pr") useStore.getState().patch({ gitDialog: { kind: "pr", wsId: t.wsId, missionId: t.missionId } });
      else close();
    } catch (e) {
      toast(e);
      setBusy(false);
    }
  }

  // Ctrl+Enter vale em qualquer ponto do diálogo (não só na mensagem)
  const onCtrlEnter = (e: ReactKeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && !busy && canCommit(msg, picked)) { e.preventDefault(); commit(); }
  };

  return (
    <Modal label="Commitar" onClose={close} style={{ width: "min(720px, 94vw)", display: "flex", flexDirection: "column", gap: 10 }}>
      <div style={{ display: "contents" }} onKeyDown={onCtrlEnter}>
      <h2>Commitar{mission ? ` na missão ${mission.name}` : " no workspace"}</h2>
      {mission?.branch && <div className="meta">Branch {mission.branch}</div>}
      {entries === null && <div className="meta">Lendo as alterações…</div>}
      {entries !== null && entries.length === 0 && <div className="empty">Nada para commitar.</div>}
      {entries !== null && entries.length > 0 && (
        <fieldset style={{ border: "1px solid var(--line)", borderRadius: 6, maxHeight: "26vh", overflowY: "auto", display: "flex", flexDirection: "column", gap: 2 }}>
          <legend className="meta">Arquivos a incluir ({picked.length} de {entries.length})</legend>
          {entries.map((e) => (
            <label key={e.path} className="row" style={{ gap: 6 }}>
              <input type="checkbox" checked={picked.includes(e.path)} onChange={() => toggle(e.path)} />
              <span className="chip">{e.code}</span>
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={`${e.path} (${GIT_CODE[e.code]})`}>{e.path}</span>
            </label>
          ))}
        </fieldset>
      )}
      <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        <span className="meta">Mensagem</span>
        <textarea ref={msgRef} aria-label="Mensagem do commit" rows={4} value={msg} onChange={(e) => setMsg(e.target.value)} placeholder="tipo: descrição curta" />
      </label>
      {note && <div className="meta">{note}</div>}
      <div className="row" style={{ gap: 8, justifyContent: "flex-end" }}>
        <button className="btn" disabled={busy || picked.length === 0} onClick={suggest}>Sugerir mensagem</button>
        <button className="btn" onClick={close}>Cancelar</button>
        <button className="btn" disabled={busy || !canCommit(msg, picked)} onClick={() => commit("pr")}>Commitar e abrir PR…</button>
        <button className="btn primary" disabled={busy || !canCommit(msg, picked)} onClick={() => commit()}>Commitar</button>
      </div>
      </div>
    </Modal>
  );
}

function PrDialog({ t }: { t: Target }) {
  const mission = useMissionOf(t);
  const [base, setBase] = useState(mission?.baseBranch ?? DEFAULT_BASE);
  // sem base da missão: a do remoto (origin/HEAD), senão init.defaultBranch, senão main/master
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [note, setNote] = useState<string | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  // base (da missão ou do remoto) e, sem IA configurada, título e descrição locais já preenchidos
  useEffect(() => {
    let live = true;
    (async () => {
      const b = mission?.baseBranch || (await api.gitPrBase(t.wsId, t.missionId).catch(() => base));
      if (!live) return;
      setBase(b);
      const pre = await api.gitPrPrefill(t.wsId, t.missionId, b).catch(() => null);
      if (live && pre) { setTitle((x) => x || pre.title); setBody((x) => x || pre.body); setNote(sourceNote(pre.source, pre.aiError)); }
    })();
    return () => { live = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t.wsId, t.missionId]);

  async function suggest() {
    setBusy(true);
    setError(null);
    try {
      const r = await api.gitPrSuggest(t.wsId, t.missionId, base);
      setTitle(r.title);
      setBody(r.body);
      setNote(sourceNote(r.source, r.aiError));
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  async function send() {
    setConfirm(false);
    setBusy(true);
    setError(null);
    try {
      const u = await api.gitPrOpen(t.wsId, t.missionId, base, title, body);
      setUrl(u);
      useStore.getState().toast("PR aberto", "ok");
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <Modal label="Abrir PR" onClose={close} style={{ width: "min(720px, 94vw)", display: "flex", flexDirection: "column", gap: 10 }}>
        <h2>Abrir PR{mission ? ` da missão ${mission.name}` : ""}</h2>
        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span className="meta">Branch base</span>
          <input aria-label="Branch base" value={base} onChange={(e) => setBase(e.target.value)} disabled={!!url} />
        </label>
        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span className="meta">Título</span>
          <input aria-label="Título do PR" value={title} onChange={(e) => setTitle(e.target.value)} disabled={!!url} />
        </label>
        <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span className="meta">Descrição</span>
          <textarea aria-label="Descrição do PR" rows={6} value={body} onChange={(e) => setBody(e.target.value)} disabled={!!url} />
        </label>
        {note && <div className="meta">{note}</div>}
        {error && <div role="alert" style={{ color: "var(--err)" }}>{error}</div>}
        {url && (
          <div className="row" style={{ gap: 8, justifyContent: "space-between" }}>
            <span className="meta" style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{url}</span>
            <button className="btn primary" onClick={() => openUrl(url).catch(toast)}>Abrir no navegador</button>
          </div>
        )}
        <div className="row" style={{ gap: 8, justifyContent: "flex-end" }}>
          <button className="btn" disabled={busy || !!url} onClick={suggest}>Sugerir título e descrição</button>
          <button className="btn" onClick={close}>{url ? "Fechar" : "Cancelar"}</button>
          {!url && <button className="btn primary" disabled={busy || !title.trim() || !base.trim()} onClick={() => setConfirm(true)}>Abrir PR…</button>}
        </div>
      </Modal>
      {confirm && (
        <Modal label="Confirmar envio" onClose={() => setConfirm(false)} style={{ width: "min(480px, 92vw)", display: "flex", flexDirection: "column", gap: 10 }}>
          <h2>Enviar e abrir o PR?</h2>
          <p>Isso envia a branch para o remoto (origin) e abre um PR contra <strong>{base}</strong>.</p>
          <div className="row" style={{ gap: 8, justifyContent: "flex-end" }}>
            <button className="btn" onClick={() => setConfirm(false)}>Cancelar</button>
            <button className="btn primary" onClick={send}>Enviar e abrir PR</button>
          </div>
        </Modal>
      )}
    </>
  );
}
