import { describe, expect, it } from "vitest";
import { branchOptions, canCommit, sourceNote, syncLabel } from "./gitflow";

describe("commit e PR", () => {
  it("só commita com mensagem e arquivo marcado", () => {
    expect(canCommit("  ", ["a.ts"])).toBe(false);
    expect(canCommit("feat: x", [])).toBe(false);
    expect(canCommit("feat: x", ["a.ts"])).toBe(true);
  });

  it("explica de onde veio a sugestão", () => {
    expect(sourceNote("ia", null)).toMatch(/IA/);
    expect(sourceNote("modelo", null)).toMatch(/sem chave/);
    expect(sourceNote("modelo", "timeout")).toMatch(/timeout/);
  });
});

describe("branch e sincronizar", () => {
  const b = { current: "main", local: ["main", "feat/login"], remote: ["origin/fix-x"], upstream: "origin/main", ahead: 0, behind: 0, fetchError: null };

  it("filtra locais e remotas e oferece criar o nome que não existe", () => {
    expect(branchOptions(b, "")).toEqual([{ name: "main", kind: "local" }, { name: "feat/login", kind: "local" }, { name: "origin/fix-x", kind: "remota" }]);
    expect(branchOptions(b, "LOG")).toEqual([{ name: "feat/login", kind: "local" }, { name: "LOG", kind: "nova" }]);
    expect(branchOptions(b, "feat/login").map((o) => o.kind)).toEqual(["local"]);
    // o nome sem o remoto já existe como remota: não cria outra
    expect(branchOptions(b, "fix-x")).toEqual([{ name: "origin/fix-x", kind: "remota" }]);
    expect(branchOptions(b, "a b")).toEqual([]);
  });

  it("o botão diz o que entra e sai, ou publicar", () => {
    expect(syncLabel({ upstream: null, ahead: 3, behind: 0 })).toBe("Publicar");
    expect(syncLabel({ upstream: "origin/main", ahead: 0, behind: 0 })).toBe("Sincronizar");
    expect(syncLabel({ upstream: "origin/main", ahead: 1, behind: 2 })).toBe("Sincronizar ↓2 ↑1");
  });
});

describe("evento git por workspace", () => {
  it("só a aba do workspace relê; evento sem workspace vale para todas", async () => {
    const { gitChanged, gitChangedOf } = await import("./gitflow");
    expect(gitChangedOf(gitChanged("a"), "a")).toBe(true);
    expect(gitChangedOf(gitChanged("a"), "b")).toBe(false);
    expect(gitChangedOf(new Event("regente:git"), "b")).toBe(true);
  });
});
