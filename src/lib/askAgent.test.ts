import { describe, expect, it } from "vitest";
import { askAboutFile, parentOf, relPath } from "./askAgent";
import { SEND_MAX } from "./sendToAgent";

const base = { workspaceId: "ws", missionId: null, root: "C:/app", path: "C:/app/src/a.ts" };

describe("pasta de cima", () => {
  it("corta o último trecho", () => {
    expect(parentOf("D:\\r\\src\\a.ts")).toBe("D:\\r\\src");
    expect(parentOf("C:/app/src")).toBe("C:/app");
  });

  // o git lista repositório aninhado não rastreado (worktree) com barra no fim: `?? wt/`. Sem andar, a árvore
  // subia de pasta em pasta para sempre e a janela congelava ao abrir.
  it("com barra no fim, ainda sobe", () => {
    expect(parentOf("D:\\r\\regente-wt-lote\\")).toBe("D:\\r");
    expect(parentOf("C:/app/.worktrees/x/")).toBe("C:/app/.worktrees");
  });
});

describe("pedidos sobre o arquivo", () => {
  it("usa o caminho relativo ao workspace", () => {
    expect(relPath("C:/app/src/a.ts", "C:/app/")).toBe("src/a.ts");
    expect(relPath("/outro/b.ts", "C:/app")).toBe("/outro/b.ts");
  });

  it("sem seleção, pede sobre o arquivo", () => {
    const s = askAboutFile("testes", { ...base, selection: "" });
    expect(s.text).toBe("Escreva testes para o arquivo `src/a.ts`.");
    expect(s.title).toBe("Pedir ao agente: escrever testes");
  });

  it("com seleção, leva o trecho em bloco de código", () => {
    const s = askAboutFile("revisar", { ...base, selection: "const x = 1;" });
    expect(s.text).toContain("Revise esta seleção de `src/a.ts` e aponte problemas.");
    expect(s.text).toContain("```\nconst x = 1;\n```");
  });

  it("a seleção enorme sai cortada no teto", () => {
    const s = askAboutFile("explicar", { ...base, selection: "x".repeat(SEND_MAX * 3) });
    expect(s.text.length).toBeLessThanOrEqual(SEND_MAX + 200);
    expect(s.text).toContain("[cortado]");
  });
});
