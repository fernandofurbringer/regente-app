import { describe, expect, it } from "vitest";
import { fmtMem, isIdleClaude } from "./resources";

const row = (over: object = {}) => ({ kind: "agent", cli: "claude", status: "ocioso", closedAt: null, ...over }) as Parameters<typeof isIdleClaude>[0];

describe("isIdleClaude", () => {
  it("aceita só painel Claude ocioso e aberto", () => {
    expect(isIdleClaude(row())).toBe(true);
    expect(isIdleClaude(row({ status: "trabalhando" }))).toBe(false);
    expect(isIdleClaude(row({ cli: "codex" }))).toBe(false);
    expect(isIdleClaude(row({ kind: "terminal", cli: null }))).toBe(false);
    expect(isIdleClaude(row({ closedAt: 123 }))).toBe(false);
  });
});

describe("fmtMem", () => {
  it("usa MB abaixo de 1 GB e vírgula decimal em GB", () => {
    expect(fmtMem(870)).toBe("870 MB");
    expect(fmtMem(3200)).toBe("3,1 GB");
    expect(fmtMem(1024)).toBe("1 GB");
  });
});
