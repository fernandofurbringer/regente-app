import { describe, expect, it } from "vitest";
import type { AgentDef } from "./ipc";
import { filterAgents, installMark } from "./catalog";

const agent = (name: string, over: Partial<AgentDef> = {}): AgentDef => ({
  name, description: "", cli: null, model: null, effort: null, role: null, skills: [], body: "", source: "regente", path: "", ...over,
});
const AGENTS = [
  agent("revisor-texto", { description: "Revisa Português", source: "original", role: "reviewer" }),
  agent("builder-web", { description: "Constrói páginas", source: "claude" }),
  agent("pesquisador", { description: "Pesquisa", source: "regente", role: "scout" }),
];
const ALL = { q: "", source: "todos", onlyRole: false };

describe("filterAgents", () => {
  it("sem filtro devolve todos", () => {
    expect(filterAgents(AGENTS, ALL)).toHaveLength(3);
  });
  it("busca sem acento e sem caixa, no nome e na descrição", () => {
    expect(filterAgents(AGENTS, { ...ALL, q: "portugues" }).map((a) => a.name)).toEqual(["revisor-texto"]);
    expect(filterAgents(AGENTS, { ...ALL, q: "CONSTROI" }).map((a) => a.name)).toEqual(["builder-web"]);
    expect(filterAgents(AGENTS, { ...ALL, q: "  revisor " }).map((a) => a.name)).toEqual(["revisor-texto"]);
  });
  it("filtra por origem", () => {
    expect(filterAgents(AGENTS, { ...ALL, source: "claude" }).map((a) => a.name)).toEqual(["builder-web"]);
  });
  it("tem tarefa = função definida", () => {
    expect(filterAgents(AGENTS, { ...ALL, onlyRole: true }).map((a) => a.name)).toEqual(["revisor-texto", "pesquisador"]);
  });
  it("combina os filtros", () => {
    expect(filterAgents(AGENTS, { q: "pesq", source: "regente", onlyRole: true }).map((a) => a.name)).toEqual(["pesquisador"]);
    expect(filterAgents(AGENTS, { q: "pesq", source: "original", onlyRole: false })).toEqual([]);
  });
});

describe("installMark", () => {
  it("carregando, sim e não", () => {
    expect(installMark(undefined)).toBe("…");
    expect(installMark(true)).toBe("✓");
    expect(installMark(false)).toBe("—");
  });
});
