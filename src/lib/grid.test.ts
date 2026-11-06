import { describe, expect, it } from "vitest";
import { moveItem } from "./grid";

describe("grid", () => {
  it("move", () => expect(moveItem(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"]));
});
