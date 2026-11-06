/** Layout da grade de painéis e das colunas do workspace (bloco C): modos de grade, proporções das colunas, alturas das
 *  linhas e a largura da árvore. Puro, menos `read*`/`write*` (armazenamento local, nunca lançam). */

export type GridMode = "igual" | "principal-esq" | "principal-cima" | "colunas";
export interface GridPrefs {
  modo: GridMode;
  /** colunas no modo "igual": automático (1 painel = 1, senão 2) ou fixo */
  cols: "auto" | 1 | 2 | 3;
  /** proporção de cada coluna; vazio (ou de outro tamanho) = todas iguais */
  colFr: number[];
  /** altura fixa (px) por linha lógica; ausente = divide o espaço */
  rowPx: Record<string, number>;
}
export interface ShellPrefs { treePx: number }

export const GRID_DEFAULT: GridPrefs = { modo: "igual", cols: "auto", colFr: [], rowPx: {} };
export const SHELL_DEFAULT: ShellPrefs = { treePx: 280 };
export const GUTTER_PX = 6;
/** menor largura de coluna e menor/maior altura de linha que o arrasto aceita */
export const MIN_COL_PX = 160;
export const MIN_ROW_PX = 160;
export const MAX_ROW_PX = 2000;
const MODOS: GridMode[] = ["igual", "principal-esq", "principal-cima", "colunas"];
export const MODE_LABEL: Record<GridMode, string> = {
  igual: "Iguais", "principal-esq": "Principal à esquerda", "principal-cima": "Principal em cima", colunas: "Colunas lado a lado",
};

/** Item da grade, na ordem: painel visível ou linha de largura total (grupo recolhido, aviso da missão). */
export type Slot = { kind: "pane"; id: string } | { kind: "full"; key: string };
/** Posição lógica (0-based) de um item; `full` = linha de largura total. */
export interface Placed { key: string; col: number; row: number; colSpan: number; rowSpan: number; full: boolean }
export interface GridPlan { cols: number; rows: number; placed: Placed[]; /** linhas só de itens de largura total: altura automática */ fullRows: number[] }

/** Próximo modo do ciclo (Ctrl+Shift+L). */
export function nextMode(m: GridMode): GridMode {
  return MODOS[(MODOS.indexOf(m) + 1) % MODOS.length];
}

/** Troca o modo e/ou as colunas. O arranjo mudou: os tamanhos arrastados no outro não servem e saem (senão a altura de
 *  uma linha de um modo vira a de outra linha no novo). Sem mudança, nada muda. */
export function comArranjo(p: GridPrefs, modo: GridMode, cols: GridPrefs["cols"] = p.cols): GridPrefs {
  if (p.modo === modo && p.cols === cols) return p;
  return { modo, cols, colFr: [], rowPx: {} };
}

/** Coloca os itens em linhas de `cols` colunas, na ordem. A linha cheia fecha a linha anterior; o último painel de uma
 *  linha incompleta estica até o fim dela (como a grade sempre fez com o ímpar). */
function fill(slots: Slot[], cols: number, row0: number, out: Placed[], fullRows: number[]): number {
  let row = row0;
  let col = 0;
  let last: Placed | null = null;
  const fecha = () => {
    if (col > 0 && last) last.colSpan = cols - last.col;
    if (col > 0) row++;
    col = 0;
    last = null;
  };
  for (const s of slots) {
    if (s.kind === "full") {
      fecha();
      out.push({ key: s.key, col: 0, row, colSpan: cols, rowSpan: 1, full: true });
      fullRows.push(row);
      row++;
      continue;
    }
    last = { key: s.id, col, row, colSpan: 1, rowSpan: 1, full: false };
    out.push(last);
    col++;
    if (col === cols) {
      row++;
      col = 0;
      last = null;
    }
  }
  fecha();
  return row;
}

