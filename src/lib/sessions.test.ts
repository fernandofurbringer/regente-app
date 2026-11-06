import { describe, expect, it } from "vitest";
import { filterSessions, fmtSessionSize, latestSession } from "./sessions";
import type { SessionInfo } from "./ipc";

const s = (cli: string, id: string, modifiedMs: number, prompt: string | null): SessionInfo => ({ cli, id, modifiedMs, size: 1, prompt });

describe("fmtSessionSize", () => {
  it("usa KB abaixo de 1 MB e MB a partir daí", () => {
    expect(fmtSessionSize(10)).toBe("1 KB");
    expect(fmtSessionSize(2048)).toBe("2 KB");
    expect(fmtSessionSize(1024 * 1024 * 3)).toBe("3.0 MB");
  });
});

describe("filterSessions e latestSession", () => {
  const list = [s("claude", "a", 100, "Corrigir login"), s("codex", "b", 300, "Tela de preços"), s("claude", "c", 200, null)];

  it("busca pela prévia ou pela IA, sem maiúsculas; vazia mostra tudo", () => {
    expect(filterSessions(list, "")).toBe(list);
    expect(filterSessions(list, "  LOGIN ").map((x) => x.id)).toEqual(["a"]);
    expect(filterSessions(list, "codex").map((x) => x.id)).toEqual(["b"]);
    expect(filterSessions(list, "nada disso")).toEqual([]);
  });

  it("a última é a de data mais nova, de qualquer IA; lista vazia dá null", () => {
    expect(latestSession(list)?.id).toBe("b");
    expect(latestSession([])).toBeNull();
  });
});
