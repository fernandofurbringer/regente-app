import { emitTo } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { openPath, openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal, type ILinkProvider } from "@xterm/xterm";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errMsg, type PaneRow, type PaneSearch } from "../lib/ipc";
import { searchSummary } from "../lib/paneSearch";
import { openPaneHistory } from "../lib/paneHistory";
import { frameDetail, hibernatedAnnouncement } from "../lib/frameText";
import { quoted } from "../lib/paths";
import { ATTACH_MAX_TRIES, attachRetryDelay, scrollbackLines } from "../lib/terminalOpts";
import { TERMINAL_FONT_DEFAULT, loadTerminalFont } from "../lib/terminalFont";
import type { DragPayload } from "../lib/pointerDrag";
import { browserUrl, fileUriToPath, findLinks, findLocalhost, inkContinuation, isImagePath, resolvePath, spanToCells, splitLocation, type Cell, type Found } from "../lib/links";
// ▼ trilha C
import { dicaDePorta, registrar } from "../lib/devServers";
// ▲ trilha C
import { useStore } from "../state/store";
import { queueErrorScan } from "../lib/paneErrors";
import { selectionSendTo } from "../lib/askAgent";
import Composer from "./Composer";
import PaneActivity from "./PaneActivity";

export const terms = new Map<string, { screen(): string; text(): string; focus(): void; find(term: string): void; hasSelection(): boolean; selection(): string }>();

/** Foco no terminal de um painel que o usuário acabou de abrir (o xterm só existe depois do mount). Não vale para o que o agente ou o cron abre. */
export function focusNewPane(id: string) {
  setTimeout(() => {
    const t = terms.get(id);
    if (t) t.focus();
    else useStore.getState().patch({ activePane: id });
  }, 50);
}
// "pergunta" é processo vivo: fora da lista, o painel soltava a saída e a tela congelava na pergunta do agente
export const ALIVE = ["iniciando", "trabalhando", "ocioso", "acordando", "pergunta"];
const DARK = { background: "#0c0d17", foreground: "#f3f1fb", cursor: "#cbbcff", selectionBackground: "#b6a0ff44" };
// alto contraste: preto puro e texto branco (o resto da paleta ANSI é a do xterm, já clara sobre preto)
const CONTRASTE = { background: "#000000", foreground: "#ffffff", cursor: "#ffffff", selectionBackground: "#c8b8ff66" };
// paleta ANSI completa no claro: o padrão do xterm (amarelo/branco claros) some no fundo branco
const LIGHT = {
  background: "#ffffff", foreground: "#1c2128", cursor: "#8b6cff", selectionBackground: "#8b6cff33",
  black: "#24292f", red: "#cf222e", green: "#116329", yellow: "#9a6700", blue: "#0969da", magenta: "#8250df", cyan: "#1b7c83", white: "#57606a",
  brightBlack: "#57606a", brightRed: "#a40e26", brightGreen: "#1a7f37", brightYellow: "#7d4e00", brightBlue: "#0550ae", brightMagenta: "#6639ba", brightCyan: "#136061", brightWhite: "#424a53",
};
const termTheme = (t: string | undefined) => (t === "light" ? LIGHT : t === "contraste" ? CONTRASTE : DARK);
const SEARCH_DECOR = { decorations: { matchOverviewRuler: "#d29922", activeMatchColorOverviewRuler: "#8b6cff", matchBackground: "#d2992255", activeMatchBackground: "#8b6cff88" } };
/** linhas quebradas olhadas acima e abaixo da linha sob o mouse */
const WRAP_MAX = 8;
const VIEWABLE = /\.(pdf|png|jpe?g|gif|webp|svg|avif|ico|bmp)$/i;

/** PDF e imagem abrem no app do sistema; o resto (que o editor não abre) só é mostrado no Explorer, nunca executado. */
export async function openOutside(path: string) {
  if (VIEWABLE.test(path)) return openPath(path);
  await revealItemInDir(path);
  useStore.getState().toast("Arquivo mostrado no Explorer", "info");
}

