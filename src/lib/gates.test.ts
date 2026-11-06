import { describe, expect, it } from "vitest";
import { isAdjust, isTyping, optionLabel, parseOptions } from "./gates";

describe("parseOptions", () => {
  it("lê a lista de opções", () => {
    expect(parseOptions('["aprovar","ajustar"]')).toEqual(["aprovar", "ajustar"]);
  });
  it("ausente, inválido ou não-lista = vazio", () => {
    expect(parseOptions(null)).toEqual([]);
    expect(parseOptions("{não é json")).toEqual([]);
    expect(parseOptions('{"a":1}')).toEqual([]);
  });
  it("descarta itens que não são texto", () => {
    expect(parseOptions('["aprovar",3,null]')).toEqual(["aprovar"]);
  });
});

describe("isAdjust / optionLabel", () => {
  it("opção de ajuste pede observação", () => {
    expect(isAdjust("ajustar")).toBe(true);
    expect(isAdjust("Pedir ajuste")).toBe(true);
    expect(isAdjust("aprovar")).toBe(false);
  });
  it("rótulo com inicial maiúscula", () => {
    expect(optionLabel("aprovar")).toBe("Aprovar");
    expect(optionLabel("")).toBe("");
  });
});

describe("isTyping (gate não rouba o foco de quem digita)", () => {
  it("pergunta por campos, terminal e editor; sem ancestral com eles, é falso", () => {
    const seen: string[] = [];
    const fromTerminal = { closest: (sel: string) => { seen.push(sel); return sel.includes(".xterm") ? {} : null; } };
    expect(isTyping(fromTerminal as unknown as Element)).toBe(true);
    for (const t of ["input", "textarea", "select", ".xterm", ".monaco-editor"]) expect(seen[0]).toContain(t);
    expect(isTyping({ closest: () => null } as unknown as Element)).toBe(false);
    expect(isTyping(null)).toBe(false);
  });
});
