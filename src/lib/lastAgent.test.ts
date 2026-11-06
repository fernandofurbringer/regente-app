import { describe, expect, it } from "vitest";
import { parseMission, parsePicks } from "./lastAgent";

describe("parsePicks", () => {
  it("sem valor ou texto inválido vira vazio", () => {
    expect(parsePicks(null)).toEqual({});
    expect(parsePicks("{não é json")).toEqual({});
    expect(parsePicks("[1,2]")).toEqual({});
    expect(parsePicks("\"texto\"")).toEqual({});
  });

  it("guarda modelo e esforço por CLI; vazio vira null", () => {
    const raw = JSON.stringify({ claude: { model: "opus", effort: "high" }, codex: { model: "", effort: null }, gemini: 5 });
    expect(parsePicks(raw)).toEqual({
      claude: { model: "opus", effort: "high" },
      codex: { model: null, effort: null },
    });
  });
});


describe("parseMission", () => {
  it("sem valor ou texto inválido vira null", () => {
    expect(parseMission(null)).toBeNull();
    expect(parseMission("{quebrado")).toBeNull();
  });

  it("campos fora do esperado caem no padrão e a contagem fica entre 1 e 64", () => {
    expect(parseMission(JSON.stringify({ mode: "xyz", count: 500, worktree: "sim", template: [1] }))).toEqual({ mode: "livre", count: 64, worktree: false, template: {} });
    expect(parseMission(JSON.stringify({ mode: "agentico", count: 0, worktree: true, template: { cli: "claude" } }))).toEqual({ mode: "agentico", count: 1, worktree: true, template: { cli: "claude" } });
  });
});
