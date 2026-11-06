import { useVirtualizer } from "@tanstack/react-virtual";
import { ask } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { api, errMsg, type FsEntry, type GitMarca } from "../../lib/ipc";
import { tipoArquivo } from "../../lib/tipoArquivo";
import { useStore, wsOf } from "../../state/store";
import { askAboutFile, parentOf, relPath, VERB_ORDER, verbLabel, type AgentVerb } from "../../lib/askAgent";
import { queueMention } from "../../lib/composerMentions";
import { readLastTarget } from "../../lib/sendToAgent";
import { startPointerDrag, type DragPayload } from "../../lib/pointerDrag";
import { GIT_CHANGED } from "../../lib/gitflow";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";

interface Node { e: FsEntry; level: number }
type Editing = { mode: "novo-arquivo" | "nova-pasta" | "renomear"; dir: string; path?: string; value: string };
const join = (dir: string, name: string) => `${dir.replace(/[\\/]+$/, "")}\\${name}`;
const under = (p: string, dir: string) => p === dir || p.startsWith(`${dir}\\`) || p.startsWith(`${dir}/`);
const EDIT_LABEL: Record<Editing["mode"], string> = { "novo-arquivo": "Nome do novo arquivo", "nova-pasta": "Nome da nova pasta", renomear: "Novo nome" };
/** avisa o editor: arquivo/pasta renomeado ou movido (to) ou apagado (to = null) */
const moved = (from: string, to: string | null) => window.dispatchEvent(new CustomEvent("regente:path-moved", { detail: { from, to } }));