/** Plano da grade: quantas colunas e linhas e onde fica cada item. Painel focado: só ele, numa célula. */
export function planGrid(slots: Slot[], focused: string | null, prefs: GridPrefs): GridPlan {
  const panes = slots.filter((s): s is { kind: "pane"; id: string } => s.kind === "pane");
  if (focused && panes.some((p) => p.id === focused)) {
    return { cols: 1, rows: 1, placed: [{ key: focused, col: 0, row: 0, colSpan: 1, rowSpan: 1, full: false }], fullRows: [] };
  }
  const n = panes.length;
  const fulls = slots.filter((s) => s.kind === "full");
  const placed: Placed[] = [];
  const fullRows: number[] = [];
  if (n >= 2 && (prefs.modo === "principal-esq" || prefs.modo === "principal-cima")) {
    const [main, ...rest] = panes;
    let rows: number;
    let cols: number;
    if (prefs.modo === "principal-esq") {
      cols = 2;
      placed.push({ key: main.id, col: 0, row: 0, colSpan: 1, rowSpan: rest.length, full: false });
      rest.forEach((p, i) => placed.push({ key: p.id, col: 1, row: i, colSpan: 1, rowSpan: 1, full: false }));
      rows = rest.length;
    } else {
      cols = Math.min(3, rest.length);
      placed.push({ key: main.id, col: 0, row: 0, colSpan: cols, rowSpan: 1, full: false });
      rows = fill(rest, cols, 1, placed, fullRows);
    }
    // grupos recolhidos e avisos vêm depois, cada um na sua linha
    rows = fill(fulls, cols, rows, placed, fullRows);
    return { cols, rows, placed, fullRows };
  }
  const cols = prefs.modo === "colunas"
    ? Math.max(1, n)
    : prefs.cols === "auto" ? (n <= 1 ? 1 : 2) : Math.max(1, Math.min(prefs.cols, Math.max(1, n)));
  const rows = fill(slots, cols, 0, placed, fullRows);
  return { cols, rows, placed, fullRows };
}

/** Proporções efetivas: as salvas quando servem para este número de colunas, senão todas iguais. */
export function colFrOf(prefs: GridPrefs, cols: number): number[] {
  return prefs.colFr.length === cols ? prefs.colFr : Array.from({ length: cols }, () => 1);
}

/** `grid-template-columns` e `grid-template-rows`, com uma faixa de `GUTTER_PX` entre colunas e entre linhas. */
export function templates(plan: GridPlan, prefs: GridPrefs): { columns: string; rows: string } {
  const columns = colFrOf(prefs, plan.cols).map((f) => `minmax(0, ${f}fr)`).join(` ${GUTTER_PX}px `);
  // a altura salva só vale para a linha com divisor embaixo (é ele que a define e desfaz): o painel focado, a linha
  // única e as linhas de outro modo não herdam uma altura que não daria para desfazer
  const comDivisor = new Set(rowGutters(plan).map((g) => g.r));
  const rows = Array.from({ length: Math.max(1, plan.rows) }, (_, r) => {
    if (plan.fullRows.includes(r)) return "auto";
    const px = comDivisor.has(r) ? prefs.rowPx[String(r)] : undefined;
    // a altura arrastada é a preferida, não fixa: com menos espaço (janela menor) a linha encolhe até o mínimo
    return px ? `minmax(${MIN_ROW_PX}px, ${px}px)` : "minmax(220px, 1fr)";
  }).join(` ${GUTTER_PX}px `);
  return { columns, rows };
}

/** `grid-column` e `grid-row` de um item (as faixas dos divisores ficam nos índices pares). */
export function cssPlace(p: Placed): { gridColumn: string; gridRow: string } {
  return { gridColumn: `${2 * p.col + 1} / span ${2 * p.colSpan - 1}`, gridRow: `${2 * p.row + 1} / span ${2 * p.rowSpan - 1}` };
}

