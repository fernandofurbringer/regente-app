import { describe, expect, it } from "vitest";
import { adoptedGroupId, groupedItems, groupHeads } from "./groups";
import type { PaneGroup, PaneRow } from "./ipc";

const pane = (id: string, groupId: string | null = null) => ({ id, groupId }) as PaneRow;
const group = (id: string, collapsed = false) => ({ id, name: id, collapsed, workspaceId: "w", ord: 0 }) as PaneGroup;
const label = (items: ReturnType<typeof groupedItems>) => items.map((i) => (i.kind === "group" ? `grupo:${i.count}` : i.pane.id));

describe("groupedItems", () => {
  it("abre o grupo antes dos painéis dele e conta os painéis", () => {
    const items = groupedItems([pane("a"), pane("b", "g"), pane("c", "g")], [group("g")]);
    expect(label(items)).toEqual(["grupo:2", "b", "c", "a"]);
  });

  it("painéis do grupo ficam em sequência mesmo com painel solto no meio da ordem", () => {
    const items = groupedItems([pane("a", "g"), pane("b"), pane("c", "g")], [group("g")]);
    expect(label(items)).toEqual(["grupo:2", "a", "c", "b"]);
  });

  it("painéis de grupo recolhido vêm escondidos e o cabeçalho continua", () => {
    const items = groupedItems([pane("a", "g"), pane("b")], [group("g", true)]);
    expect(items[0]).toMatchObject({ kind: "group", count: 1 });
    expect(items[1]).toMatchObject({ kind: "pane", hidden: true });
    expect(items[2]).toMatchObject({ kind: "pane", hidden: false });
  });

  it("grupo vazio abre na sua posição de grupo, antes dos painéis soltos", () => {
    const items = groupedItems([pane("a")], [group("vazio")]);
    expect(items[0]).toMatchObject({ kind: "group", count: 0 });
    expect(label(items)).toEqual(["grupo:0", "a"]);
  });

  it("painel apontando para grupo inexistente fica sem grupo", () => {
    const items = groupedItems([pane("a", "sumiu")], []);
    expect(items).toEqual([{ kind: "pane", pane: pane("a", "sumiu"), hidden: false }]);
  });
});

describe("groupHeads", () => {
  it("o cabeçalho vai no primeiro painel VISÍVEL do grupo, não no primeiro não recolhido", () => {
    const items = groupedItems([pane("a", "g"), pane("b", "g")], [group("g")]);
    // só "b" está na missão em foco
    expect(groupHeads(items, new Set(["b"])).get("g")).toBe("b");
  });

  it("grupo sem nenhum painel visível não tem cabeçalho na grade", () => {
    const items = groupedItems([pane("a", "g")], [group("g")]);
    expect(groupHeads(items, new Set()).has("g")).toBe(false);
  });

  it("painel de grupo recolhido nunca leva o cabeçalho", () => {
    const items = groupedItems([pane("a", "g"), pane("b", "g")], [group("g", true)]);
    expect(groupHeads(items, new Set(["a", "b"])).has("g")).toBe(false);
  });
});

describe("adoptedGroupId", () => {
  it("soltar sobre um painel de grupo faz o painel solto entrar nesse grupo", () => {
    expect(adoptedGroupId(pane("a"), pane("b", "g"))).toBe("g");
    expect(adoptedGroupId(pane("a", "outro"), pane("b", "g"))).toBe("g");
  });

  it("soltar sobre painel sem grupo, ou já no mesmo grupo, não muda nada", () => {
    expect(adoptedGroupId(pane("a", "g"), pane("b"))).toBeUndefined();
    expect(adoptedGroupId(pane("a", "g"), pane("b", "g"))).toBeUndefined();
  });
});