/** Acha em disco o caminho do link (o texto e, se não existir, as alternativas dele). null = não existe em lugar nenhum. */
async function resolverEmDisco(f: Found, pane: PaneRow) {
  for (const t of [f.text, ...(f.alts ?? [])]) {
    const loc = splitLocation(t);
    const r = await api.fsResolverLink(loc.raw, pane.cwd, pane.workspaceId).catch(() => undefined);
    // remoto ou erro do comando: segue o caminho como texto, como antes
    if (r === undefined) return { ...resolvePath(t, pane.cwd), dentro: true, dir: false };
    if (r) return { path: r.path, line: loc.line, col: loc.col, dentro: r.dentro, dir: r.dir };
  }
  return null;
}

/** Abre um link achado no terminal: localhost → painel navegador; URL externa → navegador do sistema;
 *  imagem do projeto → painel imagem (reaproveita o existente); arquivo → editor (fora dos workspaces, só leitura);
 *  pasta → Explorer; PDF/binário/grande → app do sistema. Não achou o arquivo: avisa e fica no terminal. */
export async function openFound(f: Found, pane: PaneRow) {
  const st = useStore.getState();
  if (f.kind === "url") {
    if (findLocalhost(f.text)) st.upsertPane(await api.paneSpawn({ workspaceId: pane.workspaceId, missionId: pane.missionId, kind: "browser", cwd: pane.cwd, urlOrPath: browserUrl(f.text) }));
    else await openUrl(f.text);
    return;
  }
  const achado = await resolverEmDisco(f, pane);
  if (!achado) {
    st.toast(`Não achei ${splitLocation(f.text).raw} (procurei na pasta do painel, no projeto e nos repositórios dentro dele)`, "erro");
    return;
  }
  const { path, line, col } = achado;
  if (achado.dir) return revealItemInDir(path);
  if (/\.pdf$/i.test(path)) return openOutside(path);
  if (f.kind === "video") {
    // um vídeo por caminho: o painel já aberto para o mesmo arquivo recebe o foco em vez de outro
    const existing = Object.values(st.panes).find((p) => p.kind === "video" && p.workspaceId === pane.workspaceId && p.urlOrPath?.toLowerCase() === path.toLowerCase());
    if (existing) return document.getElementById(`pane-${existing.id}`)?.scrollIntoView({ block: "nearest" });
    st.upsertPane(await api.paneSpawn({ workspaceId: pane.workspaceId, missionId: pane.missionId, kind: "video", cwd: pane.cwd, urlOrPath: path }));
    return;
  }
  if (f.kind === "image") {
    const check = await api.imageCheck(path);
    if (check === "invalida") return st.toast(`${path} não é uma imagem válida`, "erro");
    if (check !== "ok") return openOutside(path);
    const existing = Object.values(st.panes).find((p) => p.kind === "image" && p.workspaceId === pane.workspaceId && p.urlOrPath?.toLowerCase() === path.toLowerCase());
    if (existing) {
      document.getElementById(`pane-${existing.id}`)?.scrollIntoView({ block: "nearest" });
      window.dispatchEvent(new CustomEvent("regente:reload-image", { detail: existing.id }));
      return;
    }
    st.upsertPane(await api.paneSpawn({ workspaceId: pane.workspaceId, missionId: pane.missionId, kind: "image", cwd: pane.cwd, urlOrPath: path }));
    return;
  }
  if (window.location.hash.startsWith("#/pane/")) {
    // janela de painel destacado não tem editor: abre na janela dona do workspace (a destacada dele ou a principal)
    const ws = `ws-${pane.workspaceId}`;
    return emitTo((await WebviewWindow.getByLabel(ws)) ? ws : "main", "open-file", { wsId: pane.workspaceId, path, line, col });
  }
  st.patch({ openFile: { ...st.openFile, [pane.workspaceId]: { path, line, col } }, view: { ...st.view, [pane.workspaceId]: "arquivos" } });
}

/** Link OSC 8 (o programa marca o trecho com o destino): `file://` abre como arquivo; http(s) como qualquer URL, sem a
 *  confirmação em inglês do xterm. */
