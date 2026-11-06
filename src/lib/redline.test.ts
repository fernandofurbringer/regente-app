import { describe, expect, it } from "vitest";
import { accountPeak, colorFor, countdown, labelOf, peakHour, resetText, seriesOf, worstTopBar, type RPayload, type RPoint } from "./redline";

const at = (h: number, pct: number, window = "five_hour"): RPoint => ({
  ts: new Date(2026, 9, 14, h, 0).getTime(), // local time, so getHours() matches on any machine
  provider: "claude",
  account: "principal",
  window,
  usedPercent: pct,
});

describe("countdown", () => {
  it("reads like the tray", () => {
    expect(countdown(-1)).toBe("agora");
    expect(countdown(45 * 60_000)).toBe("45min");
    expect(countdown((2 * 60 + 10) * 60_000)).toBe("2h10");
    expect(countdown((3 * 24 + 4) * 3_600_000)).toBe("3d4h");
  });
});

describe("colorFor", () => {
  it("is green, amber, then red by use", () => {
    expect(colorFor(10)).toBe("#3fb950");
    expect(colorFor(59.9)).toBe("#3fb950");
    expect(colorFor(60)).toBe("#d29922");
    expect(colorFor(85)).toBe("#f85149");
  });
});

describe("peakHour", () => {
  it("is the hour with the highest average increase", () => {
    // +20 landing at 15h, +5 at 16h, a drop at 17h (a reset) that does not count
    const pts = [at(14, 10), at(15, 30), at(16, 35), at(17, 2)];
    expect(peakHour(pts)).toBe(15);
  });

  it("picks the hour of the biggest rise among steady climbs", () => {
    const pts = [at(9, 0), at(10, 10), at(11, 12), at(12, 14), at(13, 16)]; // 10 at 10h, then 2 per hour
    expect(peakHour(pts)).toBe(10);
  });

  it("is null when nothing went up", () => {
    expect(peakHour([at(9, 50), at(10, 40)])).toBeNull();
    expect(peakHour([])).toBeNull();
  });
});

describe("worstTopBar", () => {
  it("is the worst use among the marked accounts only", () => {
    const p = {
      windows: [
        { provider: "claude", account: "principal", window: "five_hour", usedPercent: 42, resetsAt: null },
        { provider: "claude", account: "trabalho", window: "seven_day", usedPercent: 91, resetsAt: null },
        { provider: "codex", account: "principal", window: "5h", usedPercent: 12, resetsAt: null },
      ],
      stale: [],
      topo: ["claude/principal", "codex/principal"],
    };
    expect(worstTopBar(p)).toBe(42);
    expect(worstTopBar({ ...p, topo: [] })).toBeNull();
  });
});

describe("labels and series", () => {
  it("names the windows like the tray", () => {
    expect(labelOf("five_hour")).toBe("5h");
    expect(labelOf("seven_day")).toBe("semana");
    expect(labelOf("opus")).toBe("opus");
  });

  it("groups the history per window, oldest first", () => {
    const s = seriesOf([at(16, 35), at(14, 10), at(15, 30, "seven_day")]);
    expect([...s.keys()]).toEqual(["claude/principal/5h", "claude/principal/semana"]);
    expect(s.get("claude/principal/5h")!.map((p) => p.usedPercent)).toEqual([10, 35]);
  });
});

describe("limites por conta e reinício", () => {
  const p: RPayload = {
    windows: [
      { provider: "claude", account: "principal", window: "five_hour", usedPercent: 42, resetsAt: 0 },
      { provider: "claude", account: "principal", window: "seven_day", usedPercent: 93, resetsAt: null },
      { provider: "codex", account: "principal", window: "five_hour", usedPercent: 10, resetsAt: null },
    ],
    stale: [],
    topo: [],
  };
  it("pior janela de uma conta, null quando não há leitura", () => {
    expect(accountPeak(p, "claude", "principal")).toBe(93);
    expect(accountPeak(p, "claude", "outra")).toBeNull();
    expect(accountPeak(p, null, "principal")).toBeNull();
  });
  it("reinicia em usa o mesmo formato do relógio", () => {
    const now = 1_000_000;
    expect(resetText(now + (2 * 60 + 10) * 60_000, now)).toBe("reinicia em 2h10");
    expect(resetText(now + 45 * 60_000, now)).toBe("reinicia em 45min");
    expect(resetText(now - 1, now)).toBe("reinicia agora");
    expect(resetText(null, now)).toBeNull();
  });
});
