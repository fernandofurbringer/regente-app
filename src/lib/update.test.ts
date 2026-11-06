import { describe, expect, it } from "vitest";
import { progressPercent, statusText } from "./update";

describe("atualização: textos e progresso", () => {
  it("descreve cada estado do canal", () => {
    expect(statusText({ state: "nao-configurado", current: "1.0.0", version: null, notes: null })).toContain("Sem canal configurado");
    expect(statusText({ state: "atualizado", current: "1.0.0", version: null, notes: null })).toContain("mais recente");
    expect(statusText({ state: "disponivel", current: "1.0.0", version: "1.1.0", notes: "x" })).toBe("Versão 1.1.0 disponível.");
  });

  it("calcula o percentual do download e não inventa quando o tamanho é desconhecido", () => {
    expect(progressPercent(50, 200)).toBe(25);
    expect(progressPercent(300, 200)).toBe(100);
    expect(progressPercent(10, null)).toBeNull();
    expect(progressPercent(10, 0)).toBeNull();
  });
});