function oscLinkHandler(getPane: () => PaneRow) {
  return {
    allowNonHttpProtocols: true,
    activate: (_ev: MouseEvent, uri: string) => {
      const pane = getPane();
      const f: Found = /^https?:\/\//i.test(uri)
        ? { kind: "url", text: uri, start: 0, end: 0 }
        : /^file:\/\//i.test(uri)
          ? { kind: isImagePath(fileUriToPath(uri)) ? "image" : "path", text: fileUriToPath(uri), start: 0, end: 0 }
          : { kind: "url", text: "", start: 0, end: 0 };
      if (!f.text) return;
      openFound(f, pane).catch((e) => useStore.getState().toastErr(e));
    },
  };
}

/**
 * Monta o texto de uma linha do buffer célula a célula (caractere largo ocupa duas células e conta uma vez), guardando a
 * célula de cada caractere. `pular`: recuo do começo que não entra no texto (continuação do Ink).
 */
function cellsOf(term: Terminal, row: number, pular = 0): { text: string; cells: Cell[] } {
  const line = term.buffer.active.getLine(row);
  let text = "";
  const cells: Cell[] = [];
  if (!line) return { text, cells };
  for (let x = pular; x < term.cols; x++) {
    const c = line.getCell(x);
    if (!c || c.getWidth() === 0) continue;
    const ch = c.getChars() || " ";
    for (let i = 0; i < ch.length; i++) cells.push({ x: x + 1, y: row + 1 });
    text += ch;
  }
  return { text, cells };
}

/** Junta as linhas físicas quebradas (isWrapped) e as continuações do Ink numa linha lógica, acha os links e devolve os
 *  da linha sob o mouse. Um link que só existe juntando as linhas leva as metades como alternativas (o clique confere). */
function linkProvider(term: Terminal, getPane: () => PaneRow): ILinkProvider {
  return {
    provideLinks(y, cb) {
      const buf = term.buffer.active;
      const row = y - 1;
      const textoDe = (r: number) => buf.getLine(r)?.translateToString(true) ?? "";
      // sobe enquanto a linha é continuação (do xterm, ou do Ink: a de cima encosta na margem e esta vem recuada)
      let first = row;
      while (first > Math.max(0, row - WRAP_MAX)) {
        const l = buf.getLine(first);
        if (l?.isWrapped || inkContinuation(textoDe(first - 1), textoDe(first), term.cols) !== null) first--;
        else break;
      }
      let text = "";
      let cells: Cell[] = [];
      /** posição no texto onde começa cada continuação do Ink (um link que a cruza é "juntado") */
      const juntas: number[] = [];
      for (let r = first; r <= row + WRAP_MAX; r++) {
        const l = buf.getLine(r);
        if (!l) break;
        const ink = r > first && !l.isWrapped ? inkContinuation(textoDe(r - 1), textoDe(r), term.cols) : null;
        if (r > first && !l.isWrapped && ink === null) break;
        if (ink !== null) juntas.push(text.length);
        const pular = ink !== null ? textoDe(r).length - ink.length : 0;
        const parte = cellsOf(term, r, pular);
        // a linha de cima termina na margem: os espaços do fim dela não entram (o caminho continua colado na de baixo)
        if (ink !== null) {
          const corte = text.trimEnd().length;
          text = text.slice(0, corte);
          cells = cells.slice(0, corte);
        }
        text += parte.text;
        cells = cells.concat(parte.cells);
      }
      const achados = findLinks(text);
      // link que cruza uma junta do Ink: as metades dele (os links achados linha a linha) viram alternativas
      for (const f of achados) {
        if (!juntas.some((j) => f.start < j && f.end > j)) continue;
        const cortes = [0, ...juntas, text.length];
        const metades = cortes.slice(1).flatMap((fim, i) => findLinks(text.slice(cortes[i], fim)).map((x) => x.text));
        f.alts = [...(f.alts ?? []), ...metades];
      }
      const links = achados
        .map((f) => ({ f, range: spanToCells(cells, f.start, f.end) }))
        .filter((x): x is { f: Found; range: { start: Cell; end: Cell } } => !!x.range && x.range.start.y <= y && x.range.end.y >= y)
        .map(({ f, range }) => ({
          range,
          text: f.text,
          activate: () => {
            openFound(f, getPane()).catch((e) => useStore.getState().toastErr(e));
          },
        }));
      cb(links.length ? links : undefined);
    },
  };
}

