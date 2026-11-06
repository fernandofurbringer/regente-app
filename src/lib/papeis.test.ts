import { describe, expect, it } from "vitest";
import type { Perfil } from "./ipc";
import type { RPayload } from "./redline";
import { camposDe, familia, limiteDoPapel, tabelaDe } from "./papeis";

const s = (model: string, effort = "low", cli = "claude") => ({ cli, model, effort });
const perfil = {
  modelos: { planejar: s("claude-opus-5-5", "high"), implementar: s("claude-haiku-5-5"), revisar: s("claude-sonnet-5-5", "high"), testar: s("claude-sonnet-5-5"), explorar: s("claude-sonnet-5-5") },
  papeis: { piloto: s("claude-opus-5-5", "high"), scout: s("claude-sonnet-5-5"), builder: s("claude-sonnet-5-5", "medium"), reviewer: s("claude-sonnet-5-5", "high"), controller: s("claude-sonnet-5-5"), cruzado: null },
} as unknown as Perfil;

describe("quem faz o quê", () => {
  it("a tabela vai e volta sem perder nada, e o scout pode ir para outra CLI", () => {
    const t = tabelaDe(perfil);
    expect(t.piloto?.model).toBe("claude-opus-5-5");
    expect(t.cruzado).toBeNull();
    t.scout = s("gemini-3.5-flash", "low", "gemini");
    t.cruzado = s("gpt-5.5", "medium", "codex");
    const c = camposDe(t, perfil);
    expect(c.papeis.scout).toEqual({ cli: "gemini", model: "gemini-3.5-flash", effort: "low" });
    expect(c.papeis.cruzado?.cli).toBe("codex");
    expect(c.modelos.implementar).toEqual(perfil.modelos.implementar);
  });
  it("limite: a janela própria da família, senão a geral mais cheia da CLI", () => {
    const limits = { windows: [
      { provider: "claude", account: "principal", window: "five_hour", usedPercent: 30, resetsAt: null },
      { provider: "claude", account: "principal", window: "seven_day", usedPercent: 55, resetsAt: null },
      { provider: "claude", account: "principal", window: "seven_day_sonnet", usedPercent: 12, resetsAt: null },
      { provider: "codex", account: "principal", window: "5h", usedPercent: 70, resetsAt: null },
    ], stale: [] } as unknown as RPayload;
    expect(limiteDoPapel(limits, s("claude-sonnet-5-5"))).toEqual({ janela: "seven_day_sonnet", pct: 12 });
    expect(limiteDoPapel(limits, s("claude-opus-5-5"))).toEqual({ janela: "seven_day", pct: 55 });
    expect(limiteDoPapel(limits, s("gpt-5.3-codex-spark", "low", "codex"))).toEqual({ janela: "5h", pct: 70 });
    expect(limiteDoPapel(limits, s("gemini-3.5-flash", "low", "gemini"))).toBeNull();
    expect(limiteDoPapel(null, s("x"))).toBeNull();
    expect(familia("gpt-5.3-codex-spark")).toBe("spark");
  });
});
