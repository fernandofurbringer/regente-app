import { describe, expect, it } from "vitest";
import { isOrchestrator, roleLabel } from "./roles";

describe("roleLabel", () => {
  it("rótulos fixos dos papéis", () => {
    expect(roleLabel("piloto")).toBe("PILOTO");
    expect(roleLabel("ajudante")).toBe("AJUDANTE");
    expect(roleLabel("maestro")).toBe("MAESTRO");
  });
  it("chefe mostra a função em caixa alta", () => {
    expect(roleLabel("chefe:builder")).toBe("CHEFE · BUILDER");
    expect(roleLabel("chefe:reviewer")).toBe("CHEFE · REVIEWER");
  });
  it("agente usa o nome do agente", () => {
    expect(roleLabel("agente", "designer")).toBe("DESIGNER");
    expect(roleLabel("agente", null)).toBe("");
  });
  it("sem papel = vazio", () => {
    expect(roleLabel(null)).toBe("");
    expect(roleLabel(null, "designer")).toBe("");
  });
});

describe("isOrchestrator", () => {
  it("só piloto e maestro", () => {
    expect(isOrchestrator("piloto")).toBe(true);
    expect(isOrchestrator("maestro")).toBe(true);
    expect(isOrchestrator("ajudante")).toBe(false);
    expect(isOrchestrator("chefe:builder")).toBe(false);
    expect(isOrchestrator(null)).toBe(false);
  });
});