/** Matriz linha x coluna com a chave do item que ocupa cada célula (undefined = vazia). */
function occupancy(plan: GridPlan): (string | undefined)[][] {
  const m: (string | undefined)[][] = Array.from({ length: plan.rows }, () => Array.from({ length: plan.cols }, () => undefined));
  for (const p of plan.placed) {
    for (let r = p.row; r < p.row + p.rowSpan; r++) for (let c = p.col; c < p.col + p.colSpan; c++) if (m[r]) m[r][c] = p.key;
  }
  return m;
}

/** Divisor entre colunas `c` e `c+1`, nas linhas `rowFrom..rowTo` (inclusive), onde dois itens diferentes se encostam. */
export interface ColGutter { c: number; rowFrom: number; rowTo: number }
/** Divisor entre linhas `r` e `r+1`, nas colunas `colFrom..colTo`, onde duas linhas de painéis se encostam. */
export interface RowGutter { r: number; colFrom: number; colTo: number }

/** Divisores de coluna, juntando as linhas seguidas em que a fronteira está aberta (item que atravessa a fronteira a fecha). */
export function colGutters(plan: GridPlan): ColGutter[] {
  const m = occupancy(plan);
  const out: ColGutter[] = [];
  for (let c = 0; c < plan.cols - 1; c++) {
    let from = -1;
    for (let r = 0; r <= plan.rows; r++) {
      const a = m[r]?.[c];
      const b = m[r]?.[c + 1];
      const open = r < plan.rows && a !== undefined && b !== undefined && a !== b;
      if (open && from < 0) from = r;
      if (!open && from >= 0) {
        out.push({ c, rowFrom: from, rowTo: r - 1 });
        from = -1;
      }
    }
  }
  return out;
}

/** Divisores de linha: só entre duas linhas de painéis (linha cheia tem altura automática), onde a fronteira está aberta. */
export function rowGutters(plan: GridPlan): RowGutter[] {
  const m = occupancy(plan);
  const out: RowGutter[] = [];
  for (let r = 0; r < plan.rows - 1; r++) {
    if (plan.fullRows.includes(r) || plan.fullRows.includes(r + 1)) continue;
    let from = -1;
    for (let c = 0; c <= plan.cols; c++) {
      const a = m[r]?.[c];
      const b = m[r + 1]?.[c];
      const open = c < plan.cols && a !== undefined && b !== undefined && a !== b;
      if (open && from < 0) from = c;
      if (!open && from >= 0) {
        out.push({ r, colFrom: from, colTo: c - 1 });
        from = -1;
      }
    }
  }
  return out;
}