function fitIn(fit: FitAddon | null, el: HTMLElement | null) {
  if (!fit || !el || el.offsetWidth === 0) return;
  try {
    fit.fit();
  } catch {
    /* xterm ainda sem dimensões */
  }
}

/** WebGL só nos painéis visíveis: o Chromium limita os contextos (≈ 16) e cada painel oculto custaria um. */
const WEBGL_MAX = 8;
const webglPanes = new Set<string>();
const CLI_LABEL: Record<string, string> = { claude: "Claude Code", codex: "Codex", gemini: "Gemini" };

/** O xterm só remede a célula quando a opção muda: troca por uma genérica e volta, depois refaz o fit. */
function remeasure(t: Terminal, fit: FitAddon | null, el: HTMLElement | null) {
  const { fontFamily } = t.options;
  t.options.fontFamily = "monospace";
  t.options.fontFamily = fontFamily;
  fitIn(fit, el);
}

export default function TerminalPane({ pane }: { pane: PaneRow }) {
  const composerOpen = useStore((s) => !!s.composerOpen[pane.id]);
  const host = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const searchRef = useRef<SearchAddon | null>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const paneRef = useRef(pane);
  paneRef.current = pane;
  const sinkRef = useRef<number | null>(null);
  /** época do attach: callbacks de uma época velha (processo trocou, desmontou) são ignorados */
  const gen = useRef(0);
  const attaching = useRef(false);
  /** o xterm está lendo o replay: as respostas dele a consultas antigas (DA, DSR, OSC 10/11, DECRQM) não vão ao processo */
  const replaying = useRef(false);
  const onBytesRef = useRef<(b: Uint8Array) => void>(() => {});
  // só os campos que o xterm usa: trocar outra configuração não refaz fonte, tema nem fit de todos os painéis
  const font = useStore((s) => s.settings?.terminalFont);
  const fontSize = useStore((s) => s.settings?.terminalFontSize);
  const theme = useStore((s) => s.settings?.theme);
  const scrollback = useStore((s) => s.settings?.scrollback);
  const screenReader = useStore((s) => s.settings?.screenReaderMode ?? false);
  const build = useStore((s) => s.windowsBuild);
  const query = useStore((s) => s.search[pane.id] ?? "");
  // "Iniciando…" enquanto não chega saída; falha de attach depois de ~60 s com botão para tentar de novo
  const [gotOutput, setGotOutput] = useState(false);
  const [attachFailed, setAttachFailed] = useState(false);
  const [attachKey, setAttachKey] = useState(0);
  const [searchOpen, setSearchOpen] = useState(false);
  // trechos do histórico em disco (botão "Histórico" da busca); o buffer do xterm continua com o SearchAddon
  const [disk, setDisk] = useState<PaneSearch | null>(null);
  const [diskBusy, setDiskBusy] = useState(false);
  const [count, setCount] = useState({ idx: -1, total: 0 });
  const [frame, setFrame] = useState<string | null>(null);
  // menu do clique direito com seleção: copiar ou enviar ao agente
  const [selMenu, setSelMenu] = useState<{ x: number; y: number; text: string } | null>(null);
  // anúncio curto e sempre montado (região viva); o quadro completo vai sob demanda por aria-describedby
  const [live, setLive] = useState("");
  const lastStatus = useRef(pane.status);
  useEffect(() => {
    const prev = lastStatus.current;
    lastStatus.current = pane.status;
    setLive(hibernatedAnnouncement(prev, pane.status, pane.name));
  }, [pane.status, pane.name]);

  // cria o xterm uma vez por painel
  useEffect(() => {
    replaying.current = false;
    const fontStack = font ?? TERMINAL_FONT_DEFAULT;
    const size = fontSize ?? 13;
    // a fonte web pode chegar depois do open: quando chegar, o xterm remede a célula
    const fontReady = loadTerminalFont(fontStack, size);
    const term = new Terminal({
      fontFamily: fontStack,
      fontSize: size,
      scrollback: scrollbackLines(scrollback),
      screenReaderMode: screenReader,
      cursorBlink: true,
      allowProposedApi: true,
      windowsPty: { backend: "conpty", buildNumber: build },
      theme: termTheme(theme),
      // OSC 8: file:// abre o arquivo e http(s) abre sem a confirmação em inglês (ver oscLinkHandler)
      linkHandler: oscLinkHandler(() => paneRef.current),
    });
    const fit = new FitAddon();
    const search = new SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    term.open(host.current!);
    term.registerLinkProvider(linkProvider(term, () => paneRef.current));
    // renderizador WebGL quando o painel está à vista; sem WebGL, ou ao perder o contexto, fica o DOM
    let webgl: WebglAddon | null = null;
    const webglOff = () => {
      if (!webgl) return;
      webgl.dispose();
      webgl = null;
      webglPanes.delete(pane.id);
    };
    const webglOn = () => {
      if (webgl || webglPanes.size >= WEBGL_MAX) return;
      try {
        const addon = new WebglAddon();
        addon.onContextLoss(webglOff);
        term.loadAddon(addon);
        webgl = addon;
        webglPanes.add(pane.id);
      } catch {
        /* sem WebGL neste sistema: o DOM continua valendo */
      }
    };
    const visibility = new IntersectionObserver(([e]) => (e.isIntersecting ? webglOn() : webglOff()));
    visibility.observe(host.current!);
    const dec = new TextDecoder();
    // a saída de terminal (não de agente) é olhada atrás de erros para o chip "Corrigir isto"
    const errDec = new TextDecoder();
    let tail = "";
    onBytesRef.current = (b) => {
      term.write(b);
      if (paneRef.current.kind === "terminal") queueErrorScan(paneRef.current.id, errDec.decode(b, { stream: true }));
      // junta o fim do pedaço anterior: a URL pode vir cortada entre dois pedaços
      const novo = dec.decode(b, { stream: true });
      const s = tail + novo;
      // porta em uso: a dica diz quem está com ela (só no pedaço novo, para não repetir o aviso) — A1.7
      const antes = useStore.getState();
      const dica = dicaDePorta(novo, antes.devServers, paneRef.current.id, (id) => antes.panes[id]?.name ?? "outro painel");
      if (dica) antes.toast(dica, "erro");
      const url = findLocalhost(s);
      // URL colada no fim do pedaço pode estar incompleta: espera o próximo
      if (!url || s.endsWith(url)) {
        tail = s.slice(-200);
        return;
      }
      tail = "";
      const st = useStore.getState();
      const ws = paneRef.current.workspaceId;
      if (st.readyUrl[ws] !== url) st.patch({ readyUrl: { ...st.readyUrl, [ws]: url } });
      // um registro por painel de terminal: o chip "Servidores (n)" lista e age (Parar). Painel de agente nunca entra.
      if (paneRef.current.kind === "terminal") st.patch({ devServers: registrar(st.devServers, { paneId: paneRef.current.id, url, since: Date.now(), source: "terminal" }) });
    };
    term.onData((d) => {
      if (!replaying.current) api.paneWrite(paneRef.current.id, d).catch(() => {});
    });
    term.onResize(({ cols, rows }) => api.paneResize(paneRef.current.id, cols, rows).catch(() => {}));
    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type !== "keydown" || !ev.ctrlKey || ev.altKey) return true;
      const k = ev.key.toLowerCase();
      if (k === "f") {
        ev.preventDefault();
        if (searchInput.current) searchInput.current.select();
        else setSearchOpen(true);
        searchInput.current?.focus();
        return false;
      }
      // Ctrl+V / Ctrl+Shift+V: o colar nativo chega ao textarea do xterm (bracketed paste)
      if (k === "v") return false;
      if (k === "c" && (ev.shiftKey || term.hasSelection())) {
        if (term.hasSelection()) {
          navigator.clipboard.writeText(term.getSelection()).catch(() => {});
          term.clearSelection();
        }
        return false;
      }
      return true;
    });
    search.onDidChangeResults((r) => setCount({ idx: r.resultIndex, total: r.resultCount }));
    // quem digita aqui é o foco do piloto/ajudante: o backend guarda o painel focado
    const onFocus = () => {
      api.paneFocus(pane.id).catch(() => {});
      useStore.getState().patch({ activePane: pane.id });
    };
    term.textarea?.addEventListener("focus", onFocus);
    fontReady.then(() => {
      if (termRef.current === term) remeasure(term, fit, host.current);
    });
    termRef.current = term;
    fitRef.current = fit;
    searchRef.current = search;
    terms.set(pane.id, {
      // busca do xterm (realce e rolagem) para o termo vindo da busca nos painéis
      find: (term) => { search.findNext(term, SEARCH_DECOR); },
      // Ctrl+Shift+C: com seleção no terminal, o copiar nativo do xterm é que vale
      hasSelection: () => term.hasSelection(),
      selection: () => term.getSelection(),
      // texto puro das linhas visíveis (quadro congelado ao hibernar)
      screen: () => {
        const b = term.buffer.active;
        const out: string[] = [];
        for (let i = 0; i < term.rows; i++) out.push(b.getLine(b.viewportY + i)?.translateToString(true) ?? "");
        return out.join("\n");
      },
      text: () => {
        const b = term.buffer.active;
        const out: string[] = [];
        for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) ?? "");
        return out.join("\n").trimEnd();
      },
      focus: () => term.focus(),
    });
    let timer: number | undefined;
    const ro = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = window.setTimeout(() => fitIn(fit, host.current), 60);
    });
    ro.observe(host.current!);
    return () => {
      gen.current++;
      attaching.current = false;
      clearTimeout(timer);
      ro.disconnect();
      visibility.disconnect();
      webglOff();
      term.textarea?.removeEventListener("focus", onFocus);
      terms.delete(pane.id);
      if (sinkRef.current !== null) api.paneDetach(pane.id, sinkRef.current).catch(() => {});
      sinkRef.current = null;
      termRef.current = null;
      fitRef.current = null;
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pane.id]);

  // tema/fonte ao vivo; a célula muda de tamanho, então refaz o fit
  useEffect(() => {
    const t = termRef.current;
    if (!t || font === undefined || fontSize === undefined) return;
    t.options.fontFamily = font;
    t.options.fontSize = fontSize;
    t.options.theme = termTheme(theme);
    t.options.scrollback = scrollbackLines(scrollback);
    t.options.screenReaderMode = screenReader;
    t.options.windowsPty = { backend: "conpty", buildNumber: build };
    const raf = requestAnimationFrame(() => fitIn(fitRef.current, host.current));
    let live = true;
    loadTerminalFont(font, fontSize).then(() => {
      if (live && termRef.current === t) remeasure(t, fitRef.current, host.current);
    });
    return () => {
      live = false;
      cancelAnimationFrame(raf);
    };
  }, [font, fontSize, theme, scrollback, build, screenReader]);

  // attach/reattach conforme o status
  useEffect(() => {
    const status = pane.status;
    if (status === "acordando" || !ALIVE.includes(status)) {
      // processo novo (ou nenhum): o sink antigo morreu e attach em voo é descartado
      gen.current++;
      attaching.current = false;
      if (sinkRef.current !== null) api.paneDetach(pane.id, sinkRef.current).catch(() => {});
      sinkRef.current = null;
    }
    if (!ALIVE.includes(status) || sinkRef.current !== null || attaching.current) return;
    const my = ++gen.current;
    const live = () => gen.current === my;
    attaching.current = true;
    termRef.current?.reset();
    setGotOutput(false);
    setAttachFailed(false);
    let tries = 0;
    let first = true;
    let announced = false;
    const tryAttach = () =>
      api
        .paneAttach(pane.id, (b) => {
          if (!live()) return;
          if (!announced) {
            announced = true;
            setGotOutput(true);
          }
          if (first) {
            // o primeiro pedaço é o replay: sem detectar URL e sem responder consultas
            first = false;
            replaying.current = true;
            termRef.current?.write(b, () => { replaying.current = false; });
          } else onBytesRef.current(b);
        })
        .then((id) => {
          if (!live()) return void api.paneDetach(pane.id, id).catch(() => {});
          attaching.current = false;
          sinkRef.current = id;
          setAttachFailed(false);
          // processo acordado nasce em 120×32: ajusta ao tamanho real
          const t = termRef.current;
          if (t) api.paneResize(pane.id, t.cols, t.rows).catch(() => {});
        })
        .catch(() => {
          if (!live()) return;
          // backoff curto (40 a 320 ms) para o processo que acabou de nascer; depois 500 ms, até ~60 s
          if (++tries <= ATTACH_MAX_TRIES) window.setTimeout(tryAttach, attachRetryDelay(tries));
          else {
            attaching.current = false;
            setAttachFailed(true);
          }
        });
    tryAttach();
  }, [pane.status, pane.id, attachKey]);

  // quadro congelado enquanto hibernado/acordando
  useEffect(() => {
    if (pane.status !== "hibernado" && pane.status !== "acordando") return setFrame(null);
    let cancelled = false;
    api.paneFrame(pane.id).then((f) => { if (!cancelled) setFrame(f ?? ""); }).catch(() => { if (!cancelled) setFrame(""); });
    return () => { cancelled = true; };
  }, [pane.status, pane.id]);

  const setQuery = (v: string) => useStore.getState().patch({ search: { ...useStore.getState().search, [pane.id]: v } });
  function find(dir: 1 | -1) {
    const s = searchRef.current;
    if (!s || !query) return;
    if (dir === 1) s.findNext(query, SEARCH_DECOR);
    else s.findPrevious(query, SEARCH_DECOR);
  }
  async function searchDisk() {
    const q = query.trim();
    if (!q || diskBusy) return;
    setDiskBusy(true);
    try {
      setDisk(await api.searchPaneHistory(pane.id, q));
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    } finally {
      setDiskBusy(false);
    }
  }
  function closeSearch() {
    setDisk(null);
    setSearchOpen(false);
    searchRef.current?.clearDecorations();
    termRef.current?.focus();
  }
  // zona de soltar (data-drop="shot-path"): a captura ou a pasta do corte entra como caminho citado
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const onDrop = (e: Event) => {
      const d = (e as CustomEvent<DragPayload>).detail;
      if (d.kind === "shot-path") api.paneWrite(paneRef.current.id, `${quoted(d.data)} `).catch(() => {});
    };
    el.addEventListener("regente-drop", onDrop);
    return () => el.removeEventListener("regente-drop", onDrop);
  }, []);
  function frameKey(e: KeyboardEvent) {
    const d = e.key === "Enter" ? "\r" : e.key.length === 1 ? e.key : null;
    if (d) api.paneWrite(pane.id, d).catch(() => {});
  }

  return (
    <>
      <div className="term" ref={host} data-drop="shot-path"
        onContextMenu={(e) => {
          // sem seleção o clique direito segue como antes
          const t = terms.get(pane.id);
          if (!t?.hasSelection()) return;
          e.preventDefault();
          setSelMenu({ x: e.clientX, y: e.clientY, text: t.selection() });
        }} />
      {selMenu && <SelectionMenu at={selMenu} pane={pane} onClose={() => setSelMenu(null)} />}
      {pane.kind === "agent" && composerOpen && <Composer pane={pane} />}
      {pane.kind === "agent" && pane.cli === "claude" && <PaneActivity pane={pane} />}
      {pane.status === "iniciando" && !gotOutput && !attachFailed && (
        <div className="term-msg" role="status">Iniciando {(pane.cli && CLI_LABEL[pane.cli]) || pane.name}…</div>
      )}
      {attachFailed && (
        <div className="term-msg" role="alert">
          Não foi possível conectar ao terminal.
          <button className="btn" onClick={() => { setAttachFailed(false); setAttachKey((k) => k + 1); }}>Tentar de novo</button>
        </div>
      )}
      {searchOpen && (
        <div className="search-bar" role="search">
          <input ref={searchInput} autoFocus value={query} placeholder="Buscar" aria-label="Buscar no terminal" onChange={(e) => { setQuery(e.target.value); setDisk(null); }}
            onKeyDown={(e) => { if (e.key === "Enter") find(e.shiftKey ? -1 : 1); if (e.key === "Escape") closeSearch(); }} />
          <span className="count">{count.total === 0 ? "0" : `${count.idx + 1}/${count.total}${count.total >= 1000 ? "+ (limite)" : ""}`}</span>
          <button className="icon-btn" aria-label="Resultado anterior" onClick={() => find(-1)}>↑</button>
          <button className="icon-btn" aria-label="Próximo resultado" onClick={() => find(1)}>↓</button>
          <button className="icon-btn" aria-label="Buscar no histórico em disco" title="Buscar também no histórico em disco (o que já saiu do buffer)" disabled={!query.trim() || diskBusy} onClick={() => void searchDisk()}>{diskBusy ? "…" : "Histórico"}</button>
          <button className="icon-btn" aria-label="Fechar busca" onClick={closeSearch}>✕</button>
          {disk && (
            <div className="search-disk" role="region" aria-label="Resultados no histórico em disco">
              <div className="meta" role="status">{searchSummary(disk, query.trim())}</div>
              {disk.hits.map((h, i) => (
                <button key={i} className="menu-item" style={{ display: "flex", gap: 8, width: "100%", textAlign: "left" }} title="Abrir o histórico completo no editor"
                  onClick={() => void openPaneHistory(pane.id, pane.workspaceId).catch((e) => useStore.getState().toast(errMsg(e), "erro"))}>
                  <span className="badge">no histórico</span>
                  <span style={{ fontFamily: "var(--mono)", fontSize: 12, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{h.excerpt}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
      {/* uma frase, uma vez por hibernação; não anuncia na montagem nem na restauração */}
      <div className="sr-only" aria-live="polite">{live}</div>
      {frame !== null && (
        <>
        {/* o texto da tela não é anunciado sozinho: o botão do quadro o referencia por aria-describedby */}
        <div id={`frame-detail-${pane.id}`} className="sr-only">{frameDetail(frame)}</div>
        <div className="frame" tabIndex={0} role="button" aria-label="Painel hibernado. Clique ou digite para acordar"
          aria-describedby={`frame-detail-${pane.id}`} onClick={() => api.paneWake(pane.id).catch(() => {})} onKeyDown={frameKey}>
          <div>{frame}</div>
          <span className="frame-note">{pane.status === "acordando" ? "Acordando…" : "Hibernado: clique ou digite para acordar"}</span>
        </div>
        </>
      )}
    </>
  );
}

/** Clique direito com seleção no terminal: copiar, ou mandar o texto ao agente. Fecha ao clicar fora ou com Esc. */
function SelectionMenu({ at, pane, onClose }: { at: { x: number; y: number; text: string }; pane: PaneRow; onClose(): void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDown = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) onClose(); };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [onClose]);
  return (
    <div ref={ref} className="menu" role="menu" aria-label="Seleção do terminal" style={{ position: "fixed", left: at.x, top: at.y, zIndex: 50 }}
      onKeyDown={(e) => { if (e.key === "Escape") { onClose(); terms.get(pane.id)?.focus(); } }}>
      <button className="menu-item" role="menuitem" onClick={() => { navigator.clipboard.writeText(at.text).catch(() => {}); onClose(); terms.get(pane.id)?.focus(); }}>Copiar</button>
      <button className="menu-item" role="menuitem" onClick={() => { onClose(); useStore.getState().patch({ sendTo: selectionSendTo(pane, at.text) }); }}>Enviar seleção ao agente…</button>
    </div>
  );
}
