import { describe, expect, it } from "vitest";
import { accountChoices, accountOption, spawnAccount } from "./lastAccount";

describe("conta antes de abrir", () => {
  it("só oferece escolha com 2 ou mais contas, da mais folgada para a mais usada", () => {
    expect(accountChoices(undefined, () => null)).toEqual([]);
    expect(accountChoices([], () => null)).toEqual([]);
    const uso: Record<string, number> = { principal: 90, b: 10 };
    expect(accountChoices(["b"], (c) => uso[c] ?? null)).toEqual(["b", "principal"]);
  });
  it("rótulo com o uso quando há leitura", () => {
    expect(accountOption("b", 42.4)).toBe("b (42% usado)");
    expect(accountOption("b", null)).toBe("b");
  });
  it("principal vai como null no spawn", () => {
    expect(spawnAccount("principal")).toBeNull();
    expect(spawnAccount(undefined)).toBeNull();
    expect(spawnAccount("fernando2")).toBe("fernando2");
  });
});
