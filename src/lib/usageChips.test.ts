import { describe, expect, it } from "vitest";
import type { RPayload } from "./redline";
import { chipLabel, usageChips } from "./usageChips";

const NOW = 1_792_287_922_849; // 2026-10-22, a leitura real do banco do usuário (5h 26 %, 7d 5 %)
const win = (window: string, usedPercent: number, account = "principal", resetsAt: number | null = null) => ({ provider: "claude", account, window, usedPercent, resetsAt });
const real: RPayload = {
  windows: [win("seven_day", 5, "principal", NOW + (3 * 24 + 4) * 3_600_000), win("five_hour", 26, "principal", NOW + 130 * 60_000)],
  stale: [],
  topo: ["claude/principal"],
};

describe("usageChips", () => {
  it("mostra 5h antes de 7d, com percentual, cor e tempo até reiniciar", () => {
    const c = usageChips(real, NOW);
    expect(c.map((x) => [x.label, x.pct, x.reset])).toEqual([["5h", 26, "2h10"], ["7d", 5, "3d4h"]]);
    expect(c[0].color).toBe("#3fb950");
    expect(c[0].account).toBeNull();
    expect(c[0].stale).toBe(false);
    expect(c[0].title).toBe("Claude · principal · 5h: 26% · reinicia em 2h10");
    expect(c[0].key).toBe("claude/principal/five_hour");
  });

  it("sem leitura ou sem conta no topo, não há chip", () => {
    expect(usageChips(null, NOW)).toEqual([]);
    expect(usageChips({ ...real, topo: [] }, NOW)).toEqual([]);
  });

  it("só janelas do Claude; o Codex fica fora mesmo marcado no topo", () => {
    const p: RPayload = { ...real, windows: [...real.windows, { provider: "codex", account: "principal", window: "5h", usedPercent: 70, resetsAt: null }], topo: ["claude/principal", "codex/principal"] };
    expect(usageChips(p, NOW).map((c) => c.label)).toEqual(["5h", "7d"]);
  });

  it("marca leitura antiga e põe o nome da conta quando há mais de uma do Claude no topo", () => {
    const p: RPayload = {
      windows: [win("five_hour", 90), win("five_hour", 10, "trabalho")],
      stale: [{ provider: "claude", account: "trabalho", at: NOW }],
      topo: ["claude/principal", "claude/trabalho"],
    };
    const c = usageChips(p, NOW);
    expect(c.map((x) => [x.account, x.stale, x.color])).toEqual([["principal", false, "#f85149"], ["trabalho", true, "#3fb950"]]);
    expect(c[1].title).toBe("Claude · trabalho · 5h: 10% · leitura antiga: a última consulta falhou");
    expect(c[1].reset).toBeNull();
  });

  it("rotula as janelas por modelo e deixa as desconhecidas como vêm", () => {
    expect(chipLabel("five_hour")).toBe("5h");
    expect(chipLabel("seven_day")).toBe("7d");
    expect(chipLabel("seven_day_sonnet")).toBe("7d sonnet");
    expect(chipLabel("extra")).toBe("extra");
  });

  it("depois de 5h e 7d, as demais janelas vêm por nome", () => {
    const p: RPayload = { ...real, windows: [win("zeta", 1), win("seven_day_opus", 2), ...real.windows] };
    expect(usageChips(p, NOW).map((c) => c.label)).toEqual(["5h", "7d", "7d opus", "zeta"]);
  });

  it("limita o percentual a 0..100 e a cor segue o número mostrado", () => {
    const p: RPayload = { ...real, windows: [win("five_hour", 104), win("seven_day", 59.6), win("seven_day_opus", -3)] };
    expect(usageChips(p, NOW).map((c) => [c.pct, c.color])).toEqual([[100, "#f85149"], [60, "#d29922"], [0, "#3fb950"]]);
  });

  it("janela que já reiniciou: 'reinicia agora'", () => {
    const p: RPayload = { ...real, windows: [win("five_hour", 10, "principal", NOW - 1)] };
    const [c] = usageChips(p, NOW);
    expect(c.reset).toBe("agora");
    expect(c.title).toBe("Claude · principal · 5h: 10% · reinicia agora");
  });
});