/** `grid-column`/`grid-row` de um divisor (faixa par entre as colunas ou linhas que ele separa). */
export function cssColGutter(g: ColGutter): { gridColumn: string; gridRow: string } {
  return { gridColumn: `${2 * g.c + 2}`, gridRow: `${2 * g.rowFrom + 1} / ${2 * g.rowTo + 2}` };
}
export function cssRowGutter(g: RowGutter): { gridColumn: string; gridRow: string } {
  return { gridColumn: `${2 * g.colFrom + 1} / ${2 * g.colTo + 2}`, gridRow: `${2 * g.r + 2}` };
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
const r4 = (v: number) => Math.round(v * 10_000) / 10_000;

/** Arrasto do divisor entre as colunas `c` e `c+1`: `wA`/`wB` são as larguras (px) no início do arrasto. A soma das
 *  proporções das duas não muda; cada uma fica com pelo menos `MIN_COL_PX`. */
export function resizePair(fr: number[], c: number, wA: number, wB: number, dx: number): number[] {
  const total = wA + wB;
  if (c < 0 || c + 1 >= fr.length || total < 2 * MIN_COL_PX) return fr;
  const a = clamp(wA + dx, MIN_COL_PX, total - MIN_COL_PX);
  const soma = fr[c] + fr[c + 1];
  const out = [...fr];
  out[c] = r4((soma * a) / total);
  out[c + 1] = r4(soma - out[c]);
  return out;
}

/** Arrasto do divisor abaixo da linha `r`: `h0` é a altura (px) dela no início do arrasto. */
export function resizeRow(rowPx: Record<string, number>, r: number, h0: number, dy: number): Record<string, number> {
  return { ...rowPx, [String(r)]: Math.round(clamp(h0 + dy, MIN_ROW_PX, MAX_ROW_PX)) };
}

/** Faixas resolvidas (px) de `grid-template-columns`/`-rows` lidas do estilo computado ("220px 6px 300px"). */
export function trackPx(computed: string): number[] {
  return computed.trim().split(/\s+/).map((t) => parseFloat(t)).filter((v) => Number.isFinite(v));
}

const isRecord = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

/** Lê as preferências da grade; campo inválido cai no padrão. */
export function parseGridPrefs(raw: string | null): GridPrefs {
  try {
    const v: unknown = raw ? JSON.parse(raw) : null;
    if (!isRecord(v)) return { ...GRID_DEFAULT };
    const modo = MODOS.includes(v.modo as GridMode) ? (v.modo as GridMode) : GRID_DEFAULT.modo;
    const cols = v.cols === "auto" || v.cols === 1 || v.cols === 2 || v.cols === 3 ? v.cols : GRID_DEFAULT.cols;
    const fr = Array.isArray(v.colFr) ? v.colFr : [];
    const colFr = fr.length >= 1 && fr.length <= 64 && fr.every((x) => typeof x === "number" && x >= 0.05 && x <= 20) ? (fr as number[]) : [];
    const rowPx: Record<string, number> = {};
    if (isRecord(v.rowPx)) {
      for (const [k, px] of Object.entries(v.rowPx)) {
        if (/^\d{1,3}$/.test(k) && typeof px === "number" && px >= MIN_ROW_PX && px <= MAX_ROW_PX) rowPx[k] = Math.round(px);
      }
    }
    return { modo, cols, colFr, rowPx };
  } catch {
    return { ...GRID_DEFAULT };
  }
}

/** Lê a largura da árvore; valor fora da faixa cai no padrão. */
export function parseShellPrefs(raw: string | null): ShellPrefs {
  try {
    const v: unknown = raw ? JSON.parse(raw) : null;
    if (!isRecord(v)) return { ...SHELL_DEFAULT };
    const num = (x: unknown, lo: number, hi: number, d: number) => (typeof x === "number" && x >= lo && x <= hi ? Math.round(x) : d);
    return {
      treePx: num(v.treePx, 160, 600, SHELL_DEFAULT.treePx),
    };
  } catch {
    return { ...SHELL_DEFAULT };
  }
}

/** Limites do arrasto das colunas do workspace (os mesmos do `parseShellPrefs`). */
export const SHELL_LIMITS: Record<keyof ShellPrefs, [number, number]> = { treePx: [160, 600] };
export function clampShell(k: keyof ShellPrefs, px: number): number {
  const [lo, hi] = SHELL_LIMITS[k];
  return Math.round(clamp(px, lo, hi));
}

const gridKey = (ws: string) => `regente.layout.${ws}`;
const SHELL_KEY = "regente.layout.shell";
function ler(k: string): string | null {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
}
function gravar(k: string, v: unknown): void {
  try {
    localStorage.setItem(k, JSON.stringify(v));
  } catch {
    // sem armazenamento: vale até fechar o Regente
  }
}
export const readGridPrefs = (ws: string): GridPrefs => parseGridPrefs(ler(gridKey(ws)));
export const writeGridPrefs = (ws: string, p: GridPrefs): void => gravar(gridKey(ws), p);
export const readShellPrefs = (): ShellPrefs => parseShellPrefs(ler(SHELL_KEY));
export const writeShellPrefs = (p: ShellPrefs): void => gravar(SHELL_KEY, p);
