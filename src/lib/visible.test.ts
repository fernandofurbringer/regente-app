import { describe, expect, it, vi } from "vitest";
import type { PaneGroup, PaneRow } from "./ipc";

vi.mock("./ipc", () => ({ api: { panesVisible: vi.fn() } }));

import { shownPanes, shownPanesIn, visiblePaneIds } from "./visible";

const pane = (id: string, over: Partial<PaneRow> = {}): PaneRow =>
  ({ id, workspaceId: "w", missionId: null, ord: 0, closedAt: null, groupId: null, kind: "agent", ...over }) as PaneRow;
const group = (id: string, over: Partial<PaneGroup> = {}): PaneGroup => ({ id, workspaceId: "w", name: id, collapsed: false, ord: 0, ...over });
const ids = (ps: PaneRow[]) => ps.map((p) => p.id);

describe("painéis à vista (shownPanes)", () => {
  it("numera na ordem do workspace, sem os fechados", () => {
    const panes = [pane("a", { ord: 2 }), pane("b", { ord: 0 }), pane("c", { ord: 1, closedAt: 5 })];
    expect(ids(shownPanesIn(panes, [], null))).toEqual(["b", "a"]);
  });

  it("filtro de missão: só os painéis da missão escolhida", () => {
    const panes = [pane("a", { ord: 0, missionId: "m1" }), pane("b", { ord: 1, missionId: "m2" }), pane("c", { ord: 2, missionId: "m1" })];
    expect(ids(shownPanesIn(panes, [], "m1"))).toEqual(["a", "c"]);
    expect(ids(shownPanesIn(panes, [], null))).toEqual(["a", "b", "c"]);
  });

  it("grupo recolhido sai da numeração, e os do grupo aberto vêm antes dos soltos (ordem da grade)", () => {
    const groups = [group("g1", { collapsed: true }), group("g2")];
    const panes = [
      pane("solto", { ord: 0 }),
      pane("oculto", { ord: 1, groupId: "g1" }),
      pane("de-g2", { ord: 2, groupId: "g2" }),
    ];
    expect(ids(shownPanesIn(panes, groups, null))).toEqual(["de-g2", "solto"]);
  });

  it("shownPanes(wsId) lê o estado e respeita o workspace", () => {
    const state = {
      active: "w", view: {}, missionSel: { w: null }, groups: {}, settings: null,
      panes: { a: pane("a", { ord: 0 }), x: pane("x", { ord: 0, workspaceId: "outro" }) },
    };
    expect(ids(shownPanes("w", state))).toEqual(["a"]);
  });

  it("painel de subagente (virtual) entra na numeração como qualquer outro", () => {
    const panes = [pane("a", { ord: 0 }), pane("sub", { ord: 1, kind: "subagent" })];
    expect(ids(shownPanesIn(panes, [], null))).toEqual(["a", "sub"]);
  });

  it("visiblePaneIds: vazio fora da visão de terminais; senão a lista numerada", () => {
    const base = { active: "w", missionSel: {}, groups: {}, settings: null, panes: { a: pane("a", { ord: 0 }) } };
    expect(visiblePaneIds({ ...base, view: { w: "arquivos" } })).toEqual([]);
    expect(visiblePaneIds({ ...base, view: {} })).toEqual(["a"]);
  });
});

describe("shownPanesIn com subagentes inline", () => {
  it("subagente inline fica fora da grade; no modo painel entra", () => {
    const list = [pane("a", { ord: 0 }), pane("s", { ord: 1, kind: "subagent" }), pane("t", { ord: 2, kind: "terminal" })];
    expect(ids(shownPanesIn(list, [], null, false))).toEqual(["a", "t"]);
    expect(ids(shownPanesIn(list, [], null, true))).toEqual(["a", "s", "t"]);
  });
});
