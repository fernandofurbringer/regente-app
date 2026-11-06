import { describe, expect, it } from "vitest";
import { agoText, groupByLastUse, useGroup } from "./recent";

const NOW = new Date(2026, 9, 15, 15, 0, 0); // 15/10/2026 15:00 local
const startToday = new Date(2026, 9, 15).getTime();
const DAY = 86_400_000;

describe("Home: último uso", () => {
  it("useGroup separa hoje, ontem, a semana e o resto pelo calendário", () => {
    expect(useGroup(startToday + 1000, NOW)).toBe("Hoje");
    expect(useGroup(startToday - 1000, NOW)).toBe("Ontem");
    expect(useGroup(startToday - 6 * DAY, NOW)).toBe("Últimos 7 dias");
    expect(useGroup(startToday - 30 * DAY, NOW)).toBe("Mais antigos");
  });

  it("groupByLastUse ordena o grupo pelo uso mais recente e esconde grupos vazios", () => {
    const ws = [
      { name: "velho", at: startToday - 30 * DAY },
      { name: "hoje-cedo", at: startToday + 100 },
      { name: "ontem", at: startToday - 500 },
      { name: "hoje-tarde", at: startToday + 900 },
    ];
    const groups = groupByLastUse(ws, (w) => w.at, NOW);
    expect(groups.map(([g]) => g)).toEqual(["Hoje", "Ontem", "Mais antigos"]);
    expect(groups[0][1].map((w) => w.name)).toEqual(["hoje-tarde", "hoje-cedo"]);
  });

  it("agoText em minutos, horas e dias", () => {
    const now = Date.now();
    expect(agoText(now - 12 * 60_000, now)).toBe("há 12 min");
    expect(agoText(now - 3 * 3_600_000, now)).toBe("há 3 h");
    expect(agoText(now - 2 * DAY, now)).toBe("há 2 d");
    expect(agoText(now - 5_000, now)).toBe("agora");
  });
});
