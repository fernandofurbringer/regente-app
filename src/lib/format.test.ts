import { describe, expect, it } from "vitest";
import { aggregate, budgetBand, dayRange, fmtTokens, fmtUsd, fmtUsdPt, sinceDay } from "./format";
import type { UsageRow } from "./ipc";

describe("format", () => {
  it("tokens", () => {
    expect(fmtTokens(950)).toBe("950");
    expect(fmtTokens(1530)).toBe("1.5k");
    expect(fmtTokens(132_000)).toBe("132k");
    expect(fmtTokens(1_300_000)).toBe("1.3M");
    expect(fmtTokens(150_900_000)).toBe("151M");
  });
  it("tokens round before picking the unit", () => {
    expect(fmtTokens(999)).toBe("999");
    expect(fmtTokens(1000)).toBe("1k");
    expect(fmtTokens(9949)).toBe("9.9k");
    expect(fmtTokens(9999)).toBe("10k");
    expect(fmtTokens(999_499)).toBe("999k");
    expect(fmtTokens(999_999)).toBe("1M");
    expect(fmtTokens(9_999_999)).toBe("10M");
  });
  it("usd", () => {
    expect(fmtUsd(0)).toBe("$0.00");
    expect(fmtUsd(0.004)).toBe("<US$ 0,01");
    expect(fmtUsd(0.1449)).toBe("$0.14");
  });
  it("usd no topo: pt-BR, valor pequeno não vira zero", () => {
    expect(fmtUsdPt(3.2)).toBe("≈US$ 3,20");
    expect(fmtUsdPt(0)).toBe("US$ 0,00");
    expect(fmtUsdPt(0.004)).toBe("<US$ 0,01");
    expect(fmtUsdPt(0.01)).toBe("≈US$ 0,01");
    expect(fmtUsdPt(12.5)).toBe("≈US$ 12,50");
  });
  it("faixas do orçamento diário", () => {
    expect(budgetBand(0, 10).band).toBe("ok");
    expect(budgetBand(7.99, 10).band).toBe("ok");
    expect(budgetBand(8, 10)).toEqual({ pct: 80, band: "warn" });
    expect(budgetBand(9.99, 10).band).toBe("warn");
    expect(budgetBand(10, 10)).toEqual({ pct: 100, band: "over" });
    expect(budgetBand(3, 0).band).toBe("ok");
  });
  it("sinceDay", () => {
    const now = new Date(2026, 9, 14);
    expect(sinceDay("hoje", now)).toBe("2026-10-14");
    expect(sinceDay("7", now)).toBe("2026-10-08");
    expect(sinceDay("tudo", now)).toBeNull();
  });
  it("dayRange", () => {
    expect(dayRange("2026-02-27", "2026-03-02")).toEqual(["2026-02-27", "2026-02-28", "2026-03-01", "2026-03-02"]);
    expect(dayRange("2026-10-14", "2026-10-14")).toEqual(["2026-10-14"]);
    expect(dayRange("2026-10-15", "2026-10-14")).toEqual([]);
  });
  it("aggregate", () => {
    const r = (cli: string, tokIn: number): UsageRow => ({ day: "d", cli, model: "m", effort: "e", tokIn, tokOut: 1, costUsd: 0.5, terminalsOpened: 1 });
    expect(aggregate([r("a", 1), r("b", 2), r("a", 3)], "cli")).toEqual([
      { key: "a", tokIn: 4, tokOut: 2, costUsd: 1, terminals: 2 },
      { key: "b", tokIn: 2, tokOut: 1, costUsd: 0.5, terminals: 1 },
    ]);
  });
});
