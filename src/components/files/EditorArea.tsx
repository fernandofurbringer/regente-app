import Editor, { type OnMount } from "@monaco-editor/react";
import { message } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import FloatMenu from "../FloatMenu";
import { paraFechar, vizinha, type Fechar } from "../../lib/editorTabs";
import { registerEditor, registerEditorInstance } from "../../lib/editorContext";
import { bytesUtf8, type EditorStatus } from "../../lib/editorStatus";
import { askFromEditor, VERB_ORDER, verbLabel } from "../../lib/askAgent";
import Markdown from "react-markdown";
import { MD } from "../../lib/markdown";
import { api, errCode, errMsg } from "../../lib/ipc";
import { monaco, TEMA_ESCURO } from "../../monaco";
import { useStore, type OpenFile } from "../../state/store";
import { openOutside } from "../TerminalPane";
import { arcoIrisIndentacao, margemGit } from "./editorExtras";
import { tipoArquivo } from "../../lib/tipoArquivo";

interface Doc { path: string; text: string; saved: string; mtime: number; preview: boolean; readOnly?: boolean }
type Ed = Parameters<OnMount>[0];
const uriOf = (p: string) => monaco.Uri.file(p).toString();
const modelOf = (p: string) => monaco.editor.getModel(monaco.Uri.parse(uriOf(p)));
const base = (p: string) => p.split(/[\\/]/).pop() ?? p;
/** nome da linguagem para gente ("C#", "TypeScript"), pelo primeiro apelido que o Monaco registra */
const langNome = (id: string) => monaco.languages.getLanguages().find((l) => l.id === id)?.aliases?.[0] ?? id;
/** Windows não diferencia maiúsculas em caminhos */
const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
const under = (p: string, dir: string) => same(p, dir) || p.toLowerCase().startsWith(`${dir.toLowerCase()}\\`);
const fail = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

