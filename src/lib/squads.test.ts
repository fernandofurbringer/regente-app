import { describe, expect, it } from "vitest";
import type { FormerPane, SquadDef } from "./ipc";
import { copyId, gateCount, squadFormerPrompt, stepsToText, textToSteps } from "./squads";
import { intakeToPairs, pairsToIntake } from "./recipes";

describe("passos de fase", () => {
  it("um passo por linha, ida e volta", () => {
    expect(stepsToText(["desenhar", "codar"])).toBe("desenhar\ncodar");
    expect(textToSteps(" desenhar \n\n codar \n")).toEqual(["desenhar", "codar"]);
    expect(textToSteps("")).toEqual([]);
  });
});

describe("copyId", () => {
  it("usa -meu e numera quando já existe", () => {
    expect(copyId("jogo-web", ["jogo-web"])).toBe("jogo-web-meu");
    expect(copyId("jogo-web", ["jogo-web", "jogo-web-meu"])).toBe("jogo-web-meu-2");
    expect(copyId("jogo-web", ["jogo-web", "jogo-web-meu", "jogo-web-meu-2"])).toBe("jogo-web-meu-3");
  });
});

describe("gateCount", () => {
  it("conta fases com título de gate", () => {
    const s = { phases: [{ name: "a", steps: [], gate: "Gate 1" }, { name: "b", steps: [], gate: null }, { name: "c", steps: [], gate: "Gate 2" }] } as Pick<SquadDef, "phases">;
    expect(gateCount(s)).toBe(2);
  });
});

describe("squadFormerPrompt", () => {
  it("traz a instrução e o JSON cercado", () => {
    const panes: FormerPane[] = [{ name: "p", cli: "claude", model: null, effort: null, role: null, agent: null, costUsd: 0.5, durationMin: 3 }];
    const p = squadFormerPrompt(panes);
    expect(p.startsWith("Analise as 20 sessões recentes abaixo")).toBe(true);
    expect(p).toContain("squad_save");
    expect(p).toContain("```json\n[");
    expect(p).toContain('"durationMin": 3');
    expect(p.endsWith("```")).toBe(true);
  });
});

describe("intake de receita", () => {
  it("ida e volta, sem perguntas vazias", () => {
    const pairs = intakeToPairs({ "URL do site": "https://a.com" });
    expect(pairs).toEqual([{ q: "URL do site", a: "https://a.com" }]);
    expect(pairsToIntake([{ q: " ", a: "x" }, { q: "Cor", a: "" }])).toEqual({ Cor: "" });
  });
});

describe("squadFormerPrompt com crases nos nomes", () => {
  it("nome com crase não fecha o bloco de código", () => {
    const panes: FormerPane[] = [{ name: "a ``` b", cli: null, model: null, effort: null, role: null, agent: null, costUsd: 0, durationMin: 0 }];
    const p = squadFormerPrompt(panes);
    expect(p.split("```")).toHaveLength(3);
    expect(p).toContain("\u0060\u0060\u0060");
  });
});
