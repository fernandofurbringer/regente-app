import { describe, expect, it } from "vitest";
import { mergeRing, fixPrompt } from "./paneErrors";
import { RING_SIZE, type ErrorBlock } from "./errors";
import { SEND_MAX } from "./sendToAgent";

const block = (n: number): ErrorBlock => ({ kind: "rust", lines: [`error[E${n}]: falhou`], startLine: n });

describe("anel de erros do painel", () => {
  it("não repete o mesmo bloco e mantém o mais recente por último", () => {
    const ring = mergeRing([], [block(1), block(2)]);
    expect(mergeRing(ring, [block(2)]).map((b) => b.startLine)).toEqual([1, 2]);
  });

  it("guarda no máximo RING_SIZE blocos, descartando os mais velhos", () => {
    const many = Array.from({ length: RING_SIZE + 3 }, (_, i) => block(i));
    const ring = mergeRing([], many);
    expect(ring).toHaveLength(RING_SIZE);
    expect(ring[ring.length - 1].startLine).toBe(RING_SIZE + 2);
  });
});

describe("texto do Corrija este erro", () => {
  it("leva o erro, o painel, a pasta e as últimas linhas", () => {
    const text = fixPrompt({ name: "Terminal 2", cwd: "C:/app" }, block(7), "linha A\n\nlinha B");
    expect(text).toContain("Corrija este erro:");
    expect(text).toContain("error[E7]: falhou");
    expect(text).toContain("Pasta: C:/app");
    expect(text).toContain("linha B");
  });

  it("nunca passa de 4 KB, mesmo com a tela cheia de texto", () => {
    const text = fixPrompt({ name: "T", cwd: "/x" }, { kind: "stack", lines: ["Error: x".padEnd(3000, "y")], startLine: 0 }, "z".repeat(10_000));
    expect(text.length).toBeLessThanOrEqual(SEND_MAX);
    expect(text.endsWith("[cortado]")).toBe(true);
  });
});
