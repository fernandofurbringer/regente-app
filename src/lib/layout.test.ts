import { describe, expect, it } from "vitest";
import {
  GRID_DEFAULT, MIN_COL_PX, SHELL_DEFAULT, clampShell, colFrOf, comArranjo, colGutters, cssColGutter, cssPlace, cssRowGutter, nextMode,
  parseGridPrefs, parseShellPrefs, planGrid, resizePair, resizeRow, rowGutters, templates, trackPx, type GridPrefs, type Slot,
} from "./layout";

const P = (...ids: string[]): Slot[] => ids.map((id) => ({ kind: "pane", id }));
const F = (key: string): Slot => ({ kind: "full", key });
const prefs = (p: Partial<GridPrefs> = {}): GridPrefs => ({ ...GRID_DEFAULT, ...p });
const pos = (plan: ReturnType<typeof planGrid>) => Object.fromEntries(plan.placed.map((p) => [p.key, [p.row, p.col, p.rowSpan, p.colSpan]]));

describe("planGrid", () => {
  it("automático: 1 painel ocupa tudo; 3 painéis em 2 colunas, o último estica", () => {
    expect(pos(planGrid(P("a"), null, prefs()))).toEqual({ a: [0, 0, 1, 1] });
    const g = planGrid(P("a", "b", "c"), null, prefs());
    expect([g.cols, g.rows]).toEqual([2, 2]);
    expect(pos(g)).toEqual({ a: [0, 0, 1, 1], b: [0, 1, 1, 1], c: [1, 0, 1, 2] });
  });

  it("linha cheia fecha a linha anterior (o último painel dela estica) e tem altura automática", () => {
    const g = planGrid([...P("a", "b", "c"), F("g"), ...P("d")], null, prefs());
    expect(pos(g)).toEqual({ a: [0, 0, 1, 1], b: [0, 1, 1, 1], c: [1, 0, 1, 2], g: [2, 0, 1, 2], d: [3, 0, 1, 2] });
    expect([g.rows, g.fullRows]).toEqual([4, [2]]);
  });

  it("painel focado: só ele; foco desconhecido é ignorado", () => {
    expect(pos(planGrid([...P("a", "b"), F("g")], "b", prefs()))).toEqual({ b: [0, 0, 1, 1] });
    expect(planGrid(P("a", "b"), "z", prefs()).cols).toBe(2);
  });

  it("colunas fixas não passam do número de painéis; modo colunas põe todos lado a lado", () => {
    expect(planGrid(P("a", "b"), null, prefs({ cols: 3 })).cols).toBe(2);
    expect(planGrid(P("a", "b", "c", "d"), null, prefs({ cols: 3 })).cols).toBe(3);
    const c = planGrid(P("a", "b", "c"), null, prefs({ modo: "colunas" }));
    expect([c.cols, c.rows]).toEqual([3, 1]);
  });

  it("principal à esquerda: o primeiro ocupa a coluna inteira; os outros empilham; grupos vêm depois", () => {
    const g = planGrid([F("g"), ...P("a", "b", "c")], null, prefs({ modo: "principal-esq" }));
    expect(pos(g)).toEqual({ a: [0, 0, 2, 1], b: [0, 1, 1, 1], c: [1, 1, 1, 1], g: [2, 0, 1, 2] });
    expect(g.fullRows).toEqual([2]);
  });

  it("principal em cima: o primeiro na linha toda; os outros embaixo, até 3 por linha", () => {
    expect(pos(planGrid(P("a", "b", "c", "d"), null, prefs({ modo: "principal-cima" })))).toEqual({ a: [0, 0, 1, 3], b: [1, 0, 1, 1], c: [1, 1, 1, 1], d: [1, 2, 1, 1] });
    expect(pos(planGrid(P("a", "b"), null, prefs({ modo: "principal-cima" })))).toEqual({ a: [0, 0, 1, 1], b: [1, 0, 1, 1] });
    // um painel só: cai no modo igual
    expect(pos(planGrid(P("a"), null, prefs({ modo: "principal-esq" })))).toEqual({ a: [0, 0, 1, 1] });
  });
});

describe("templates e posições", () => {
  it("colunas com divisor de 6 px; linha com altura salva; linha cheia automática", () => {
    const g = planGrid([...P("a", "b", "c", "d"), F("g")], null, prefs());
    const t = templates(g, prefs({ rowPx: { "0": 300 } }));
    expect(t.columns).toBe("minmax(0, 1fr) 6px minmax(0, 1fr)");
    expect(t.rows).toBe("minmax(160px, 300px) 6px minmax(220px, 1fr) 6px auto");
    expect(templates(g, prefs({ colFr: [1.5, 0.5] })).columns).toBe("minmax(0, 1.5fr) 6px minmax(0, 0.5fr)");
  });

  it("altura salva só vale para a linha com divisor embaixo: foco e linha única ignoram", () => {
    const quatro = planGrid(P("a", "b", "c", "d"), null, prefs());
    expect(templates(quatro, prefs({ rowPx: { "0": 300, "1": 400 } })).rows).toBe("minmax(160px, 300px) 6px minmax(220px, 1fr)");
    expect(templates(planGrid(P("a", "b"), "a", prefs()), prefs({ rowPx: { "0": 500 } })).rows).toBe("minmax(220px, 1fr)");
    expect(templates(planGrid(P("a"), null, prefs()), prefs({ rowPx: { "0": 500 } })).rows).toBe("minmax(220px, 1fr)");
  });

  it("proporções de outro número de colunas são ignoradas", () => {
    expect(colFrOf(prefs({ colFr: [2, 1] }), 3)).toEqual([1, 1, 1]);
  });

  it("posição css pula as faixas dos divisores", () => {
    expect(cssPlace({ key: "x", col: 1, row: 0, colSpan: 1, rowSpan: 2, full: false })).toEqual({ gridColumn: "3 / span 1", gridRow: "1 / span 3" });
    expect(cssPlace({ key: "x", col: 0, row: 2, colSpan: 2, rowSpan: 1, full: true })).toEqual({ gridColumn: "1 / span 3", gridRow: "5 / span 1" });
  });
});

