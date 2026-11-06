import { describe, expect, it } from "vitest";
import { cardIds, restoreCards, restoreTotals } from "./restoreCards";
import type { PaneRow } from "./ipc";

const pane = (id: string, missionId: string | null) => ({ id, missionId, name: id }) as PaneRow;

describe("restoreCards", () => {
  it("agrupa por missão e põe os painéis sem missão no próprio cartão", () => {
    const cards = restoreCards([pane("a", "m1"), pane("b", null), pane("c", "m1")], [{ id: "m1", name: "Login" }]);
    expect(cards.map((c) => [c.title, c.panes.map((p) => p.id)])).toEqual([
      ["Login", ["a", "c"]],
      ["Sem missão", ["b"]],
    ]);
  });

  it("missão que não está mais no rail ainda ganha cartão", () => {
    expect(restoreCards([pane("a", "gone")], [])[0].title).toBe("Missão");
  });
});

describe("cardIds", () => {
  const [card] = restoreCards([pane("a", "m"), pane("b", "m")], [{ id: "m", name: "M" }]);

  it("sem escolha vale o cartão inteiro", () => {
    expect(cardIds(card, null)).toEqual(["a", "b"]);
  });

  it("respeita os painéis desmarcados", () => {
    expect(cardIds(card, { a: false })).toEqual(["b"]);
    expect(cardIds(card, { a: false, b: false })).toEqual([]);
  });
});

describe("restoreTotals", () => {
  it("conta painéis e workspaces com pendências, ignorando os vazios", () => {
    const rest = { ws1: [pane("a", null), pane("b", null)], ws2: [], ws3: [pane("c", "m1")] };
    expect(restoreTotals(rest)).toEqual({ panes: 3, workspaces: 2 });
    expect(restoreTotals({})).toEqual({ panes: 0, workspaces: 0 });
  });
});
