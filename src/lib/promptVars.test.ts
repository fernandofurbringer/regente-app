import { describe, expect, it } from "vitest";
import { expandPrompt, promptVarsUsed } from "./promptVars";

describe("expandPrompt", () => {
  it("troca cada variável pelo valor", () => {
    expect(expandPrompt("Arquivo {{arquivo}} em {{workspace}}", { arquivo: "a.ts", workspace: "/w" })).toBe("Arquivo a.ts em /w");
  });

  it("variável sem valor vira vazio e desconhecida fica como está", () => {
    expect(expandPrompt("[{{selecao}}] {{nope}}", {})).toBe("[] {{nope}}");
  });

  it("aceita espaços dentro das chaves", () => {
    expect(expandPrompt("{{ branch }}", { branch: "main" })).toBe("main");
  });

  it("não expande de novo o que foi colado (clipboard com {{arquivo}})", () => {
    expect(expandPrompt("{{clipboard}}", { clipboard: "{{arquivo}} {{selecao}}", arquivo: "x" })).toBe("{{arquivo}} {{selecao}}");
  });

  it("texto sem variável sai igual", () => {
    expect(expandPrompt("sem nada {x}", { arquivo: "a" })).toBe("sem nada {x}");
  });
});

describe("promptVarsUsed", () => {
  it("lista cada variável conhecida uma vez", () => {
    expect(promptVarsUsed("{{clipboard}} {{arquivo}} {{clipboard}} {{x}}").sort()).toEqual(["arquivo", "clipboard"]);
  });
});