export default function EditorArea({ wsId }: { wsId: string }) {
  const [docs, setDocs] = useState<Doc[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const theme = useStore((s) => s.settings?.theme);
  const request = useStore((s) => s.openFile[wsId] ?? null);
  const docsRef = useRef(docs);
  docsRef.current = docs;
  const activeRef = useRef(active);
  activeRef.current = active;
  const editorRef = useRef<Ed | null>(null);
  /** extensões do editor (indent-rainbow, margem do Git): o desligar de cada uma */
  const extras = useRef<(() => void)[]>([]);
  useEffect(() => () => extras.current.forEach((f) => f()), []);
  /** linha a revelar quando o modelo do arquivo estiver no editor */
  const lineRef = useRef<{ path: string; line: number; col?: number } | null>(null);
  /** caminhos salvando agora (Ctrl+S duplo não grava duas vezes) */
  const saving = useRef(new Set<string>());
  /** menu aberto: o de uma aba (clique direito) ou a lista "⋯ n" de todos os abertos */
  const [menu, setMenu] = useState<{ kind: "aba"; path: string; at: DOMRect } | { kind: "todas"; at: DOMRect } | null>(null);
  const fecharMenu = useCallback(() => setMenu(null), []);
  const tabsRef = useRef<HTMLDivElement>(null);
  const dirtyKey = docs.filter((d) => d.text !== d.saved).map((d) => d.path).join("\n");
  // variáveis dos prompts salvos ({{arquivo}}, {{selecao}}): o arquivo ativo e a seleção do editor
  useEffect(() => registerEditor(wsId, () => {
    const ed = editorRef.current;
    const sel = ed?.getSelection();
    const model = ed?.getModel();
    return { path: activeRef.current, selection: sel && model && !sel.isEmpty() ? model.getValueInRange(sel) : "" };
  }), [wsId]);

  useEffect(() => registerEditorInstance(wsId, () => editorRef.current), [wsId]);
  /** tamanho em disco do arquivo ativo: só recalcula quando o texto salvo muda */
  const bytesCache = useRef<{ text: string; n: number }>({ text: "", n: 0 });
  /** Publica na barra de status o estado do editor; null sem arquivo no editor (nenhum aberto, prévia do markdown). */
  function publicar() {
    const ed = editorRef.current;
    const model = ed?.getModel();
    const d = docsRef.current.find((x) => x.path === activeRef.current);
    let status: EditorStatus | null = null;
    if (ed && model && d && !d.preview && model.uri.toString() === uriOf(d.path)) {
      if (bytesCache.current.text !== d.saved) bytesCache.current = { text: d.saved, n: bytesUtf8(d.saved) };
      const pos = ed.getPosition();
      const s = ed.getSelection();
      const temSel = !!s && !s.isEmpty();
      const o = model.getOptions();
      status = {
        ln: pos?.lineNumber ?? 1,
        col: pos?.column ?? 1,
        sel: s && temSel ? model.getValueLengthInRange(s) : 0,
        selLinhas: s && temSel ? s.endLineNumber - s.startLineNumber + 1 : 0,
        tabSize: o.tabSize,
        insertSpaces: o.insertSpaces,
        eol: model.getEOL() === "\r\n" ? "CRLF" : "LF",
        lang: langNome(model.getLanguageId()),
        bytes: bytesCache.current.n,
        dirty: d.text !== d.saved,
        readOnly: !!d.readOnly,
      };
    }
    const st = useStore.getState();
    if (JSON.stringify(st.editorStatus[wsId] ?? null) !== JSON.stringify(status)) st.patch({ editorStatus: { ...st.editorStatus, [wsId]: status } });
  }
  // texto, aba ativa, prévia e salvar mudam o que a barra mostra; a comparação acima evita escrever igual
  useEffect(() => publicar());

  function reveal() {
    const ed = editorRef.current;
    const want = lineRef.current;
    if (!ed || !want || ed.getModel()?.uri.toString() !== uriOf(want.path)) return;
    lineRef.current = null;
    ed.revealLineInCenter(want.line);
    ed.setPosition({ lineNumber: want.line, column: want.col ?? 1 });
    ed.focus();
  }

  async function open(req: OpenFile) {
    if (req.text !== undefined) {
      // instantâneo (histórico de painel): sempre o texto novo, somente leitura, sem ir ao disco
      modelOf(req.path)?.dispose();
      const snap = { path: req.path, text: req.text, saved: req.text, mtime: 0, preview: false, readOnly: true };
      setDocs((d) => (d.some((x) => same(x.path, req.path)) ? d.map((x) => (same(x.path, req.path) ? snap : x)) : [...d, snap]));
      setActive(req.path);
      return;
    }
    const existing = docsRef.current.find((d) => same(d.path, req.path));
    const path = existing?.path ?? req.path;
    lineRef.current = req.line ? { path, line: req.line, col: req.col } : null;
    if (existing) {
      setActive(path);
      reveal();
      return;
    }
    try {
      const f = await api.fsRead(path);
      // um modelo que sobrou de quando o arquivo foi fechado teria o texto antigo
      modelOf(path)?.dispose();
      setDocs((d) => (d.some((x) => same(x.path, path)) ? d : [...d, { path, text: f.text, saved: f.text, mtime: f.mtimeMs, preview: false }]));
      setActive(path);
    } catch (e) {
      const c = errCode(e);
      if (c !== "outside_workspace" && c !== "binary" && c !== "encoding" && c !== "too_big") return fail(e);
      // fora dos workspaces (AppData, ~/.claude): texto abre aqui, só leitura; binário ou grande vai para o sistema
      if (c === "outside_workspace") {
        const ext = await api.fsReadExterno(path).catch(() => null);
        if (ext) {
          modelOf(path)?.dispose();
          const doc = { path, text: ext.text, saved: ext.text, mtime: ext.mtimeMs, preview: false, readOnly: true };
          setDocs((d) => (d.some((x) => same(x.path, path)) ? d : [...d, doc]));
          setActive(path);
          return;
        }
      }
      await openOutside(path).catch(fail);
    }
  }

  /** Relê do disco. Sem `force`, só docs limpos e só se o arquivo mudou. */
  async function reload(path: string, force = false) {
    const d = docsRef.current.find((x) => x.path === path);
    if (!d || d.readOnly || (!force && d.text !== d.saved)) return;
    try {
      const f = await api.fsRead(path);
      const cur = docsRef.current.find((x) => x.path === path);
      if (!cur || (!force && (f.mtimeMs === cur.mtime || cur.text !== cur.saved))) return;
      setDocs((ds) => ds.map((x) => (x.path === path ? { ...x, text: f.text, saved: f.text, mtime: f.mtimeMs } : x)));
      const m = modelOf(path);
      if (m && m.getValue() !== f.text) {
        // mantém rolagem e cursor do arquivo aberto
        const ed = editorRef.current;
        const view = ed?.getModel() === m ? ed.saveViewState() : null;
        m.setValue(f.text);
        if (view) ed!.restoreViewState(view);
      }
    } catch {
      /* apagado ou ilegível: fica como está */
    }
  }

  async function save(path: string, force = false): Promise<boolean> {
    const d = docsRef.current.find((x) => x.path === path);
    if (!d || d.readOnly || saving.current.has(path)) return false;
    saving.current.add(path);
    try {
      const mtime = await api.fsWrite(path, d.text, force ? null : d.mtime);
      setDocs((ds) => ds.map((x) => (x.path === path ? { ...x, saved: d.text, mtime } : x)));
      window.dispatchEvent(new Event("regente:arquivo-salvo"));
      return true;
    } catch (e) {
      if (errCode(e) !== "changed_on_disk") {
        fail(e);
        return false;
      }
      const r = await message(`${base(path)} mudou no disco desde que foi aberto.`, {
        title: "Conflito", kind: "warning", buttons: { yes: "Sobrescrever", no: "Recarregar do disco", cancel: "Cancelar" },
      });
      if (r === "Sobrescrever" || r === "Yes") {
        saving.current.delete(path);
        return save(path, true);
      }
      if (r === "Recarregar do disco" || r === "No") await reload(path, true);
      return false;
    } finally {
      saving.current.delete(path);
    }
  }

  /** Fecha a aba; false se o usuário cancelou ou o salvar falhou (a aba fica). */
  async function closeDoc(path: string): Promise<boolean> {
    const d = docsRef.current.find((x) => x.path === path);
    if (d && d.text !== d.saved) {
      const r = await message(`${base(path)} tem alterações não salvas.`, {
        title: "Fechar", kind: "warning", buttons: { yes: "Salvar", no: "Descartar", cancel: "Cancelar" },
      });
      if (r === "Salvar" || r === "Yes") {
        if (!(await save(path))) return false;
      } else if (r !== "Descartar" && r !== "No") return false;
    }
    modelOf(path)?.dispose();
    const rest = docsRef.current.filter((x) => x.path !== path);
    // fechamentos seguidos (Fechar outros/todos) leem a lista nova antes do próximo render
    docsRef.current = rest;
    setDocs(rest);
    if (activeRef.current === path) {
      const next = rest[rest.length - 1]?.path ?? null;
      activeRef.current = next;
      setActive(next);
    }
    return true;
  }

  async function fecharVarios(kind: Fechar, alvo: string) {
    setMenu(null);
    const docsAgora = docsRef.current.map((d) => ({ path: d.path, dirty: d.text !== d.saved }));
    // um Cancelar para tudo, como no VS Code (não pergunta de novo para cada sujo que sobrou)
    for (const p of paraFechar(kind, docsAgora, alvo)) if (!(await closeDoc(p))) break;
  }
  /** Ctrl+PageDown (+1) / Ctrl+PageUp (-1), como no VS Code */
  const trocar = (passo: 1 | -1) => {
    const p = vizinha(docsRef.current.map((d) => d.path), activeRef.current, passo);
    if (p) setActive(p);
  };

  useEffect(() => {
    if (!request) return;
    open(request);
    useStore.getState().patch({ openFile: { ...useStore.getState().openFile, [wsId]: null } });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [request, wsId]);

  // a aba ativa sempre à vista: abrir pela árvore ou pelo terminal, Ctrl+PageDown, a lista "⋯ n" e a janela que encolhe
  // corpo em bloco de propósito: o WebView2 novo devolve uma Promise do scrollIntoView, e o React chamaria esse retorno de
  // efeito como limpeza ("is not a function")
  const mostrarAtiva = () => {
    tabsRef.current?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
  };
  useEffect(mostrarAtiva, [active, docs.length]);
  useEffect(() => {
    const el = tabsRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => mostrarAtiva());
    ro.observe(el);
    return () => ro.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // aba ativada: relê se mudou no disco; e revela a linha pedida
  useEffect(() => {
    reveal();
    if (active) reload(active);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  // publica os sujos (fechar aba/janela, integrar/arquivar missão perguntam antes)
  useEffect(() => {
    const st = useStore.getState();
    st.patch({ dirty: { ...st.dirty, [wsId]: dirtyKey ? dirtyKey.split("\n") : [] } });
  }, [dirtyKey, wsId]);

  useEffect(() => {
    const onFocus = () => docsRef.current.forEach((d) => reload(d.path));
    // a árvore renomeou/moveu (to) ou apagou (to = null) um arquivo ou pasta
    const onMoved = (e: Event) => {
      const { from, to } = (e as CustomEvent<{ from: string; to: string | null }>).detail;
      const hit = docsRef.current.filter((d) => under(d.path, from));
      if (!hit.length) return;
      hit.forEach((d) => modelOf(d.path)?.dispose());
      const moved = (p: string) => (to === null ? null : to + p.slice(from.length));
      const next = docsRef.current.flatMap((d) => (!under(d.path, from) ? [d] : to === null ? [] : [{ ...d, path: moved(d.path)! }]));
      setDocs(next);
      const a = activeRef.current;
      if (a && under(a, from)) setActive(moved(a) ?? next[next.length - 1]?.path ?? null);
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener("regente:path-moved", onMoved);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("regente:path-moved", onMoved);
      const st = useStore.getState();
      const dirty = { ...st.dirty };
      delete dirty[wsId];
      st.patch({ dirty, editorStatus: { ...st.editorStatus, [wsId]: null } });
      docsRef.current.forEach((d) => modelOf(d.path)?.dispose());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsId]);

  const onMount: OnMount = (editor) => {
    editorRef.current = editor;
    extras.current.forEach((f) => f());
    // cursor, seleção, indentação, linguagem e troca de arquivo mudam a barra de status sem renderizar o EditorArea
    const ouvintes = [editor.onDidChangeCursorSelection(publicar), editor.onDidChangeModelOptions(publicar), editor.onDidChangeModelLanguage(publicar), editor.onDidChangeModel(publicar)];
    extras.current = [arcoIrisIndentacao(editor), margemGit(editor, () => activeRef.current), ...ouvintes.map((o) => () => o.dispose())];
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => { if (activeRef.current) save(activeRef.current); });
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.PageDown, () => trocar(1));
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.PageUp, () => trocar(-1));
    // menu de contexto do editor: pedidos ao agente sobre o arquivo (ou a seleção)
    VERB_ORDER.forEach((v, i) => editor.addAction({
      id: `regente.pedir.${v}`, label: `Pedir ao agente: ${verbLabel(v)}`, contextMenuGroupId: "navigation", contextMenuOrder: 1.5 + i / 10,
      run: () => { const err = askFromEditor(v); if (err) useStore.getState().toast(err, "erro"); },
    }));
    reveal();
    publicar();
  };
  const doc = docs.find((d) => d.path === active);

  return (
    <div className="editor-area"
      onKeyDown={(e) => {
        // com o foco fora do Monaco (nas abas): Ctrl+PageUp/PageDown também trocam de aba; dentro, o Monaco já tratou
        if (e.ctrlKey && (e.key === "PageDown" || e.key === "PageUp") && !e.defaultPrevented) {
          e.preventDefault();
          trocar(e.key === "PageDown" ? 1 : -1);
        }
      }}>
      <div className="editor-tabbar">
        <div ref={tabsRef} className="editor-tabs" role="tablist" aria-label="Arquivos abertos"
          onWheel={(e) => { if (e.deltaY && !e.deltaX) e.currentTarget.scrollLeft += e.deltaY; }}>
          {docs.map((d) => {
            const nome = base(d.path);
            const tipo = tipoArquivo(nome);
            return (
              <div key={d.path} className={`editor-tabwrap ${d.path === active ? "active" : ""}`}
                // botão do meio fecha; o mousedown dele precisa do preventDefault, senão o WebView2 liga a rolagem automática
                onMouseDown={(e) => { if (e.button === 1) e.preventDefault(); }}
                onAuxClick={(e) => { if (e.button === 1) { e.preventDefault(); void closeDoc(d.path); } }}
                onContextMenu={(e) => { e.preventDefault(); setMenu({ kind: "aba", path: d.path, at: e.currentTarget.getBoundingClientRect() }); }}>
                <button role="tab" aria-selected={d.path === active} title={d.path}
                  className={`editor-tab ${d.path === active ? "active" : ""} ${d.text !== d.saved ? "dirty" : ""}`} onClick={() => setActive(d.path)}>
                  <span className={`ic ic-${tipo.classe}`}>{tipo.rotulo}</span><span className="editor-tab-name">{nome}</span>
                </button>
                <button className="icon-btn" aria-label={`Fechar ${nome}`} title="Fechar (o botão do meio do mouse também fecha)" onClick={() => closeDoc(d.path)}>✕</button>
              </div>
            );
          })}
        </div>
        {doc && /\.mdx?$/i.test(doc.path) && (
          <button className="tb-btn" style={{ alignSelf: "center", marginLeft: 6 }} aria-pressed={doc.preview}
            onClick={() => setDocs((ds) => ds.map((x) => (x.path === doc.path ? { ...x, preview: !x.preview } : x)))}>{doc.preview ? "Editar" : "Visualizar"}</button>
        )}
        {docs.length > 0 && (
          <button className="editor-tabs-all" data-menu-trigger="" aria-haspopup="menu" aria-expanded={menu?.kind === "todas"} title="Todos os arquivos abertos"
            onClick={(e) => setMenu(menu?.kind === "todas" ? null : { kind: "todas", at: e.currentTarget.getBoundingClientRect() })}>⋯ {docs.length}</button>
        )}
      </div>
      {menu?.kind === "todas" && (
        <FloatMenu at={menu.at} onClose={fecharMenu} label="Arquivos abertos" className="editor-menu">
          {docs.map((d) => (
            <button key={d.path} className="menu-item" role="menuitem" aria-current={d.path === active} title={d.path}
              onClick={() => { setMenu(null); setActive(d.path); }}>
              {d.text !== d.saved ? "● " : ""}{base(d.path)}
            </button>
          ))}
        </FloatMenu>
      )}
      {menu?.kind === "aba" && (
        <FloatMenu at={menu.at} onClose={fecharMenu} label={`Ações de ${base(menu.path)}`} className="editor-menu">
          <button className="menu-item" role="menuitem" onClick={() => { const p = menu.path; setMenu(null); void closeDoc(p); }}>Fechar</button>
          <button className="menu-item" role="menuitem" disabled={docs.length < 2} onClick={() => void fecharVarios("outros", menu.path)}>Fechar outros</button>
          <button className="menu-item" role="menuitem" onClick={() => void fecharVarios("salvos", menu.path)}>Fechar salvos</button>
          <button className="menu-item" role="menuitem" onClick={() => void fecharVarios("todos", menu.path)}>Fechar todos</button>
          <div className="menu-sep" role="separator" />
          <button className="menu-item" role="menuitem"
            onClick={() => { const p = menu.path; setMenu(null); navigator.clipboard.writeText(p).then(() => useStore.getState().toast("Caminho copiado", "ok"), fail); }}>Copiar caminho</button>
          <button className="menu-item" role="menuitem"
            onClick={() => { const p = menu.path; setMenu(null); window.dispatchEvent(new CustomEvent("regente:revelar", { detail: p })); }}>Revelar na árvore</button>
        </FloatMenu>
      )}
      {!doc ? (
        <div className="empty">Abra um arquivo na árvore ou clique num caminho no terminal.</div>
      ) : doc.preview ? (
        <div className="md-preview"><Markdown components={MD}>{doc.text}</Markdown></div>
      ) : (
        <Editor path={uriOf(doc.path)} defaultValue={doc.text} theme={theme === "light" ? "vs" : theme === "contraste" ? "hc-black" : TEMA_ESCURO} onMount={onMount}
          onChange={(val) => setDocs((ds) => ds.map((x) => (x.path === doc.path ? { ...x, text: val ?? "" } : x)))}
          options={{ fontSize: 13, minimap: { enabled: false }, automaticLayout: true, readOnly: !!doc.readOnly,
            bracketPairColorization: { enabled: true }, guides: { bracketPairs: "active", indentation: true, highlightActiveIndentation: true }, lineDecorationsWidth: 12, renderLineHighlight: "all" }} />
      )}
    </div>
  );
}
