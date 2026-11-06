import { describe, expect, it } from "vitest";
import { clampIdleMinutes, clampResidentFloor } from "./hibernate";

describe("limites da hibernação", () => {
  it("minutos entre 1 e 1440, inteiros", () => {
    expect(clampIdleMinutes(0)).toBe(1);
    expect(clampIdleMinutes(5000)).toBe(1440);
    expect(clampIdleMinutes(7.6)).toBe(8);
    expect(clampIdleMinutes(Number.NaN)).toBe(10);
  });
  it("painéis residentes entre 0 e 16", () => {
    expect(clampResidentFloor(-3)).toBe(0);
    expect(clampResidentFloor(99)).toBe(16);
    expect(clampResidentFloor(Number.NaN)).toBe(2);
  });
});
