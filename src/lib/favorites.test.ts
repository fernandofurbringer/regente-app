import { describe, expect, it } from "vitest";
import { favKey, sortFavFirst, toggleFav } from "./favorites";

describe("favoritos", () => {
  it("monta a chave com o tipo", () => {
    expect(favKey("agent", "revisor")).toBe("agent:revisor");
    expect(favKey("recipe", "stack-1")).toBe("recipe:stack-1");
  });

  it("liga e desliga sem repetir", () => {
    expect(toggleFav([], "skill:x")).toEqual(["skill:x"]);
    expect(toggleFav(["skill:x", "agent:a"], "skill:x")).toEqual(["agent:a"]);
  });

  it("põe os favoritos primeiro e preserva a ordem em cada grupo", () => {
    const items = ["b", "a", "d", "c"];
    expect(sortFavFirst(items, (s) => s, ["c", "a"])).toEqual(["a", "c", "b", "d"]);
    expect(sortFavFirst(items, (s) => s, [])).toEqual(items);
  });
});
