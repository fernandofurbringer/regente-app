import { afterEach, describe, expect, it, vi } from "vitest";
import { absOf, acceptedOf, langOf, reviewKeyAction, revertWarning, sameFile, storeAccepted, unreviewed } from "./review";

const file = (path: string, status: "A" | "M" | "D", fingerprint: string) => ({ path, status, add: 1, del: 0, fingerprint });

describe("revisão de missão", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("escolhe a linguagem do Monaco pela extensão", () => {
    expect(langOf("src/App.tsx")).toBe("typescript");
    expect(langOf("Cargo.toml")).toBe("ini");
    expect(langOf("LICENSE")).toBe("plaintext");
  });

  it("lista os arquivos ainda não aceitos com este conteúdo (mudou, volta a pendente)", () => {
    const files = [file("a.ts", "M", "h1"), file("b.ts", "A", "h2")];
    expect(unreviewed(files, { "a.ts": "h1" }).map((f) => f.path)).toEqual(["b.ts"]);
    expect(unreviewed(files, { "a.ts": "antigo" }).map((f) => f.path)).toEqual(["a.ts", "b.ts"]);
    expect(unreviewed(files, {})).toHaveLength(2);
  });

  it("guarda os aceitos por missão e lê de volta; sem armazenamento não quebra", () => {
    const mem = new Map<string, string>();
    vi.stubGlobal("localStorage", { getItem: (k: string) => mem.get(k) ?? null, setItem: (k: string, v: string) => { mem.set(k, v); } });
    storeAccepted("m1", { "a.ts": "h1" });
    expect(acceptedOf("m1")).toEqual({ "a.ts": "h1" });
    expect(acceptedOf("m2")).toEqual({});
    vi.stubGlobal("localStorage", { getItem: () => "{não é json", setItem: () => { throw new Error("cheio"); } });
    expect(acceptedOf("m1")).toEqual({});
    expect(() => storeAccepted("m1", { x: "y" })).not.toThrow();
  });

  it("mapeia as teclas da revisão (j/k/a/r), sem diferença de caixa", () => {
    expect(reviewKeyAction("j")).toBe("next");
    expect(reviewKeyAction("K")).toBe("prev");
    expect(reviewKeyAction("a")).toBe("accept");
    expect(reviewKeyAction("r")).toBe("revert");
    expect(reviewKeyAction("x")).toBeNull();
    expect(reviewKeyAction("constructor")).toBeNull();
  });

  it("monta o caminho absoluto no worktree e compara arquivos sem diferenciar caixa", () => {
    expect(absOf("D:\\w\\missao\\", "src/a.ts")).toBe("D:\\w\\missao\\src\\a.ts");
    expect(sameFile("D:/W/a.ts", "d:\\w\\A.TS")).toBe(true);
    expect(sameFile("D:/W/a.ts", "d:\\w\\b.ts")).toBe(false);
  });

  it("avisa o que a reversão faz com arquivo novo", () => {
    expect(revertWarning(file("x", "A", ""))).toMatch(/apagado/);
    expect(revertWarning(file("x", "M", ""))).toMatch(/revert/);
  });
});
