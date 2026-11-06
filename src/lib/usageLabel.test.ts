import { describe, expect, it } from "vitest";
import { cacheTip, paneUsageText, usageTotalText } from "./usageLabel";

describe("rótulos de consumo", () => {
  // caso real: "responda só OK" = 2 entrada, 4 saída, ~63k de cache
  it("o texto do painel não mostra o cache", () => {
    const text = paneUsageText(2, 4, "≈$0.01");
    expect(text).toBe("2 ent · 4 sai · ≈$0.01");
    expect(text).not.toMatch(/63|cache|k\b/);
  });

  it("o cache aparece só na dica, marcado como fora do total", () => {
    const tip = cacheTip(62_000, 1_000);
    expect(tip).toBe("cache: 62k lido / 1k escrito — não conta");
    expect(tip).toContain("não conta");
  });

  it("missão e topo somam entrada+saída", () => {
    expect(usageTotalText(12_000, 0.4)).toBe("12k ent+sai · ≈$0.40");
  });
});
