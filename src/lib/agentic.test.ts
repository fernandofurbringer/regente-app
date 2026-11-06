import { describe, expect, it } from "vitest";
import type { Decision, TaskType } from "./ipc";
import { baseUrlError, linesToList, short, slugId, spend, spendByType, typeName } from "./agentic";

const dec = (over: Partial<Decision>): Decision => ({
  id: "d", missionId: null, paneId: null, request: "r", taskType: "codigo", cli: "claude", model: "haiku", effort: null, agent: null,
  decidedBy: "haiku", costUsd: 0.01, receipt: "", createdAt: 1000, ...over,
});

describe("baseUrlError", () => {
  it("https é aceito", () => {
    expect(baseUrlError("https://openrouter.ai/api/v1", "openai")).toBeNull();
  });
  it("http só em loopback", () => {
    expect(baseUrlError("http://localhost:11434/v1", "openai")).toBeNull();
    expect(baseUrlError("http://127.0.0.1:8080", "openai")).toBeNull();
    expect(baseUrlError("http://[::1]:8080", "openai")).toBeNull();
    expect(baseUrlError("http://api.exemplo.com/v1", "openai")).toMatch(/https/);
  });
  it("vazio: obrigatório só para o provedor compatível com OpenAI", () => {
    expect(baseUrlError("  ", "openai")).toMatch(/informe/);
    expect(baseUrlError("", "anthropic")).toBeNull();
  });
  it("texto que não é URL", () => {
    expect(baseUrlError("não é url", "openai")).toBe("URL inválida");
  });
});

describe("slugId", () => {
  it("minúsculas e hífens, sem acento", () => {
    expect(slugId("Revisão de Texto!", [])).toBe("revisao-de-texto");
  });
  it("numera quando colide", () => {
    expect(slugId("codigo", ["codigo"])).toBe("codigo-2");
    expect(slugId("codigo", ["codigo", "codigo-2"])).toBe("codigo-3");
  });
  it("nome vazio vira 'tipo'", () => {
    expect(slugId("!!!", [])).toBe("tipo");
  });
});

describe("listas e texto", () => {
  it("linhas viram lista sem vazios", () => {
    expect(linesToList(" a \n\n b ")).toEqual(["a", "b"]);
  });
  it("corta em 80 caracteres com reticências", () => {
    expect(short("abc", 80)).toBe("abc");
    const long = "x".repeat(100);
    expect(short(long)).toHaveLength(80);
    expect(short(long).endsWith("…")).toBe(true);
  });
});

describe("gasto das decisões", () => {
  const list = [
    dec({ id: "a", createdAt: 2000, costUsd: 0.5 }),
    dec({ id: "b", createdAt: 3000, costUsd: 0.25, taskType: null }),
    dec({ id: "c", createdAt: 500, costUsd: 9 }),
  ];
  it("conta, soma e conta erros só desde o início", () => {
    expect(spend(list, 1000)).toEqual({ count: 2, costUsd: 0.75, errors: 1 });
  });
  it("por tipo, decisão sem tipo em (sem tipo)", () => {
    expect(spendByType(list, 1000)).toEqual({
      codigo: { count: 1, costUsd: 0.5, errors: 0 },
      "(sem tipo)": { count: 1, costUsd: 0.25, errors: 1 },
    });
  });
  it("nome do tipo ou id quando não existe", () => {
    const types = [{ id: "codigo", name: "Código" }] as TaskType[];
    expect(typeName(types, "codigo")).toBe("Código");
    expect(typeName(types, "sumido")).toBe("sumido");
    expect(typeName(types, null)).toBe("(sem tipo)");
  });
});
