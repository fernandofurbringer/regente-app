import { describe, expect, it } from "vitest";
import { paraFechar, vizinha } from "./editorTabs";

describe("vizinha", () => {
  const ps = ["a", "b", "c"];
  it("anda uma aba e dá a volta nas pontas", () => {
    expect(vizinha(ps, "a", 1)).toBe("b");
    expect(vizinha(ps, "c", 1)).toBe("a");
    expect(vizinha(ps, "a", -1)).toBe("c");
  });
  it("sem ativa vai para a primeira (ou a última, para trás); sem abas, null", () => {
    expect(vizinha(ps, null, 1)).toBe("a");
    expect(vizinha(ps, "x", -1)).toBe("c");
    expect(vizinha([], "a", 1)).toBeNull();
  });
});

describe("paraFechar", () => {
  const docs = [{ path: "a", dirty: false }, { path: "b", dirty: true }, { path: "c", dirty: false }];
  it("outros: todas menos a do menu, na ordem das abas", () => {
    expect(paraFechar("outros", docs, "b")).toEqual(["a", "c"]);
  });
  it("salvos: só as sem alterações", () => {
    expect(paraFechar("salvos", docs, "a")).toEqual(["a", "c"]);
  });
  it("todos: todas", () => {
    expect(paraFechar("todos", docs, "a")).toEqual(["a", "b", "c"]);
  });
});
