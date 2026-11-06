import { describe, expect, it } from "vitest";
import { frameDetail, hibernatedAnnouncement } from "./frameText";

describe("quadro congelado para leitor de tela", () => {
  it("o detalhe traz as últimas linhas e ignora espaços do fim", () => {
    const frame = "linha 1\nlinha 2\nlinha 3\n\n   \n";
    expect(frameDetail(frame, 2)).toBe("Última tela: linha 2\nlinha 3");
  });

  it("sem texto, o detalhe avisa em vez de ficar vazio", () => {
    expect(frameDetail("   \n")).toBe("A última tela não tem texto.");
  });

  it("anuncia uma frase curta só quando o painel passa a hibernado", () => {
    expect(hibernatedAnnouncement("trabalhando", "hibernado", "Ana")).toBe("Painel Ana hibernado");
  });

  it("não anuncia na montagem, na restauração ou em outros estados", () => {
    expect(hibernatedAnnouncement("hibernado", "hibernado", "Ana")).toBe("");
    expect(hibernatedAnnouncement("hibernado", "acordando", "Ana")).toBe("");
    expect(hibernatedAnnouncement("hibernado", "ocioso", "Ana")).toBe("");
    expect(hibernatedAnnouncement(undefined, "hibernado", "Ana")).toBe("");
  });
});
