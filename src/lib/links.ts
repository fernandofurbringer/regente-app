export type LinkKind = "url" | "path" | "image" | "video";
/**
 * Um link achado numa linha do terminal. `alts`: outras leituras do mesmo trecho, tentadas no clique se `text` não
 * existir em disco (o caminho com espaço tenta antes a versão sem o começo; o caminho juntado de duas linhas tenta as
 * metades).
 */
export interface Found { kind: LinkKind; text: string; start: number; end: number; alts?: string[] }

// Quantificadores limitados (URL ≤ 2048, segmento ≤ 255, ≤ 32 pastas) e caminho só no início de um token:
// um token gigante (base64, minificado) não trava o provider de links.
const URL_RE = /\bhttps?:\/\/[^\s"'<>`]{1,2038}[^\s"'<>`.,;:)\]]/g;
const LOCALHOST_RE = /\bhttps?:\/\/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]):\d{2,5}(?:\/[^\s"'<>`)]*)?/;
const WILDCARD_HOST_RE = /^(https?:\/\/)(?:0\.0\.0\.0|\[::\])(?=:)/;
const EXT = [
  "tsx?", "jsx?", "mjs", "cjs", "json[c5]?", "mdx?", "rs", "py", "go", "java", "kts?", "cs", "csproj", "sln", "slnx", "props", "targets",
  "razor", "cshtml", "cpp", "cc", "c", "h", "hpp", "css", "scss", "less", "html?", "vue", "svelte", "astro", "toml", "ya?ml", "xml", "sql",
  "sh", "bash", "ps1", "psm1", "bat", "cmd", "txt", "log", "csv", "tsv", "ini", "cfg", "conf", "lock", "env", "gradle", "rb", "php",
  "swift", "dart", "lua", "proto", "graphql", "gql", "tf", "ipynb", "png", "jpe?g", "gif", "webp", "svg", "avif", "ico", "bmp", "pdf",
  "mp4", "webm", "patch", "diff",
].join("|");
/** arquivos conhecidos sem extensão */
const SEM_EXT = "Dockerfile|Makefile|Procfile|Gemfile|Jenkinsfile|Vagrantfile|LICENSE|CODEOWNERS";
/** linha e coluna: `:12`, `:12:5`, `:12-20`, `#L12`, `#L12C5`, `#L12-L20`, `(12,5)` (tsc, MSBuild) */
const SUFIXO = String.raw`(?:(?::\d{1,7}){1,2}(?:-\d{1,7})?|#L\d{1,7}(?:C\d{1,4})?(?:-L?\d{1,7})?|\(\d{1,7}(?:,\d{1,4})?\))?`;
/** letra, número (com acento), `_`, `.`, `@`, `+`, `-` */
const SEG = String.raw`[\p{L}\p{N}_.@+-]`;
const FIM = String.raw`(?![\p{L}\p{N}_]|\.[\p{L}\p{N}])`;
const PATH_RE = new RegExp(
  String.raw`(?<![\p{L}\p{N}_.@/\\-])(?:[A-Za-z]:[\\/]|~[\\/]|\/[a-zA-Z]\/)?(?:${SEG}{1,255}[\\/]){0,32}(?:[\p{L}\p{N}_@-]${SEG}{0,254}\.(?:${EXT})|(?:${SEM_EXT}))${SUFIXO}${FIM}`,
  "giu",
);
/** caminho absoluto do Windows com espaço nas pastas ("C:\Users\Ana Silva\…"): só vale se tiver espaço (o resto é do PATH_RE) */
const ABS_ESPACO_RE = new RegExp(String.raw`(?<![\p{L}\p{N}_])[A-Za-z]:\\(?:[^\\/:*?"<>|\r\n\x60]{1,255}\\){0,32}[^\\/:*?"<>|\r\n\x60]{1,254}?\.(?:${EXT})${SUFIXO}${FIM}`, "giu");
/** caminho entre aspas ou crases, que pode ter espaço */
const ASPAS_RE = new RegExp(String.raw`(["'\x60])([^"'\x60\r\n]{1,400}?\.(?:${EXT})${SUFIXO})\1`, "giu");
/** link markdown `[rótulo](destino)`: o par inteiro é um link só, para o destino */
const MD_RE = /\[([^\]\r\n]{1,300})\]\(([^)\s]{1,1000})\)/g;
/** nomes de tecnologia que parecem arquivo ("Node.js") e não têm pasta na frente */
const FALSO_ARQUIVO = /^(?:node|next|nuxt|vue|react|three|d3|chart|express|socket\.io|deno|bun|ember|backbone|angular|alpine|solid|preact)\.js$/i;
const IMG_RE = /\.(png|jpe?g|gif|webp|svg|avif|ico|bmp)$/i;
const VIDEO_EXT_RE = /\.(mp4|webm)$/i;

