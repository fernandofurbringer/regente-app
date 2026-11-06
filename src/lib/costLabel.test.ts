import { describe, expect, it } from "vitest";
import { costLabel, NO_COST_TITLE } from "./costLabel";

describe("custo no cabeçalho do painel", () => {
  it("CLI sem medição local mostra custo n/d com a explicação", () => {
    expect(costLabel({ cliCost: null, remoteNoCost: false, usd: 0 })).toEqual({ text: "custo n/d", title: NO_COST_TITLE });
    expect(NO_COST_TITLE).toBe("esta IA não grava uso localmente");
  });

  it("CLI medido mostra o valor aproximado", () => {
    expect(costLabel({ cliCost: "claude", remoteNoCost: false, usd: 1.5 })).toEqual({ text: "≈$1.50" });
  });

  it("remoto sem leitura de custo (só Claude é lido por ssh)", () => {
    expect(costLabel({ cliCost: "codex", remoteNoCost: true, usd: 2 }).text).toBe("custo n/d");
  });

  it("painel sem CLI registrado não é tratado como sem medição", () => {
    expect(costLabel({ cliCost: undefined, remoteNoCost: false, usd: 0 }).text).toBe("≈$0.00");
  });
});
