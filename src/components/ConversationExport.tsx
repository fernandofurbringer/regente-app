import { save } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { api, errMsg } from "../lib/ipc";
import { useStore } from "../state/store";
import Modal from "./Modal";
import { terms } from "./TerminalPane";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
/** A escolha "com ferramentas completas" fica lembrada entre aberturas (só nesta janela). */
const FULL_KEY = "regente.copiarConversa.ferramentas";
const readFull = () => { try { return localStorage.getItem(FULL_KEY) === "1"; } catch { return false; } };
const writeFull = (v: boolean) => { try { localStorage.setItem(FULL_KEY, v ? "1" : "0"); } catch { /* sem armazenamento: vale nesta abertura */ } };

/** Conversa do painel como Markdown: copiar ou salvar como .md. Aberta pelo menu ⋯ do painel e pelo atalho Ctrl+Shift+C.
 *  O foco vai para "Copiar" quando a prévia fica pronta: Enter copia. */
export default function ConversationExport() {
  const paneId = useStore((s) => s.exportPane);
  const pane = useStore((s) => (paneId ? s.panes[paneId] : undefined));
  const [full, setFullState] = useState(readFull);
  const [md, setMd] = useState<string | null>(null);
  // recarregando (ex.: ao marcar "com ferramentas"): a prévia antiga ainda aparece, mas não se copia
  const [loading, setLoading] = useState(false);
  const copyBtn = useRef<HTMLButtonElement>(null);
  const focused = useRef(false);
  const close = () => useStore.getState().patch({ exportPane: null });
  const setFull = (v: boolean) => { setFullState(v); writeFull(v); };

  // outra conversa: a prévia antiga some e o foco inicial volta a valer
  useEffect(() => {
    setMd(null);
    focused.current = false;
  }, [paneId]);

  useEffect(() => {
    if (!paneId) return;
    let stale = false;
    setLoading(true);
    // o texto do terminal vai junto: é o que vale para outros CLIs (o backend o redige)
    api.paneTranscript(paneId, full, terms.get(paneId)?.text() ?? null)
      .then((t) => { if (!stale) setMd(t); })
      .catch((e) => { if (!stale) { toast(e); close(); } })
      .finally(() => { if (!stale) setLoading(false); });
    return () => { stale = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paneId, full]);

  useEffect(() => {
    if (md !== null && !focused.current) {
      focused.current = true;
      copyBtn.current?.focus();
    }
  }, [md]);

  if (!paneId || !pane) return null;
  const name = pane.name.replace(/[\/:*?"<>|]/g, "_");

  async function copy() {
    if (md === null) return;
    try {
      await navigator.clipboard.writeText(md);
      useStore.getState().toast("Conversa copiada", "ok");
      close();
    } catch (e) {
      toast(e);
    }
  }

  async function saveMd() {
    if (md === null) return;
    try {
      const path = await save({ defaultPath: `${name}.md`, filters: [{ name: "Markdown", extensions: ["md"] }] });
      if (!path) return;
      await api.exportMarkdown(path, md);
      useStore.getState().toast("Conversa salva", "ok");
      close();
    } catch (e) {
      toast(e);
    }
  }

  return (
    <Modal label="Copiar conversa" autoFocus={false} onClose={close} style={{ width: "min(720px, 92vw)", display: "flex", flexDirection: "column", gap: 10 }}>
      <h2>Copiar conversa</h2>
      <label className="row" style={{ gap: 6 }}>
        <input type="checkbox" checked={full} onChange={(e) => setFull(e.target.checked)} /> com ferramentas completas (entrada e saída de cada chamada)
      </label>
      <pre aria-label="Prévia do Markdown" style={{ maxHeight: "45vh", overflow: "auto", whiteSpace: "pre-wrap", fontFamily: "var(--mono)", fontSize: 12, margin: 0 }}>
        {md ?? "Montando a conversa…"}
      </pre>
      <div className="actions">
        <button className="btn" onClick={close}>Cancelar</button>
        <button className="btn" disabled={md === null} onClick={() => void saveMd()}>Salvar como .md</button>
        <button ref={copyBtn} className="btn primary" disabled={md === null || loading} onClick={() => void copy()}>Copiar</button>
      </div>
    </Modal>
  );
}
