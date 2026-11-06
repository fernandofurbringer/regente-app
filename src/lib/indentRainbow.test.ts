import { describe, expect, it } from "vitest";
import { faixasIndentacao } from "./indentRainbow";

describe("faixasIndentacao", () => {
  it("dois níveis de 2 espaços", () => {
    expect(faixasIndentacao("    x", 2)).toEqual([{ inicio: 1, fim: 3, nivel: 0 }, { inicio: 3, fim: 5, nivel: 1 }]);
  });
  it("tab conta como um nível", () => {
    expect(faixasIndentacao("\t\tx", 4)).toEqual([{ inicio: 1, fim: 2, nivel: 0 }, { inicio: 2, fim: 3, nivel: 1 }]);
  });
  it("sobra de espaços não ganha faixa; sem indentação, nada", () => {
    expect(faixasIndentacao("   x", 2)).toEqual([{ inicio: 1, fim: 3, nivel: 0 }]);
    expect(faixasIndentacao("x", 2)).toEqual([]);
  });
  it("as cores voltam ao começo depois do quarto nível", () => {
    expect(faixasIndentacao(" ".repeat(10) + "x", 2).map((f) => f.nivel)).toEqual([0, 1, 2, 3, 0]);
  });
});
