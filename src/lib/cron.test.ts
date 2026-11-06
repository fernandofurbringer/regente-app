import { describe, expect, it } from "vitest";
import { buildExpr, parseExpr, parseTime, weekdayLabel } from "./cron";
import { filterMemories } from "./memory";
import type { MemoryRow } from "./ipc";

describe("buildExpr", () => {
  it("diário", () => {
    expect(buildExpr("daily", "09:30", [], 1)).toBe("30 9 * * *");
  });
  it("semanal: números padrão, ordenados, sem repetir", () => {
    expect(buildExpr("weekly", "08:00", [5, 1, 1, 0], 1)).toBe("0 8 * * 0,1,5");
  });
  it("mensal", () => {
    expect(buildExpr("monthly", "07:15", [], 15)).toBe("15 7 15 * *");
  });
  it("erros em pt-BR", () => {
    expect(() => buildExpr("daily", "25:00", [], 1)).toThrow(/hora/);
    expect(() => buildExpr("weekly", "09:00", [], 1)).toThrow(/dia da semana/);
    expect(() => buildExpr("monthly", "09:00", [], 32)).toThrow(/dia do mês/);
  });
});

describe("parseTime", () => {
  it("aceita HH:MM e recusa o resto", () => {
    expect(parseTime("9:05")).toEqual({ h: 9, m: 5 });
    expect(parseTime("24:00")).toBeNull();
    expect(parseTime("12h")).toBeNull();
  });
});

describe("parseExpr (edição de um agendamento salvo)", () => {
  it("reconhece as três frequências", () => {
    expect(parseExpr("30 9 * * *")).toMatchObject({ freq: "daily", time: "09:30" });
    expect(parseExpr("0 8 * * 0,1,5")).toMatchObject({ freq: "weekly", time: "08:00", days: [0, 1, 5] });
    expect(parseExpr("15 7 15 * *")).toMatchObject({ freq: "monthly", time: "07:15", monthDay: 15 });
  });
  it("domingo como 7 vira 0", () => {
    expect(parseExpr("0 8 * * 7")).toMatchObject({ freq: "weekly", days: [0] });
  });
  it("o resto é personalizado", () => {
    expect(parseExpr("*/5 * * * *").freq).toBe("custom");
    expect(parseExpr("0 9 * * 1-5").freq).toBe("custom");
  });
  it("ida e volta com buildExpr", () => {
    const e = buildExpr("weekly", "18:45", [2, 4], 1);
    const p = parseExpr(e);
    expect(buildExpr("weekly", "18:45", p.days, 1)).toBe(e);
  });
});

describe("weekdayLabel", () => {
  it("0 e 7 são domingo", () => {
    expect(weekdayLabel(0)).toBe("Dom");
    expect(weekdayLabel(7)).toBe("Dom");
    expect(weekdayLabel(1)).toBe("Seg");
  });
});

describe("filterMemories", () => {
  const list = [
    { id: 1, workspaceId: "w", missionId: null, kind: "fato", text: "Usa Postgres 16", authorPane: null, createdAt: 1 },
    { id: 2, workspaceId: "w", missionId: null, kind: "decisao", text: "Não usar ORM pesado", authorPane: null, createdAt: 2 },
  ] as MemoryRow[];
  it("por tipo e texto sem acento", () => {
    expect(filterMemories(list, "todos", "")).toHaveLength(2);
    expect(filterMemories(list, "decisao", "")).toHaveLength(1);
    expect(filterMemories(list, "todos", "nao usar")).toHaveLength(1);
    expect(filterMemories(list, "fato", "orm")).toEqual([]);
  });
});
