import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errMsg, type PaneRow, type Workspace } from "../lib/ipc";
import { editorContext } from "../lib/editorContext";
import { expandMentions, TERMINAL_LINES, type MentionEnv } from "../lib/mentions";
import { promptVarsUsed, type PromptValues } from "../lib/promptVars";
import { micMessage, VoiceRecorder } from "../lib/recorder";
import { MENTION_EVENT, takeMentions } from "../lib/composerMentions";
import { sendToAgent } from "../lib/sendToAgent";
import { useStore, wsOf } from "../state/store";
import { isInlineSubagent } from "../lib/activity";
import { listFiles } from "./CommandPalette";
import { terms } from "./TerminalPane";
import "./Composer.css";

const HIST_MAX = 50;
/** Mensagem já expandida (com as menções) não passa disto: o texto fica no compositor para você cortar. */
const MSG_MAX = 48 * 1024;
const toast = (text: string) => useStore.getState().toast(text, "erro");

/** Painel do workspace pela referência: o número na ordem da grade (1, 2…) ou o nome (espaços viram _). */
function findPane(ref: string, wsId: string): PaneRow | null {
  const st = useStore.getState();
  // subagente inline mora na faixa do painel pai: não conta na numeração
  const panes = Object.values(st.panes).filter((p) => p.workspaceId === wsId && !isInlineSubagent(p, st.settings?.subagentPanes ?? false)).sort((a, b) => a.ord - b.ord);
  const n = Number(ref);
  if (Number.isInteger(n) && n >= 1 && n <= panes.length) return panes[n - 1];
  const norm = (s: string) => s.replace(/\s+/g, "_").toLowerCase();
  return panes.find((p) => norm(p.name) === norm(ref)) ?? null;
}

/** Pasta de onde saem os arquivos das menções: a do painel; sem ela, a do workspace. */
const baseOf = (pane: PaneRow, ws: Workspace | null | undefined): string => pane.cwd || ws?.path || "";

/** Ambiente das menções: o que o compositor sabe do workspace, do editor e dos terminais. */
function envFor(pane: PaneRow, ws: Workspace | null): MentionEnv {
  const wsId = pane.workspaceId;
  const root = baseOf(pane, ws).replace(/[\\/]+$/, "");
  return {
    readFile: (p) => api.fsRead(`${root}/${p}`).then((f) => f.text).catch(() => null),
    selection: () => editorContext(wsId).selection,
    diff: () => api.diffText(wsId, pane.missionId),
    lastError: (ref) => {
      const panes = ref ? [findPane(ref, wsId)].filter((p): p is PaneRow => !!p) : Object.values(useStore.getState().panes).filter((p) => p.workspaceId === wsId).sort((a, b) => a.ord - b.ord);
      if (ref && !panes.length) return null;
      const errors = useStore.getState().paneErrors;
      for (let i = panes.length - 1; i >= 0; i--) {
        const blocks = errors[panes[i].id] ?? [];
        if (blocks.length) return blocks[blocks.length - 1].lines.join("\n");
      }
      return null;
    },
    terminalTail: (ref) => {
      const target = findPane(ref, wsId);
      if (!target) return null;
      return (terms.get(target.id)?.text() ?? "").split("\n").filter((l) => l.trim()).slice(-TERMINAL_LINES).join("\n");
    },
    prompt: async (name) => {
      const list = await api.promptsList().catch(() => []);
      return list.find((p) => p.name === name)?.text ?? null;
    },
    promptValues: async (text) => {
      const used = promptVarsUsed(text);
      const ctx = editorContext(wsId);
      const values: PromptValues = {};
      if (used.includes("clipboard")) values.clipboard = await navigator.clipboard.readText().catch(() => "");
      if (used.includes("arquivo")) values.arquivo = ctx.path ?? "";
      if (used.includes("selecao")) values.selecao = ctx.selection;
      if (used.includes("workspace")) values.workspace = ws?.path ?? "";
      if (used.includes("branch")) values.branch = await api.workspaceBranch(wsId).catch(() => "");
      return values;
    },
  };
}

