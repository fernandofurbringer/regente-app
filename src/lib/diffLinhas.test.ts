import { describe, expect, it } from "vitest";
import { diffLinhas } from "./diffLinhas";

describe("diffLinhas", () => {
  it("sem mudança, sem marcas", () => {
    expect(diffLinhas("a\nb", "a\nb")).toEqual({ novas: [], alteradas: [], removidasApos: [] });
  });
  it("linha inserida no meio é nova", () => {
    expect(diffLinhas("a\nc", "a\nb\nc")).toEqual({ novas: [2], alteradas: [], removidasApos: [] });
  });
  it("linha trocada é alterada", () => {
    expect(diffLinhas("a\nb\nc", "a\nB\nc")).toEqual({ novas: [], alteradas: [2], removidasApos: [] });
  });
  it("linha apagada marca a anterior; no topo marca 0", () => {
    expect(diffLinhas("a\nb\nc", "a\nc").removidasApos).toEqual([1]);
    expect(diffLinhas("a\nb", "b").removidasApos).toEqual([0]);
  });
  it("CRLF e LF são a mesma coisa", () => {
    expect(diffLinhas("a\r\nb", "a\nb")).toEqual({ novas: [], alteradas: [], removidasApos: [] });
  });
  it("troca de 1 por 3 linhas: 1 alterada e 2 novas", () => {
    expect(diffLinhas("a\nx\nz", "a\n1\n2\n3\nz")).toEqual({ novas: [3, 4], alteradas: [2], removidasApos: [] });
  });
});
