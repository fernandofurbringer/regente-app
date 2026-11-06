import { describe, expect, it } from "vitest";
import { filterByQuery, fuzzyScore } from "./fuzzy";

describe("busca difusa da paleta", () => {
  it("exige as letras na ordem, sem acento nem caixa", () => {
    expect(fuzzyScore("nm", "Nova missão")).not.toBeNull();
    expect(fuzzyScore("MISSAO", "Nova missão")).not.toBeNull();
    expect(fuzzyScore("ba", "ab")).toBeNull();
    expect(fuzzyScore("xyz", "Nova missão")).toBeNull();
  });

  it("busca vazia casa com tudo", () => {
    expect(fuzzyScore("   ", "qualquer")).toBe(0);
  });

  it("início de palavra e letras seguidas ganham de letras espalhadas", () => {
    const seguido = fuzzyScore("pain", "Novo painel")!;
    const espalhado = fuzzyScore("pain", "epaixxxxxxxxxn")!;
    expect(seguido).toBeGreaterThan(espalhado);
    expect(fuzzyScore("np", "Novo painel")!).toBeGreaterThan(fuzzyScore("np", "Lenp")!);
  });

  it("filterByQuery ordena pela nota e mantém a ordem original nos empates", () => {
    const items = ["Configurações", "Novo painel", "Painel anterior", "Próximo painel"];
    expect(filterByQuery(items, "", (s) => s)).toEqual(items);
    // "Painel anterior" casa em sequência desde o início; "Configurações" não tem a letra p
    const achados = filterByQuery(items, "painel", (s) => s);
    expect(achados[0]).toBe("Painel anterior");
    expect(achados).toHaveLength(3);
    expect(achados).not.toContain("Configurações");
    expect(filterByQuery(items, "zzz", (s) => s)).toEqual([]);
  });
});
