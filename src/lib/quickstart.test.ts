import { describe, expect, it } from "vitest";
import { defaultAgentCli, defaultAgentLabel, defaultCli, ensureWorkspace, firstMissionInput, QUICK_CARDS, visibleCards } from "./quickstart";
import type { Detected } from "./ipc";

const cli = (id: string, found = true): Detected => ({ id, kind: "cli", path: found ? `C:/bin/${id}.cmd` : null });

describe("cards de início rápido", () => {
  it("mostra todos os cards quando nada foi fechado", () => {
    expect(visibleCards([])).toHaveLength(QUICK_CARDS.length);
  });

  it("some só com o card fechado e mantém a ordem", () => {
    const left = visibleCards(["squad", "id-que-nao-existe"]).map((c) => c.id);
    expect(left).toEqual(["pasta", "primeira", "voz", "celular", "atalhos"]);
  });

  it("os textos pedidos aparecem como estão", () => {
    const titles = QUICK_CARDS.map((c) => c.title);
    for (const t of ["Abrir uma pasta", "Primeira missão (Livre, 2 {ia})", "Squad de exemplo", "Configurar voz", "Parear o celular", "Ver atalhos (Ctrl+Shift+P)"]) {
      expect(titles).toContain(t);
    }
  });
});

describe("primeira missão", () => {
  it("é livre, sem worktree, com dois terminais de Claude no workspace", () => {
    const input = firstMissionInput("ws1", "C:/proj");
    expect(input.mode).toBe("livre");
    expect(input.worktree).toBe(false);
    expect(input.slots).toHaveLength(2);
    for (const s of input.slots ?? []) {
      expect(s).toMatchObject({ kind: "agent", cli: "claude", workspaceId: "ws1", cwd: "C:/proj", permission: "pend" });
    }
    // cada slot é um objeto próprio (mudar um não muda o outro)
    expect(input.slots?.[0]).not.toBe(input.slots?.[1]);
  });
});

describe("cards que precisam de pasta", () => {
  const ws = { id: "w1", path: "C:/proj", name: "proj", lastOpenedAt: 0, pinned: false };

  it("sem workspace, pede a pasta e segue com o workspace aberto (sem erro 'abra uma pasta')", async () => {
    const opened: string[] = [];
    const got = await ensureWorkspace(null, async () => "C:/proj", async (p) => (opened.push(p), ws));
    expect(got).toEqual(ws);
    expect(opened).toEqual(["C:/proj"]);
  });

  it("com workspace ativo, não abre seletor", async () => {
    const got = await ensureWorkspace(ws, async () => { throw new Error("não deveria pedir pasta"); }, async () => null);
    expect(got).toEqual(ws);
  });

  it("cancelar o seletor não faz nada", async () => {
    const got = await ensureWorkspace(null, async () => null, async () => { throw new Error("não deveria abrir"); });
    expect(got).toBeNull();
  });
});

describe("IA padrão", () => {
  it("é a primeira instalada na ordem claude > codex > gemini", () => {
    expect(defaultCli([cli("gemini"), cli("codex"), cli("claude")])).toBe("claude");
    expect(defaultCli([cli("gemini"), cli("codex")])).toBe("codex");
    expect(defaultCli([cli("gemini")])).toBe("gemini");
  });

  it("ignora as CLIs que não estão instaladas e null quando nenhuma está", () => {
    expect(defaultCli([cli("claude", false), cli("codex")])).toBe("codex");
    expect(defaultCli([cli("claude", false)])).toBeNull();
    expect(defaultCli([])).toBeNull();
  });

  it("a primeira missão usa a IA escolhida", () => {
    const input = firstMissionInput("ws1", "C:/proj", "codex");
    for (const s of input.slots ?? []) expect(s).toMatchObject({ cli: "codex" });
  });
});

describe("IA do painel padrão", () => {
  it("usa a última escolhida se ainda estiver instalada; senão a primeira instalada", () => {
    const d = [cli("gemini"), cli("codex")];
    expect(defaultAgentCli(d, "gemini")).toBe("gemini");
    expect(defaultAgentCli(d, "claude")).toBe("codex");
    expect(defaultAgentCli(d, null)).toBe("codex");
    expect(defaultAgentCli([], "codex")).toBeNull();
  });
});

describe("IA nos cards", () => {
  it("o nome da IA padrão entra no texto; sem IA instalada, Claude Code", () => {
    const card = visibleCards([], "Codex").find((c) => c.id === "primeira");
    expect(card?.title).toBe("Primeira missão (Livre, 2 Codex)");
    expect(card?.text).toBe("Dois terminais com Codex, lado a lado, no workspace atual.");
    expect(visibleCards([]).find((c) => c.id === "primeira")?.title).toBe("Primeira missão (Livre, 2 Claude Code)");
  });

  it("a IA padrão tem id e nome; sem nome cadastrado, o próprio id", () => {
    expect(defaultAgentLabel([cli("codex")], null, [{ id: "codex", label: "Codex" }])).toEqual({ cli: "codex", label: "Codex" });
    expect(defaultAgentLabel([], null, [])).toEqual({ cli: "claude", label: "claude" });
  });
});
