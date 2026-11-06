import { describe, expect, it } from "vitest";
import { tipoArquivo } from "./tipoArquivo";

describe("tipoArquivo", () => {
  it("por extensão, sem diferenciar maiúsculas", () => {
    expect(tipoArquivo("Painel.TSX")).toEqual({ rotulo: "TSX", classe: "ts" });
    expect(tipoArquivo("stt.rs").classe).toBe("rs");
  });
  it("nomes especiais vencem a extensão", () => {
    expect(tipoArquivo("package.json").rotulo).toBe("NPM");
    expect(tipoArquivo(".env.local").rotulo).toBe("ENV");
  });
  it("desconhecido e arquivo oculto sem extensão", () => {
    expect(tipoArquivo("Makefile").classe).toBe("outro");
    expect(tipoArquivo(".bashrc").classe).toBe("outro");
  });
});
