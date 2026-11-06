import { describe, expect, it } from "vitest";
import { bytesUtf8, indentTexto, posTexto } from "./editorStatus";

describe("posTexto", () => {
  it("posição, e a seleção quando há", () => {
    expect(posTexto({ ln: 12, col: 5, sel: 0, selLinhas: 0 })).toBe("Ln 12, Col 5");
    expect(posTexto({ ln: 12, col: 5, sel: 3, selLinhas: 1 })).toBe("Ln 12, Col 5 (3 selecionados)");
    expect(posTexto({ ln: 12, col: 5, sel: 40, selLinhas: 3 })).toBe("Ln 12, Col 5 (40 selecionados em 3 linhas)");
  });
});

describe("indentTexto", () => {
  it("espaços ou tabulação, como o VS Code", () => {
    expect(indentTexto({ tabSize: 2, insertSpaces: true })).toBe("Espaços: 2");
    expect(indentTexto({ tabSize: 4, insertSpaces: false })).toBe("Tamanho da tabulação: 4");
  });
});

describe("bytesUtf8", () => {
  it("conta bytes em UTF-8, não caracteres", () => {
    expect(bytesUtf8("ação")).toBe(6);
    expect(bytesUtf8("a\r\nb")).toBe(4);
  });
});
