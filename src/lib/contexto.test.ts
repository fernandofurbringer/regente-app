import { describe, expect, it } from "vitest";
import { faixaContexto, rotuloProposta, textoContexto } from "./contexto";

describe("medidor de contexto", () => {
  it("cores por faixa: verde < 50, âmbar 50-75, vermelho acima de 75", () => {
    expect([faixaContexto(10), faixaContexto(49.9), faixaContexto(50), faixaContexto(75), faixaContexto(75.1)]).toEqual(["bom", "bom", "medio", "medio", "ruim"]);
  });
  it("texto com tokens e janela, e só a porcentagem sem eles", () => {
    expect(textoContexto(62.4, 124_000, 200_000)).toBe("Contexto 62% (124 mil de 200 mil tokens)");
    expect(textoContexto(18, null, null)).toBe("Contexto 18%");
    expect(textoContexto(140, null, 200_000)).toBe("Contexto 100%");
  });
});

describe("chip de proposta de compactação", () => {
  it("rótulo por ação; ação desconhecida ou ausente não mostra chip", () => {
    expect(rotuloProposta("compactar")).toBe("Compactar");
    expect(rotuloProposta("recomecar")).toBe("Recomeçar com memória");
    expect(rotuloProposta("outra")).toBeNull();
    expect(rotuloProposta(null)).toBeNull();
  });
});
