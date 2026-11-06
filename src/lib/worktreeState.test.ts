import { describe, expect, it } from "vitest";
import type { WorktreeState } from "./ipc";
import { adotadasComBranchMudada, branchChip, commitAgeText, fechaDireto, stateLine } from "./worktreeState";

const NOW = 1_792_300_000_000;
const st = (p: Partial<WorktreeState> = {}): WorktreeState => ({ branch: "spec/cadastro", base: "origin/dev", ahead: 3, behind: 0, dirty: 2, lastCommitAt: NOW - 14 * 60_000, head: "7b9a1ed", ...p });

describe("stateLine", () => {
  it("branch, contagens contra a base, alterados e idade do commit", () => {
    expect(stateLine(st(), NOW)).toBe("spec/cadastro · ↑3 ↓0 vs origin/dev · 2 alterados · commit há 14 min");
  });
  it("worktree livre: sem contagens; limpo; sem commit", () => {
    expect(stateLine(st({ branch: null, dirty: 0, lastCommitAt: null }), NOW)).toBe("livre (origin/dev) · limpo");
    expect(stateLine(st({ dirty: 1 }), NOW)).toContain("1 alterado ·");
  });
});

describe("commitAgeText", () => {
  it("mesmo formato do Início: agora, minutos, horas, dias", () => {
    expect(commitAgeText(NOW - 30_000, NOW)).toBe("agora");
    expect(commitAgeText(NOW - 14 * 60_000, NOW)).toBe("há 14 min");
    expect(commitAgeText(NOW - 5 * 3_600_000, NOW)).toBe("há 5 h");
    expect(commitAgeText(NOW - 3 * 86_400_000, NOW)).toBe("há 3 d");
    expect(commitAgeText(null, NOW)).toBeNull();
  });
});

describe("adotadasComBranchMudada", () => {
  const m = (id: string, p: Partial<{ adopted: boolean; status: string; worktreePath: string | null; branch: string | null }> = {}) =>
    ({ id, adopted: true, status: "ativa", worktreePath: `C:/r/.claude/worktrees/${id}`, branch: "spec/a", ...p });
  const wt = (id: string, branch: string | null) => ({ path: `C:/r/.claude/worktrees/${id}`, head: "abc", branch, adopted: true });
  it("acha a adotada aberta cuja branch o ambiente.sh trocou (inclusive para livre)", () => {
    expect(adotadasComBranchMudada([m("a1"), m("a2"), m("a3")], [wt("a1", "spec/a"), wt("a2", "spec/b"), wt("a3", null)])).toEqual(["a2", "a3"]);
  });
  it("caminho com outra barra ou caixa é o mesmo worktree no Windows", () => {
    expect(adotadasComBranchMudada([m("a1", { worktreePath: "c:\\r\\.claude\\worktrees\\a1\\" })], [wt("a1", "spec/b")])).toEqual(["a1"]);
  });
  it("ignora missão comum, arquivada e worktree que sumiu da lista", () => {
    expect(adotadasComBranchMudada([m("a1", { adopted: false }), m("a2", { status: "arquivada" }), m("a3")], [wt("a1", "x"), wt("a2", "x")])).toEqual([]);
  });
});

describe("branchChip", () => {
  it("só missão adotada tem chip; sem branch é livre", () => {
    expect(branchChip({ adopted: true, branch: "spec/x" })).toBe("x");
    expect(branchChip({ adopted: true, branch: null })).toBe("livre");
    expect(branchChip({ adopted: false, branch: "regente/x" })).toBeNull();
  });
  it("a aba é curta: mostra o que vem depois da última barra (o chamado), a dica guarda a branch inteira", () => {
    expect(branchChip({ adopted: true, branch: "spec/4821-ajuste-frete" })).toBe("4821-ajuste-frete");
    expect(branchChip({ adopted: true, branch: "feature/a/b" })).toBe("b");
    expect(branchChip({ adopted: true, branch: "main" })).toBe("main");
  });
});

describe("missão adotada", () => {
  it("fecha sem perguntar só sem painel trabalhando/perguntando e sem arquivo não salvo", () => {
    expect(fechaDireto(["ocioso", "concluido"], false)).toBe(true);
    expect(fechaDireto(["trabalhando"], false)).toBe(false);
    expect(fechaDireto(["pergunta"], false)).toBe(false);
    expect(fechaDireto([], true)).toBe(false);
  });
});
