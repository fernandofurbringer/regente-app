import { describe, expect, it } from "vitest";
import type { PaneHit } from "./ipc";
import { searchSummary, SOURCE_LABEL } from "./paneSearch";

const hit = (source: PaneHit["source"]): PaneHit => ({ paneId: "p", paneName: "n", workspaceId: "w", source, excerpt: "x" });

describe("resumo da busca", () => {
  it("sem resultado", () => {
    expect(searchSummary({ hits: [], truncated: false, diskLimited: false }, "erro")).toBe('Nenhum resultado para "erro".');
  });
  it("com resultados, limite e leitura parcial do disco", () => {
    const t = searchSummary({ hits: [hit("terminal"), hit("historico")], truncated: true, diskLimited: true }, "erro");
    expect(t).toContain('2 ou mais resultado(s) para "erro".');
    expect(t).toContain("Limite de 200 atingido");
    expect(t).toContain("lido só em parte");
  });
  it("o resultado do disco tem selo próprio", () => {
    expect(SOURCE_LABEL.historico).toBe("no histórico");
  });
});
