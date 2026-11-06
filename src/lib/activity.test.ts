import { describe, expect, it } from "vitest";
import type { ExecRow, PaneRow } from "./ipc";
import { activitySummary, isInlineSubagent } from "./activity";

const sub = (status: string, name = "↳ Explore"): Pick<PaneRow, "status" | "name"> => ({ status: status as PaneRow["status"], name });
const ex = (status: ExecRow["status"], command: string, startedAt: number, endedAt: number | null = null): Pick<ExecRow, "status" | "command" | "description" | "startedAt" | "endedAt"> =>
  ({ status, command, description: null, startedAt, endedAt });
const NOW = 1_000_000;

describe("activitySummary", () => {
  it("nada: null (a faixa não aparece)", () => {
    expect(activitySummary([], [], NOW)).toBeNull();
  });
  it("subtarefas e o comando em andamento mais antigo, com o tempo", () => {
    const s = activitySummary([sub("trabalhando"), sub("concluido")], [ex("ok", "npm i", 0, 10), ex("rodando", "dotnet test", NOW - 42_000)], NOW);
    expect(s).toBe("↳ 2 subtarefas (1 rodando) · ⚙ dotnet test · 42 s");
  });
  it("descrição vazia usa o comando; quebras de linha viram espaço", () => {
    const e = { ...ex("rodando", "npm\n  test", NOW - 5_000), description: "  " };
    expect(activitySummary([], [e], NOW)).toBe("⚙ npm test · 5 s");
  });
  it("singular; comando concluído não aparece no resumo; comando longo é cortado", () => {
    expect(activitySummary([sub("concluido")], [ex("ok", "npm i", 0, 10)], NOW)).toBe("↳ 1 subtarefa · ⚙ 1 comando");
    const longo = "x".repeat(80);
    expect(activitySummary([], [ex("fundo", longo, NOW - 3_600_000)], NOW)).toBe(`⚙ ${"x".repeat(40)}… · 60 min`);
  });
});

describe("isInlineSubagent", () => {
  it("só subagente e só quando o ajuste é inline", () => {
    expect(isInlineSubagent({ kind: "subagent" }, false)).toBe(true);
    expect(isInlineSubagent({ kind: "subagent" }, true)).toBe(false);
    expect(isInlineSubagent({ kind: "agent" }, false)).toBe(false);
  });
});