export function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g, "");
}

/** O caminho sem o sufixo de linha/coluna, e a linha e a coluna (1-based) quando há. */
export function splitLocation(text: string): { raw: string; line?: number; col?: number } {
  const t = text.trim();
  const m =
    t.match(/^(.*?)#L(\d+)(?:C(\d+))?(?:-L?\d+)?$/) ??
    t.match(/^(.*?)\((\d+)(?:,(\d+))?\)$/) ??
    t.match(/^(.*?):(\d+)(?::(\d+))?(?:-\d+)?$/);
  if (!m || !m[1]) return { raw: t };
  const out: { raw: string; line?: number; col?: number } = { raw: m[1] };
  out.line = Number(m[2]);
  if (m[3]) out.col = Number(m[3]);
  return out;
}

/** `file:///D:/a%20b/c.md#L3` → `D:\a b\c.md#L3`; o resto volta como veio. */
export function fileUriToPath(uri: string): string {
  if (!/^file:\/\//i.test(uri)) return uri;
  let p = uri.replace(/^file:\/\/(?:localhost)?/i, "");
  try {
    p = decodeURI(p);
  } catch {
    // %XX inválido: fica como veio
  }
  if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1);
  return p.replace(/\//g, "\\");
}

export function isImagePath(p: string): boolean {
  return IMG_RE.test(splitLocation(p).raw);
}

export function isVideoPath(p: string): boolean {
  return VIDEO_EXT_RE.test(splitLocation(p).raw);
}

const kindOf = (text: string): LinkKind => (isImagePath(text) ? "image" : isVideoPath(text) ? "video" : "path");

export function findLinks(line: string): Found[] {
  const out: Found[] = [];
  const livre = (start: number, end: number) => !out.some((o) => start < o.end && end > o.start);
  for (const m of line.matchAll(MD_RE)) {
    const href = fileUriToPath(m[2]);
    const start = m.index!;
    const end = start + m[0].length;
    if (/^https?:\/\//i.test(href)) out.push({ kind: "url", text: href, start, end });
    else if (!/^[a-z][a-z+.-]*:\/\//i.test(href) && !href.startsWith("#") && !href.startsWith("mailto:")) out.push({ kind: kindOf(href), text: href, start, end });
  }
  for (const m of line.matchAll(URL_RE)) {
    const start = m.index!;
    const end = start + m[0].length;
    if (livre(start, end)) out.push({ kind: "url", text: m[0], start, end });
  }
  for (const m of line.matchAll(ASPAS_RE)) {
    const start = m.index! + 1;
    const end = start + m[2].length;
    if (/\s/.test(m[2]) && livre(start, end)) out.push({ kind: kindOf(m[2]), text: m[2], start, end });
  }
  const espacados: Found[] = [];
  for (const m of line.matchAll(ABS_ESPACO_RE)) {
    const start = m.index!;
    const end = start + m[0].length;
    if (/ /.test(m[0]) && livre(start, end)) espacados.push({ kind: kindOf(m[0]), text: m[0], start, end });
  }
  for (const m of line.matchAll(PATH_RE)) {
    const start = m.index!;
    const end = start + m[0].length;
    if (!/[\\/]/.test(m[0]) && FALSO_ARQUIVO.test(splitLocation(m[0]).raw)) continue;
    // a cauda de um caminho com espaço vira a alternativa dele (o clique tenta o inteiro primeiro)
    const dono = espacados.find((e) => start < e.end && end > e.start);
    if (dono) {
      (dono.alts ??= []).push(m[0]);
      continue;
    }
    if (livre(start, end)) out.push({ kind: kindOf(m[0]), text: m[0], start, end });
  }
  out.push(...espacados);
  return out.sort((a, b) => a.start - b.start);
}

export function findLocalhost(text: string): string | null {
  return stripAnsi(text).match(LOCALHOST_RE)?.[0] ?? null;
}

// ponytail: "Port 5173 is in use, trying another one" do Vite é aviso, não erro
const PORT_IN_USE_RE = /EADDRINUSE|address already in use/i;

/** Porta que o servidor não conseguiu abrir ("EADDRINUSE", "address already in use", "Port 3000 is in use"); null se não é esse erro. */
export function findPortInUse(text: string): number | null {
  const limpo = stripAnsi(text);
  if (!PORT_IN_USE_RE.test(limpo)) return null;
  const m = limpo.match(/port (\d{2,5})/i) ?? limpo.match(/(?::{1,3}|0\.0\.0\.0:|localhost:|127\.0\.0\.1:)(\d{2,5})/);
  return m ? Number(m[1]) : null;
}

/** 0.0.0.0 and [::] listen on every interface but the browser pane reaches them as localhost. */
export function browserUrl(url: string): string {
  return url.replace(WILDCARD_HOST_RE, "$1localhost");
}

/** Caminho relativo resolvido contra `cwd` (só texto; quem procura em disco é o `fs_resolver_link`). Aceita `/` no cwd. */
export function resolvePath(p: string, cwd: string): { path: string; line?: number; col?: number } {
  const loc = splitLocation(p);
  const raw = loc.raw.replace(/\//g, "\\");
  const base = cwd.replace(/\//g, "\\").replace(/\\+$/, "");
  const abs = /^[A-Za-z]:\\/.test(raw) ? raw : `${base}\\${raw}`;
  const drive = abs.match(/^[A-Za-z]:/)?.[0] ?? "";
  const parts: string[] = [];
  for (const seg of abs.slice(drive.length).split("\\")) {
    if (seg === "." || seg === "") continue;
    if (seg === "..") parts.pop();
    else parts.push(seg);
  }
  const out: { path: string; line?: number; col?: number } = { path: drive ? `${drive}\\${parts.join("\\")}` : parts.join("\\") };
  if (loc.line) out.line = loc.line;
  if (loc.col) out.col = loc.col;
  return out;
}

/** Converte [start,end) de uma linha lógica (várias linhas físicas quebradas) em coordenadas 1-based do xterm. */
export function spanToRange(start: number, end: number, firstRow: number, cols: number) {
  const pos = (off: number) => ({ x: (off % cols) + 1, y: firstRow + Math.floor(off / cols) });
  return { start: pos(start), end: pos(end - 1) };
}

/** Uma célula do terminal por caractere do texto montado (posição 1-based do xterm). */
export interface Cell { x: number; y: number }

/** [start,end) do texto montado → células de início e fim (respeita caractere largo e linhas juntadas). */
export function spanToCells(cells: Cell[], start: number, end: number): { start: Cell; end: Cell } | null {
  const a = cells[start];
  const b = cells[end - 1];
  return a && b ? { start: a, end: b } : null;
}

/**
 * O Claude Code (Ink) quebra a linha ele mesmo, com a continuação recuada e sem marcar a linha como continuação:
 * a linha de cima que encosta na margem direita com um caractere de caminho, seguida de uma linha recuada que começa
 * com outro, provavelmente é um caminho partido. Devolve o texto da de baixo sem o recuo, ou null.
 */
export function inkContinuation(top: string, next: string, cols: number): string | null {
  if (top.length < cols - 1 || !/[\p{L}\p{N}_./\\@-]$/u.test(top)) return null;
  const m = next.match(/^( {1,12})([\p{L}\p{N}_./\\@-]\S*)/u);
  return m ? next.slice(m[1].length) : null;
}
