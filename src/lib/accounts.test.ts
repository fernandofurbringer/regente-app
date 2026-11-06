import { describe, expect, it } from "vitest";
import { byHeadroom, otherAccounts, showAccountBadge, totalAccounts } from "./accounts";

describe("selo de conta", () => {
  it("conta o total com a principal implícita", () => {
    expect(totalAccounts(undefined)).toBe(1);
    expect(totalAccounts([])).toBe(1);
    expect(totalAccounts(["trabalho"])).toBe(2);
  });

  it("não aparece com uma conta só", () => {
    expect(showAccountBadge(undefined, null)).toBe(false);
    expect(showAccountBadge([], "principal")).toBe(false);
  });

  it("aparece com 2 ou mais contas, na principal e na extra", () => {
    expect(showAccountBadge(["trabalho"], null)).toBe(true);
    expect(showAccountBadge(["trabalho"], "principal")).toBe(true);
    expect(showAccountBadge(["trabalho", "casa"], "casa")).toBe(true);
  });

  it("conta removida não deixa selo", () => {
    expect(showAccountBadge(["trabalho"], "antiga")).toBe(false);
  });
});

describe("continuar em outra conta", () => {
  it("lista as outras contas, com a principal implícita", () => {
    expect(otherAccounts(["fernando2"], null)).toEqual(["fernando2"]);
    expect(otherAccounts(["fernando2"], "principal")).toEqual(["fernando2"]);
    expect(otherAccounts(["fernando", "fernando2"], "fernando")).toEqual(["principal", "fernando2"]);
    expect(otherAccounts(undefined, null)).toEqual([]);
  });

  it("ordena da mais folgada para a mais usada; sem leitura fica no meio", () => {
    const uso: Record<string, number | null> = { a: 95, b: 10, c: null };
    expect(byHeadroom(["a", "b", "c"], (x) => uso[x] ?? null)).toEqual(["b", "c", "a"]);
    expect(byHeadroom(["x", "y"], () => null)).toEqual(["x", "y"]);
  });
});
