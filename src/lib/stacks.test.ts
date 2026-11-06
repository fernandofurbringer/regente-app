import { describe, expect, it } from "vitest";
import type { PaneRow, RecipeDef } from "./ipc";
import { formFromStack, isSingleStack, isStack, slotFromStack, stackFromForm, stackFromPanes, stackMissionInput } from "./stacks";

const pane = (p: Partial<PaneRow>): PaneRow => ({ id: "p", workspaceId: "w", kind: "agent", name: "Revisor", nameManual: true, cli: "claude", model: "opus", effort: "high", permission: "auto", account: null, closedAt: null, ...p } as PaneRow);

describe("stacks", () => {
  it("guarda só os agentes abertos, sem prompt e sem a permissão danger", () => {
    const r = stackFromPanes([
      pane({ id: "a", permission: "danger" }),
      pane({ id: "b", kind: "terminal", cli: null }),
      pane({ id: "c", closedAt: 123 }),
      pane({ id: "d", name: "Testes", model: null, account: "trabalho" }),
    ], "Minha stack", 42);
    expect(r.id).toBe("stack-42");
    expect(r.mode).toBe("livre");
    expect(r.slots).toHaveLength(2);
    expect(r.slots[0]).toEqual({ cli: "claude", model: "opus", effort: "high", permission: "pend", name: "Revisor" });
    expect(r.slots[1]).toEqual({ cli: "claude", name: "Testes", permission: "auto", effort: "high", account: "trabalho" });
    expect(r.slots.every((s) => !("prompt" in s))).toBe(true);
    expect(isStack(r)).toBe(true);
  });

  it("nome automático não entra na stack (o painel aberto volta a nomear pelo pedido)", () => {
    const r = stackFromPanes([pane({ id: "a", name: "Revisar login", nameManual: false })], "S", 1);
    expect(r.slots[0].name).toBeUndefined();
    expect(r.slots[0]).not.toHaveProperty("name", "Revisar login");
  });

  it("abrir a stack vira uma missão livre no workspace", () => {
    const r: RecipeDef = { ...stackFromPanes([pane({})], "S", 1) };
    const input = stackMissionInput(r, { id: "w1", path: "D:/x" });
    expect(input).toMatchObject({ workspaceId: "w1", mode: "livre", worktree: false });
    expect(input.slots?.[0]).toMatchObject({ kind: "agent", workspaceId: "w1", cwd: "D:/x", cli: "claude" });
  });

  it("receitas comuns não são stacks", () => {
    expect(isStack({ id: "receita-x", mode: "livre" } as RecipeDef)).toBe(false);
  });
});

describe("stack de um painel (Catálogo)", () => {
  const form = { name: " Revisor forte ", cli: "claude", model: "opus", effort: "high", agent: "sq-seguranca", skill: "revisar", permission: "plan" as const };

  it("o formulário vira uma receita livre de um slot com agente e skill", () => {
    const r = stackFromForm(form, "stack-9");
    expect(r).toMatchObject({ id: "stack-9", name: "Revisor forte", mode: "livre", source: "regente" });
    expect(r.slots).toEqual([{ cli: "claude", permission: "plan", model: "opus", effort: "high", agent: "sq-seguranca", allowedSkills: ["revisar"] }]);
    expect(r.description).toBe("sq-seguranca · revisar · opus · high");
    expect(isStack(r) && isSingleStack(r)).toBe(true);
  });

  it("campos vazios ficam de fora (padrão do CLI e do agente)", () => {
    const r = stackFromForm({ ...form, model: "  ", effort: "", agent: "", skill: "" }, "stack-1");
    expect(r.slots[0]).toEqual({ cli: "claude", permission: "plan" });
  });

  it("ida e volta: receita -> formulário -> receita", () => {
    const r = stackFromForm(form, "stack-9");
    expect(formFromStack(r)).toEqual({ ...form, name: "Revisor forte" });
  });

  it("o slot do Novo painel traz agente, skills, modelo e esforço", () => {
    const slot = slotFromStack(stackFromForm(form, "stack-9"));
    expect(slot).toEqual({ kind: "agent", cli: "claude", permission: "plan", model: "opus", effort: "high", agent: "sq-seguranca", allowedSkills: ["revisar"] });
    expect(slotFromStack(stackFromForm({ ...form, skill: "", agent: "", model: "", effort: "" }, "stack-2"))).toMatchObject({ model: null, effort: null, agent: null, allowedSkills: null });
  });

  it("stack de vários painéis e receita comum não servem para o Novo painel", () => {
    const multi = stackFromPanes([pane({ id: "a" }), pane({ id: "b" })], "S", 5);
    expect(isSingleStack(multi)).toBe(false);
    expect(slotFromStack(multi)).toBeNull();
    expect(isSingleStack({ id: "receita-x", mode: "livre", slots: [{}] } as RecipeDef)).toBe(false);
  });

  it("permissão desconhecida volta para o padrão do CLI", () => {
    const r = { ...stackFromForm(form, "stack-9"), slots: [{ cli: "claude", permission: "danger" }] };
    expect(formFromStack(r).permission).toBe("pend");
  });
});
