import { describe, expect, it } from "vitest";
import { pendingNoticeToast } from "./notices";

const nomes: Record<string, string> = { a: "Claude 1", b: "Claude 2", c: "Codex" };
const nameOf = (id: string) => nomes[id];

describe("toast dos avisos pendentes", () => {
  it("um painel: vai para ele", () => {
    expect(pendingNoticeToast(["a"], nameOf)).toEqual({ text: "Claude 1 avisou enquanto o Regente estava fora de foco.", go: { id: "a", label: "Ir para o painel" } });
  });
  it("vários: lista todos, botão para o mais novo e o anterior", () => {
    const t = pendingNoticeToast(["a", "b", "c"], nameOf)!;
    expect(t.text).toContain("3 painéis avisaram");
    expect(t.text).toContain("Codex, Claude 2, Claude 1");
    expect(t.go).toEqual({ id: "c", label: "Ir para Codex" });
    expect(t.alt).toEqual({ id: "b", label: "Ir para Claude 2" });
  });
  it("painel fechado sai; nenhum vivo, sem toast", () => {
    expect(pendingNoticeToast(["x", "a"], nameOf)?.go.id).toBe("a");
    expect(pendingNoticeToast(["x"], nameOf)).toBeNull();
  });
});
