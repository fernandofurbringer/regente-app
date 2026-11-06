import { describe, expect, it } from "vitest";
import { markRecovery, takeRecovery } from "./recovery";

/** Storage de mentira: a mesma API que a sessionStorage usa. */
function memory() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v), removeItem: (k: string) => void m.delete(k) };
}

describe("aviso de recuperação da interface", () => {
  it("só aparece uma vez depois de marcado", () => {
    const s = memory();
    expect(takeRecovery(s)).toBe(false);
    markRecovery(s);
    expect(takeRecovery(s)).toBe(true);
    expect(takeRecovery(s)).toBe(false);
  });

  it("sem armazenamento disponível nunca falha", () => {
    expect(takeRecovery(undefined)).toBe(false);
    expect(() => markRecovery(undefined)).not.toThrow();
    const broken = { getItem: () => { throw new Error("bloqueado"); }, setItem: () => { throw new Error("bloqueado"); }, removeItem: () => {} };
    expect(takeRecovery(broken)).toBe(false);
    expect(() => markRecovery(broken)).not.toThrow();
  });
});
