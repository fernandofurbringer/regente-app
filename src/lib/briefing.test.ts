import { describe, expect, it } from "vitest";
import { BRIEF_CHIPS, insertChip } from "./briefing";

describe("insertChip", () => {
  it("usa a linha do chip quando o briefing está vazio", () => {
    expect(insertChip("", "Prazo: ")).toBe("Prazo: ");
  });

  it("acrescenta em linha própria e tira o espaço do fim", () => {
    expect(insertChip("Um app de tarefas   \n", "Stack: ")).toBe("Um app de tarefas\nStack: ");
  });

  it("não repete um rótulo que já está no briefing", () => {
    const b = "Objetivo: vender planos\nPrazo: sexta";
    expect(insertChip(b, "Prazo: ")).toBe(b);
  });

  it("reconhece o rótulo com espaço antes dos dois-pontos", () => {
    const b = "Objetivo : vender planos";
    expect(insertChip(b, "Objetivo: ")).toBe(b);
  });

  it("cada chip tem rótulo e linha", () => {
    expect(BRIEF_CHIPS.length).toBe(6);
    for (const c of BRIEF_CHIPS) expect(c.line.length).toBeGreaterThan(0);
  });
});