/** `outraRaiz`: a outra árvore do workspace (projeto × worktree da missão); "Revelar" de um arquivo de lá é dela. */
export default function FileTree({ root, onOpen, outraRaiz }: { root: string; onOpen(path: string): void; outraRaiz?: string | null }) {
  /** mostra também `.git`, `node_modules`, `target`… */
  const [ignorados, setIgnorados] = useState(false);
  const [children, setChildren] = useState<Record<string, FsEntry[]>>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [sel, setSel] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; e: FsEntry } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const toast = useStore((s) => s.toast);
  const box = useRef<HTMLDivElement>(null);
  // clique fora do menu fecha; o clique dentro chega aos botões
  useEffect(() => {
    if (!menu) return;
    const onDown = (ev: MouseEvent) => { if (!menuRef.current?.contains(ev.target as globalThis.Node)) setMenu(null); };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [menu]);

  function askFile(verb: AgentVerb, e: FsEntry) {
    const st = useStore.getState();
    const ws = wsOf(st.active);
    if (!ws) return;
    st.patch({ sendTo: askAboutFile(verb, { workspaceId: ws.id, missionId: st.missionSel[ws.id] ?? null, path: e.path, root, selection: "" }) });
  }

  // a menção vai para o último painel de IA usado; o compositor abre se estiver fechado
  function mentionFile(e: FsEntry) {
    const st = useStore.getState();
    const pane = st.panes[readLastTarget() ?? ""];
    if (!pane || pane.kind !== "agent") return st.toast("Nenhum painel de IA usado recentemente: abra um e tente de novo.", "info");
    if (!st.composerOpen[pane.id]) st.patch({ composerOpen: { ...st.composerOpen, [pane.id]: true } });
    queueMention(pane.id, `@arquivo:${relPath(e.path, root)}`);
  }
  /** Esc no campo: o blur que vem depois não salva */
  const cancelEdit = useRef(false);
  const uid = useId();
  // soltar: zonas `file-move` (a raiz e as linhas) levam o destino em data-dir
  const moveRef = useRef<(src: string, dir: string) => Promise<void>>(async () => {});
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const onDrop = (e: Event) => {
      const { kind, data } = (e as CustomEvent<DragPayload>).detail;
      const dir = (e.target as HTMLElement).dataset.dir;
      if (kind === "file" && dir) moveRef.current(data, dir);
    };
    el.addEventListener("regente-drop", onDrop);
    return () => el.removeEventListener("regente-drop", onDrop);
  }, []);

  /** marcas do Git por caminho em minúsculas (Windows não diferencia), relidas a cada 3 s com a árvore visível */
  const [marcas, setMarcas] = useState<Map<string, GitMarca["code"]>>(new Map());
  /** pastas novas inteiras (`?? dir/`): o que está dentro delas é novo */
  const [pastasNovas, setPastasNovas] = useState<Set<string>>(new Set());
  const assinatura = useRef("");
  useEffect(() => {
    let vivo = true;
    // uma rodada por vez: um `git status` lento não pode empilhar chamadas (prendia os comandos de arquivo)
    let rodando = false;
    const carregar = (forcar = false) => {
      if (rodando || (!forcar && !document.hasFocus()) || !box.current?.offsetParent) return;
      rodando = true;
      api.fsGitMarcas(root).then((l) => {
        const sig = l.map((m) => `${m.code}${m.dir ? "/" : ""}${m.path}`).join("\n");
        if (!vivo || sig === assinatura.current) return;
        const primeira = assinatura.current === "";
        assinatura.current = sig;
        setMarcas(new Map(l.map((m) => [m.path.toLowerCase(), m.code])));
        setPastasNovas(new Set(l.filter((m) => m.dir).map((m) => m.path.toLowerCase())));
        // o status mudou (agente criou, apagou ou mexeu em algo): as pastas abertas relêem
        if (!primeira) void recarregarRef.current(false);
      }).catch(() => {}).finally(() => { rodando = false; });
    };
    carregar(true);
    const t = window.setInterval(() => carregar(), 3000);
    const agora = () => carregar(true);
    const evs = ["focus", "regente:arquivo-salvo", "regente:path-moved", "regente:fs-changed", GIT_CHANGED];
    evs.forEach((e) => window.addEventListener(e, agora));
    return () => { vivo = false; window.clearInterval(t); evs.forEach((e) => window.removeEventListener(e, agora)); };
  }, [root]);
  /** pastas com algum arquivo alterado dentro (ganham ponto e cor), e as pastas novas inteiras */
  const pastasMudadas = useMemo(() => {
    const s = new Set<string>(pastasNovas);
    const raiz = root.toLowerCase();
    for (const p of marcas.keys()) {
      for (let d = parentOf(p); d.length > raiz.length && d.startsWith(raiz); d = parentOf(d)) s.add(d);
    }
    return s;
  }, [marcas, pastasNovas, root]);
  /** marca de um arquivo: a dele, ou "?" se estiver dentro de uma pasta nova inteira */
  const marcaDe = (path: string): GitMarca["code"] | undefined => {
    const low = path.toLowerCase();
    const m = marcas.get(low);
    if (m || !pastasNovas.size) return m;
    const raiz = root.toLowerCase();
    for (let d = parentOf(low); d.length > raiz.length; d = parentOf(d)) if (pastasNovas.has(d)) return "?";
    return undefined;
  };

  const load = useCallback(async (dir: string) => {
    try {
      const list = await api.fsList(dir, ignorados);
      setChildren((c) => ({ ...c, [dir]: list }));
      // pasta apagada/renomeada sai do expandido junto com as subpastas; recriada começa recolhida
      const dirs = new Set(list.filter((x) => x.isDir).map((x) => x.path));
      setExpanded((ex) => {
        const gone = [...ex].filter((p) => parentOf(p) === dir && !dirs.has(p));
        return gone.length ? new Set([...ex].filter((p) => !gone.some((g) => under(p, g)))) : ex;
      });
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }, [toast, ignorados]);

  useEffect(() => { load(root); }, [root, load]);

  /** relê a raiz e as pastas abertas de uma vez (um render só); pasta que sumiu sai calada, com as subpastas */
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const [atualizando, setAtualizando] = useState(false);
  const recarregar = useCallback(async (mostrar: boolean) => {
    const dirs = [root, ...expandedRef.current];
    if (mostrar) setAtualizando(true);
    const lidas = await Promise.all(dirs.map((d) => api.fsList(d, ignorados).then((l) => [d, l] as const).catch(() => [d, null] as const)));
    if (mostrar) setAtualizando(false);
    const sumiram = lidas.filter(([d, l]) => !l && d !== root).map(([d]) => d);
    setChildren((c) => {
      const n = { ...c };
      for (const [d, l] of lidas) if (l) n[d] = l;
      for (const d of sumiram) delete n[d];
      return n;
    });
    if (sumiram.length) setExpanded((ex) => new Set([...ex].filter((p) => !sumiram.some((g) => under(p, g)))));
  }, [root, ignorados]);
  // ligou ou desligou os ignorados: as pastas abertas relêem com o filtro novo
  const primeiroFiltro = useRef(true);
  useEffect(() => {
    if (primeiroFiltro.current) { primeiroFiltro.current = false; return; }
    void recarregar(false);
  }, [recarregar, root]);
  // o watcher do Rust vigia esta raiz enquanto a árvore a mostra
  useEffect(() => {
    void api.fsWatch(root).catch(() => {});
    return () => { void api.fsUnwatch(root).catch(() => {}); };
  }, [root]);
  const recarregarRef = useRef(recarregar);
  recarregarRef.current = recarregar;
  // voltou para o app (Explorer, terminal) ou mexeu no git pelo rodapé: o que mudou aparece sem o ⟳
  useEffect(() => {
    const onFocus = () => { if (box.current?.offsetParent) void recarregar(false); };
    // watcher (ou cópia do Explorer): só relê se mudou a raiz ou uma pasta aberta; `dirs` vazio = mudou muita coisa
    const onMudou = (ev: Event) => {
      const d = (ev as CustomEvent<{ root: string | null; dirs: string[] }>).detail;
      const low = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
      if (d.root && low(d.root) !== low(root)) return;
      const abertas = new Set([root, ...expandedRef.current].map(low));
      if (d.dirs.length && !d.dirs.some((x) => abertas.has(low(x)))) return;
      if (box.current?.offsetParent) void recarregar(false);
    };
    window.addEventListener("focus", onFocus);
    window.addEventListener(GIT_CHANGED, onFocus);
    window.addEventListener("regente:fs-changed", onMudou);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener(GIT_CHANGED, onFocus);
      window.removeEventListener("regente:fs-changed", onMudou);
    };
  }, [recarregar]);

  const flat = useMemo(() => {
    const out: Node[] = [];
    const walk = (dir: string, level: number) => {
      for (const e of children[dir] ?? []) {
        out.push({ e, level });
        if (e.isDir && expanded.has(e.path)) walk(e.path, level + 1);
      }
    };
    walk(root, 1);
    return out;
  }, [children, expanded, root]);

  const v = useVirtualizer({ count: flat.length, getScrollElement: () => box.current, estimateSize: () => 24, overscan: 20 });
  const selIdx = flat.findIndex((n) => n.e.path === sel);
  // aria-activedescendant só aponta para uma linha que existe no DOM (lista virtualizada)
  const selRendered = v.getVirtualItems().some((vi) => vi.index === selIdx);
  // "Revelar na árvore" (menu da aba do editor): abre as pastas até o arquivo, seleciona e rola até ele
  const [revelar, setRevelar] = useState<string | null>(null);
  useEffect(() => {
    const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
    const fora = () => toast("Esse arquivo não está na árvore deste workspace", "info");
    const onRevelar = async (ev: Event) => {
      // cada workspace tem a sua árvore; só a que está na tela responde (as escondidas não avisam "fora da árvore")
      if (!box.current?.offsetParent) return;
      const alvo = (ev as CustomEvent<string>).detail;
      const raiz = norm(root);
      // da outra árvore (projeto × missão): o WorkspaceView troca de árvore e repete o pedido
      if (outraRaiz && norm(alvo).startsWith(`${norm(outraRaiz)}\\`) && !norm(alvo).startsWith(`${raiz}\\`)) return;
      if (!norm(alvo).startsWith(`${raiz}\\`)) return fora();
      const partes = alvo.slice(raiz.length).split(/[\\/]/).filter(Boolean);
      let dir = root;
      const abrir: string[] = [];
      for (const parte of partes) {
        const list = await api.fsList(dir, ignorados).catch(() => null);
        if (!list) return fora();
        const pasta = dir;
        setChildren((c) => ({ ...c, [pasta]: list }));
        const e = list.find((x) => x.name.toLowerCase() === parte.toLowerCase());
        if (!e) return fora();
        if (!e.isDir) {
          setExpanded((x) => new Set([...x, ...abrir]));
          setSel(e.path);
          setRevelar(e.path);
          return;
        }
        abrir.push(e.path);
        dir = e.path;
      }
    };
    window.addEventListener("regente:revelar", onRevelar);
    return () => window.removeEventListener("regente:revelar", onRevelar);
  }, [root, toast, outraRaiz, ignorados]);
  // a linha só existe depois que as pastas abrem: rola quando ela entra na lista
  useEffect(() => {
    if (!revelar) return;
    const i = flat.findIndex((n) => n.e.path === revelar);
    if (i < 0) return;
    v.scrollToIndex(i, { align: "center" });
    box.current?.focus();
    setRevelar(null);
  }, [revelar, flat, v]);

  async function toggle(e: FsEntry) {
    if (expanded.has(e.path)) {
      // recolher leva as subpastas junto: o ⟳ e o foco não relêem pastas que não estão à vista
      setExpanded((x) => new Set([...x].filter((p) => !under(p, e.path))));
      return;
    }
    await load(e.path);
    setExpanded((x) => new Set(x).add(e.path));
  }
  function activate(e: FsEntry) {
    setSel(e.path);
    if (e.isDir) toggle(e);
    else onOpen(e.path);
  }
  const selected = () => flat.find((n) => n.e.path === sel)?.e ?? null;
  const targetDir = () => { const cur = selected(); return cur ? (cur.isDir ? cur.path : parentOf(cur.path)) : root; };

  async function remove(e: FsEntry) {
    const low = e.path.toLowerCase();
    const dirty = Object.values(useStore.getState().dirty).flat().filter((p) => under(p.toLowerCase(), low)).length;
    const warn = dirty ? ` Há alterações não salvas em ${dirty} arquivo(s), que serão perdidas.` : "";
    if (!(await ask(`Mover "${e.name}" para a Lixeira?${warn}`, { title: "Apagar", kind: "warning" }))) return;
    try {
      await api.fsDelete(e.path);
      moved(e.path, null);
      await load(parentOf(e.path));
    } catch (err) {
      toast(errMsg(err), "erro");
    }
  }
  async function duplicar(e: FsEntry) {
    try {
      const novo = await api.fsCopy(e.path, parentOf(e.path));
      await load(parentOf(e.path));
      setSel(novo);
    } catch (err) {
      toast(errMsg(err), "erro");
    }
  }
  function copiar(texto: string) {
    navigator.clipboard.writeText(texto).then(() => toast("Caminho copiado", "ok"), (e) => toast(errMsg(e), "erro"));
  }
  async function commitEdit() {
    const ed = editing;
    setEditing(null);
    if (cancelEdit.current) {
      cancelEdit.current = false;
      return;
    }
    const value = ed?.value.trim();
    if (!ed || !value) return;
    try {
      if (ed.mode === "renomear" && ed.path) {
        await api.fsRename(ed.path, join(ed.dir, value));
        moved(ed.path, join(ed.dir, value));
      } else {
        await api.fsCreate(join(ed.dir, value), ed.mode === "nova-pasta");
        if (ed.dir !== root) setExpanded((x) => new Set(x).add(ed.dir));
      }
      await load(ed.dir);
      if (ed.mode === "novo-arquivo") onOpen(join(ed.dir, value));
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  async function moveTo(src: string, dir: string) {
    if (parentOf(src) === dir || under(dir, src)) return;
    const to = join(dir, src.split(/[\\/]/).pop()!);
    try {
      await api.fsRename(src, to);
      moved(src, to);
      await load(parentOf(src));
      await load(dir);
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  moveRef.current = moveTo;
  function onKey(ev: KeyboardEvent<HTMLDivElement>) {
    if (editing) return;
    const i = selIdx;
    const cur = flat[i]?.e;
    const go = (j: number) => {
      const k = Math.max(0, Math.min(flat.length - 1, j));
      if (flat[k]) {
        setSel(flat[k].e.path);
        v.scrollToIndex(k);
      }
    };
    switch (ev.key) {
      case "ArrowDown": go(i + 1); break;
      case "ArrowUp": go(i - 1); break;
      case "Home": go(0); break;
      case "End": go(flat.length - 1); break;
      case "ArrowRight": if (cur?.isDir) { if (!expanded.has(cur.path)) toggle(cur); else go(i + 1); } break;
      case "ArrowLeft":
        if (cur?.isDir && expanded.has(cur.path)) toggle(cur);
        else if (cur) go(flat.findIndex((n) => n.e.path === parentOf(cur.path)));
        break;
      case "Enter": if (cur) activate(cur); break;
      case "F2": if (cur) setEditing({ mode: "renomear", dir: parentOf(cur.path), path: cur.path, value: cur.name }); break;
      case "Delete": if (cur) remove(cur); break;
      default: return;
    }
    ev.preventDefault();
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", minHeight: 0, borderRight: "1px solid var(--line)" }}>
      <div className="tree-tools">
        <button className="icon-btn" aria-label="Novo arquivo" title="Novo arquivo" onClick={() => setEditing({ mode: "novo-arquivo", dir: targetDir(), value: "" })}>+ arquivo</button>
        <button className="icon-btn" aria-label="Nova pasta" title="Nova pasta" onClick={() => setEditing({ mode: "nova-pasta", dir: targetDir(), value: "" })}>+ pasta</button>
        <button className="icon-btn" aria-label="Apagar selecionado" title="Apagar (Del)" disabled={!sel} onClick={() => { const c = selected(); if (c) remove(c); }}>🗑</button>
        <button className="icon-btn" aria-label="Mostrar ignorados" aria-pressed={ignorados} title={ignorados ? "Esconder .git, node_modules, target…" : "Mostrar .git, node_modules, target…"} onClick={() => setIgnorados((x) => !x)}>{ignorados ? "◉" : "◌"}</button>
        <button className="icon-btn" aria-label="Atualizar" title="Atualizar" aria-busy={atualizando} disabled={atualizando} onClick={() => void recarregar(true)}>{atualizando ? "…" : "⟳"}</button>
      </div>
      {editing && (
        <div style={{ padding: 4 }}>
          {/* Enter/Esc devolvem o foco à árvore; o blur que isso causa salva (ou não, no Esc) */}
          <input autoFocus style={{ width: "100%" }} aria-label={EDIT_LABEL[editing.mode]} value={editing.value}
            placeholder={editing.mode === "nova-pasta" ? "nome da pasta" : editing.mode === "novo-arquivo" ? "nome do arquivo" : "novo nome"}
            onChange={(e) => setEditing({ ...editing, value: e.target.value })} onBlur={commitEdit}
            onKeyDown={(e) => {
              if (e.key === "Escape") cancelEdit.current = true;
              if (e.key === "Enter" || e.key === "Escape") box.current?.focus();
            }} />
        </div>
      )}
      {menu && (
        <div ref={menuRef} className="menu" role="menu" aria-label={`Ações de ${menu.e.name}`} style={{ position: "fixed", left: menu.x, top: menu.y, zIndex: 50 }}
          onKeyDown={(ev) => { if (ev.key === "Escape") { setMenu(null); box.current?.focus(); } }}>
          {!menu.e.isDir && VERB_ORDER.map((v) => (
            <button key={v} className="menu-item" role="menuitem" onClick={() => { askFile(v, menu.e); setMenu(null); }}>Pedir ao agente: {verbLabel(v)}</button>
          ))}
          {!menu.e.isDir && <button className="menu-item" role="menuitem" onClick={() => { mentionFile(menu.e); setMenu(null); }}>Mencionar no compositor</button>}
          {!menu.e.isDir && <div className="menu-sep" role="separator" />}
          <button className="menu-item" role="menuitem" onClick={() => { void revealItemInDir(menu.e.path).catch((e) => toast(errMsg(e), "erro")); setMenu(null); }}>Revelar no Explorer</button>
          {!menu.e.isDir && <button className="menu-item" role="menuitem" onClick={() => { void openPath(menu.e.path).catch((e) => toast(errMsg(e), "erro")); setMenu(null); }}>Abrir com o app padrão</button>}
          <button className="menu-item" role="menuitem" onClick={() => { copiar(menu.e.path); setMenu(null); }}>Copiar caminho</button>
          <button className="menu-item" role="menuitem" onClick={() => { copiar(relPath(menu.e.path, root)); setMenu(null); }}>Copiar caminho relativo</button>
          <div className="menu-sep" role="separator" />
          <button className="menu-item" role="menuitem" onClick={() => { setEditing({ mode: "novo-arquivo", dir: menu.e.isDir ? menu.e.path : parentOf(menu.e.path), value: "" }); setMenu(null); }}>Novo arquivo aqui</button>
          <button className="menu-item" role="menuitem" onClick={() => { setEditing({ mode: "nova-pasta", dir: menu.e.isDir ? menu.e.path : parentOf(menu.e.path), value: "" }); setMenu(null); }}>Nova pasta aqui</button>
          <button className="menu-item" role="menuitem" onClick={() => { setEditing({ mode: "renomear", dir: parentOf(menu.e.path), path: menu.e.path, value: menu.e.name }); setMenu(null); }}>Renomear (F2)</button>
          <button className="menu-item" role="menuitem" onClick={() => { void duplicar(menu.e); setMenu(null); }}>Duplicar</button>
          <button className="menu-item" role="menuitem" onClick={() => { void remove(menu.e); setMenu(null); }}>Apagar (Del)</button>
        </div>
      )}
      <div ref={box} className="tree" role="tree" aria-label="Arquivos" tabIndex={0} onKeyDown={onKey}
        aria-activedescendant={selRendered ? `${uid}-${selIdx}` : undefined}
        data-drop="file-move" data-dir={root}>
        <div style={{ height: v.getTotalSize(), position: "relative" }}>
          {v.getVirtualItems().map((vi) => {
            const n = flat[vi.index];
            const siblings = children[parentOf(n.e.path)] ?? [];
            const marca = n.e.isDir ? undefined : marcaDe(n.e.path);
            const letra = marca === "?" ? "U" : marca;
            const pastaMudou = n.e.isDir && pastasMudadas.has(n.e.path.toLowerCase());
            const tipo = n.e.isDir ? null : tipoArquivo(n.e.name);
            return (
              <div key={n.e.path} id={`${uid}-${vi.index}`} role="treeitem" aria-level={n.level} aria-setsize={siblings.length} aria-posinset={siblings.indexOf(n.e) + 1}
                aria-selected={sel === n.e.path} aria-expanded={n.e.isDir ? expanded.has(n.e.path) : undefined}
                className={`tree-item ${sel === n.e.path ? "sel" : ""} ${letra ? `git-${letra.toLowerCase()}` : ""} ${pastaMudou ? "git-pasta" : ""}`}
                style={{ position: "absolute", top: vi.start, left: 0, right: 0, paddingLeft: 8 + (n.level - 1) * 14 }}
                onPointerDown={(e) => { startPointerDrag(e, { kind: "file", data: n.e.path }, n.e.name); }}
                data-drop="file-move" data-dir={n.e.isDir ? n.e.path : parentOf(n.e.path)}
                onClick={() => activate(n.e)}
                onContextMenu={(e) => { e.preventDefault(); setSel(n.e.path); setMenu({ x: e.clientX, y: e.clientY, e: n.e }); }}>
                <span aria-hidden className="tree-seta">{n.e.isDir ? (expanded.has(n.e.path) ? "▾" : "▸") : ""}</span>
                {tipo && <span className={`ic ic-${tipo.classe}`} aria-hidden>{tipo.rotulo}</span>}
                <span className="tree-nome">{n.e.name}</span>
                {letra && <span className="git-selo" title={letra === "U" ? "Novo, fora do Git" : letra === "A" ? "Adicionado" : letra === "D" ? "Apagado" : "Modificado"}>{letra}</span>}
                {pastaMudou && <span className="git-ponto" aria-label="contém alterações" />}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