describe("divisores", () => {
  it("2 x 2: um divisor de coluna nas duas linhas e um de linha nas duas colunas", () => {
    const g = planGrid(P("a", "b", "c", "d"), null, prefs());
    expect(colGutters(g)).toEqual([{ c: 0, rowFrom: 0, rowTo: 1 }]);
    expect(rowGutters(g)).toEqual([{ r: 0, colFrom: 0, colTo: 1 }]);
    expect(cssColGutter({ c: 0, rowFrom: 0, rowTo: 1 })).toEqual({ gridColumn: "2", gridRow: "1 / 4" });
    expect(cssRowGutter({ r: 0, colFrom: 1, colTo: 1 })).toEqual({ gridColumn: "3 / 4", gridRow: "2" });
  });

  it("painel que atravessa a fronteira a fecha; linha cheia não tem divisor de linha", () => {
    const tres = planGrid(P("a", "b", "c"), null, prefs());
    expect(colGutters(tres)).toEqual([{ c: 0, rowFrom: 0, rowTo: 0 }]);
    const esq = planGrid(P("a", "b", "c"), null, prefs({ modo: "principal-esq" }));
    expect(colGutters(esq)).toEqual([{ c: 0, rowFrom: 0, rowTo: 1 }]);
    expect(rowGutters(esq)).toEqual([{ r: 0, colFrom: 1, colTo: 1 }]);
    expect(rowGutters(planGrid([...P("a", "b"), F("g")], null, prefs()))).toEqual([]);
  });

  it("um painel só: nenhum divisor", () => {
    const g = planGrid(P("a"), null, prefs());
    expect([colGutters(g), rowGutters(g)]).toEqual([[], []]);
  });
});

describe("arrasto", () => {
  it("resizePair mantém a soma e respeita a largura mínima", () => {
    expect(resizePair([1, 1], 0, 400, 400, 100)).toEqual([1.25, 0.75]);
    // 800 px no total, a da esquerda para em 800 - MIN_COL_PX = 640: 2 x 640/800
    expect(MIN_COL_PX).toBe(160);
    expect(resizePair([1, 1], 0, 400, 400, 1000)).toEqual([1.6, 0.4]);
    expect(resizePair([1, 1], 0, 100, 100, 50)).toEqual([1, 1]);
    // colunas 1 e 2 (soma 3) passam de 400/200 px para 300/300 px
    expect(resizePair([1, 2, 1], 1, 400, 200, -100)).toEqual([1, 1.5, 1.5]);
  });

  it("resizeRow limita a altura", () => {
    expect(resizeRow({}, 1, 300, -500)).toEqual({ "1": 160 });
    expect(resizeRow({ "0": 250 }, 0, 250, 40.4)).toEqual({ "0": 290 });
  });

  it("trackPx lê as faixas resolvidas", () => {
    expect(trackPx("220px 6px 300.5px")).toEqual([220, 6, 300.5]);
    expect(trackPx(" none ")).toEqual([]);
  });
});

describe("preferências salvas", () => {
  it("lixo cai no padrão; campos válidos ficam; inválidos saem", () => {
    expect(parseGridPrefs("x{")).toEqual(GRID_DEFAULT);
    expect(parseGridPrefs(null)).toEqual(GRID_DEFAULT);
    expect(parseGridPrefs(JSON.stringify({ modo: "principal-esq", cols: 3, colFr: [1.2, 0.8], rowPx: { "0": 300, x: 5, "2": 99999 } })))
      .toEqual({ modo: "principal-esq", cols: 3, colFr: [1.2, 0.8], rowPx: { "0": 300 } });
    expect(parseGridPrefs(JSON.stringify({ modo: "louco", cols: 9, colFr: [1, -1] }))).toEqual(GRID_DEFAULT);
  });

  it("larguras fora da faixa caem no padrão; clampShell limita o arrasto", () => {
    expect(parseShellPrefs(JSON.stringify({ treePx: 5000 }))).toEqual(SHELL_DEFAULT);
    expect(clampShell("treePx", 90)).toBe(160);
  });

  it("trocar o modo ou as colunas zera os tamanhos; repetir o mesmo arranjo não muda nada", () => {
    const p = prefs({ colFr: [1.5, 0.5], rowPx: { "0": 300 } });
    expect(comArranjo(p, "principal-esq")).toEqual({ modo: "principal-esq", cols: "auto", colFr: [], rowPx: {} });
    expect(comArranjo(p, "igual", 3)).toEqual({ modo: "igual", cols: 3, colFr: [], rowPx: {} });
    expect(comArranjo(p, "igual")).toBe(p);
  });

  it("o ciclo de modos volta ao começo", () => {
    expect([nextMode("igual"), nextMode("principal-esq"), nextMode("principal-cima"), nextMode("colunas")]).toEqual(["principal-esq", "principal-cima", "colunas", "igual"]);
  });
});