/** Compositor do painel de IA: caixa multilinha. Ctrl+Enter envia (com Enter); Enter quebra linha. As menções expandem antes. */
export default function Composer({ pane }: { pane: PaneRow }) {
  const hist = useStore((s) => s.composerHist[pane.id]) ?? [];
  const [text, setText] = useState("");
  const [idx, setIdx] = useState<number | null>(null); // posição no histórico; null = texto novo
  const [draft, setDraft] = useState<string | null>(null); // o rascunho não enviado, guardado ao subir no histórico
  const [busy, setBusy] = useState(false);
  const [mic, setMic] = useState<"parado" | "gravando" | "transcrevendo">("parado");
  const [files, setFiles] = useState<{ label: string; path: string }[] | null>(null);
  const [filesCut, setFilesCut] = useState(false);
  const [filesFilter, setFilesFilter] = useState("");
  const [withContent, setWithContent] = useState(false);
  const recorder = useRef<VoiceRecorder | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);

  // foco no texto ao abrir; menções pedidas pelo arquivo (mesmo antes de abrir) entram aqui; o gravador do ditado para ao sair
  useEffect(() => {
    const flush = () => {
      const pending = takeMentions(pane.id);
      if (!pending.length) return;
      setText((t) => pending.reduce((acc, s) => (acc && !acc.endsWith(" ") && !acc.endsWith("\n") ? `${acc} ${s}` : `${acc}${s}`), t));
    };
    const onMention = (e: Event) => {
      if ((e as CustomEvent<{ paneId: string }>).detail.paneId === pane.id) flush();
    };
    flush();
    window.addEventListener(MENTION_EVENT, onMention);
    area.current?.focus();
    return () => {
      window.removeEventListener(MENTION_EVENT, onMention);
      recorder.current?.cancel();
      recorder.current = null;
    };
  }, [pane.id]);

  function close() {
    recorder.current?.cancel();
    recorder.current = null;
    setMic("parado");
    useStore.getState().patch({ composerOpen: { ...useStore.getState().composerOpen, [pane.id]: false } });
  }

  async function send() {
    const raw = text.trim();
    if (!raw || busy) return;
    setBusy(true);
    try {
      const out = await expandMentions(raw, envFor(pane, wsOf(pane.workspaceId)));
      if (new TextEncoder().encode(out).length > MSG_MAX) {
        toast("A mensagem, com as menções expandidas, passa de 48 KB. Corte o texto ou use @arquivo: (só o caminho).");
        return;
      }
      await sendToAgent(pane.id, out, true);
      const st = useStore.getState();
      st.patch({ composerHist: { ...st.composerHist, [pane.id]: [...(st.composerHist[pane.id] ?? []), raw].slice(-HIST_MAX) } });
      setText("");
      setIdx(null);
      setDraft(null);
    } catch (e) {
      toast(errMsg(e));
    } finally {
      setBusy(false);
      area.current?.focus();
    }
  }

  function onKey(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && e.ctrlKey && e.shiftKey) {
      // o atalho do compositor fecha: o foco volta ao terminal
      e.preventDefault();
      terms.get(pane.id)?.focus();
      return;
    }
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void send();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
      terms.get(pane.id)?.focus();
      return;
    }
    const ta = e.currentTarget;
    const onFirst = !ta.value.slice(0, ta.selectionStart).includes("\n");
    const onLast = !ta.value.slice(ta.selectionEnd).includes("\n");
    if (e.key === "ArrowUp" && onFirst && hist.length) {
      e.preventDefault();
      if (idx === null) setDraft(text);
      const i = idx === null ? hist.length - 1 : Math.max(0, idx - 1);
      setIdx(i);
      setText(hist[i]);
    } else if (e.key === "ArrowDown" && onLast && idx !== null) {
      e.preventDefault();
      if (idx >= hist.length - 1) {
        setIdx(null);
        setText(draft ?? "");
        setDraft(null);
      } else {
        setIdx(idx + 1);
        setText(hist[idx + 1]);
      }
    }
  }

  function insert(s: string) {
    setText((t) => (t && !t.endsWith(" ") && !t.endsWith("\n") ? `${t} ${s}` : `${t}${s}`));
    area.current?.focus();
  }

  async function openFiles() {
    if (files !== null) return setFiles(null);
    const base = baseOf(pane, wsOf(pane.workspaceId));
    if (!base) return;
    setFiles([]);
    setFilesFilter("");
    setFilesCut(false);
    const r = await listFiles(base).catch(() => ({ files: [], truncated: false }));
    setFiles(r.files);
    setFilesCut(r.truncated);
  }

  // ditado: o mesmo gravador do botão de voz; a transcrição entra no texto (sem Enter)
  async function toggleMic() {
    const r = recorder.current;
    if (!r) {
      try {
        const next = new VoiceRecorder();
        await next.start();
        recorder.current = next;
        setMic("gravando");
      } catch (e) {
        toast(micMessage(e));
      }
      return;
    }
    recorder.current = null;
    setMic("transcrevendo");
    try {
      const { blob, mime } = await r.stop();
      const said = (await api.voiceTranscribe(blob, mime, "pane")).replace(/\s*\n\s*/g, " ").trim();
      if (said) insert(said);
    } catch (e) {
      toast(errMsg(e));
    } finally {
      setMic("parado");
    }
  }

  const shownFiles = (files ?? []).filter((f) => !filesFilter || f.label.toLowerCase().includes(filesFilter.toLowerCase())).slice(0, 30);

  return (
    <div className="composer" role="region" aria-label={`Compositor de ${pane.name}`}>
      <textarea ref={area} rows={4} value={text}
        placeholder="Escreva para o agente. Ctrl+Enter envia. Menções: @arquivo:caminho, @seleção, @diff, @erro, @terminal:painel, @prompt:nome"
        aria-label="Mensagem para o agente" aria-busy={busy} readOnly={busy}
        onChange={(e) => { setText(e.target.value); setIdx(null); setDraft(null); }} onKeyDown={onKey} />
      <div className="composer-bar">
        <button type="button" className="btn" aria-expanded={files !== null} onClick={() => void openFiles()}>@ Arquivo…</button>
        <button type="button" className="btn" onClick={() => insert("@diff")} title="Diff de trabalho do workspace (até 30 KB)">@diff</button>
        <button type="button" className="btn" onClick={() => insert("@seleção")} title="Seleção do editor">@seleção</button>
        <button type="button" className="btn" onClick={() => insert("@erro")} title="Último erro detectado nos terminais">@erro</button>
        <button type="button" className="btn" aria-pressed={mic === "gravando"} disabled={mic === "transcrevendo"} onClick={() => void toggleMic()}>
          {mic === "gravando" ? "Parar ditado" : mic === "transcrevendo" ? "Transcrevendo…" : "Ditar"}
        </button>
        <span className="spacer" />
        <button type="button" className="btn primary" disabled={busy || !text.trim()} onClick={() => void send()} title="Ctrl+Enter">Enviar</button>
        <button type="button" className="btn" onClick={close} aria-label="Fechar compositor">✕</button>
      </div>
      {files !== null && (
        <div className="composer-files" role="listbox" aria-label="Arquivos do workspace">
          <div className="composer-files-head">
            <input type="search" aria-label="Filtrar arquivos" placeholder="Filtrar arquivos" value={filesFilter} onChange={(e) => setFilesFilter(e.target.value)} />
            <label className="row" style={{ gap: 4 }}><input type="checkbox" checked={withContent} onChange={(e) => setWithContent(e.target.checked)} /> colar o conteúdo (até 8 KB)</label>
          </div>
          {files.length === 0 && <span className="meta">Lendo os arquivos do workspace…</span>}
          {filesCut && <span className="meta">Lista parcial (20 mil arquivos)</span>}
          {shownFiles.map((f) => (
            <button key={f.path} type="button" className="menu-item" role="option" aria-selected={false}
              onClick={() => { insert(`@arquivo${withContent ? "+" : ""}:${f.label}`); setFiles(null); }}>{f.label}</button>
          ))}
        </div>
      )}
    </div>
  );
}
