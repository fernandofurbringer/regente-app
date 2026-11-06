import { describe, expect, it } from "vitest";
import { DIFF_MAX, expandMentions, FILE_MAX, type MentionEnv } from "./mentions";

const env = (o: Partial<MentionEnv> = {}): MentionEnv => ({
  readFile: async () => null,
  selection: () => "",
  diff: async () => "",
  lastError: () => null,
  terminalTail: () => null,
  prompt: async () => null,
  promptValues: async () => ({}),
  ...o,
});

describe("menções do compositor", () => {
  it("menção desconhecida e @url sem valor ficam como texto", async () => {
    expect(await expandMentions("veja @nada e @url", env())).toBe("veja @nada e @url");
  });

  it("@arquivo: insere o caminho; @arquivo+: o conteúdo quando cabe", async () => {
    expect(await expandMentions("@arquivo:src/a.ts", env())).toBe("src/a.ts");
    const e = env({ readFile: async () => "const a = 1;" });
    expect(await expandMentions("@arquivo+:src/a.ts", e)).toBe("src/a.ts:\n```\nconst a = 1;\n```");
  });

  it("arquivo grande vira só o caminho, com a nota", async () => {
    const e = env({ readFile: async () => "x".repeat(FILE_MAX + 1) });
    const out = await expandMentions("@arquivo+:big.log", e);
    expect(out.startsWith("big.log (grande demais")).toBe(true);
  });

  it("@seleção: a seleção, ou um aviso quando não há", async () => {
    expect(await expandMentions("@seleção", env())).toBe("(nenhuma seleção no editor)");
    expect(await expandMentions("@selecao", env({ selection: () => "let x" }))).toBe("```\nlet x\n```");
  });

  it("@diff vem capado em 30 KB com a marca", async () => {
    const out = await expandMentions("@diff", env({ diff: async () => "+linha\n".repeat(DIFF_MAX) }));
    expect(out.length).toBeLessThanOrEqual(DIFF_MAX + 40);
    expect(out).toContain("[cortado]");
    expect(out.startsWith("```diff\n")).toBe(true);
  });

  it("@erro e @terminal:<pane> usam o que o painel tem; sem painel, ficam como texto", async () => {
    const e = env({ lastError: () => "error[E0432]: x", terminalTail: (r) => (r === "dev" ? "linha 1\nlinha 2" : null) });
    expect(await expandMentions("@erro", e)).toContain("error[E0432]: x");
    expect(await expandMentions("@terminal:dev", e)).toContain("linha 2");
    expect(await expandMentions("@terminal:nada", e)).toBe("@terminal:nada");
  });

  it("@prompt: expande as variáveis; prompt inexistente fica como texto", async () => {
    const e = env({
      prompt: async (n) => (n === "rev" ? "Revise {{arquivo}}." : null),
      promptValues: async () => ({ arquivo: "src/a.ts" }),
    });
    expect(await expandMentions("@prompt:rev", e)).toBe("Revise src/a.ts.");
    expect(await expandMentions("@prompt:nada", e)).toBe("@prompt:nada");
  });

  it("uma passada: o conteúdo colado com outra menção não é expandido de novo", async () => {
    const out = await expandMentions("@seleção", env({ selection: () => "@diff" }));
    expect(out).toBe("```\n@diff\n```");
  });

  it("palavra-chave sem fronteira e e-mail não viram menção", async () => {
    const e = env({ lastError: () => "ERR", diff: async () => "D" });
    expect(await expandMentions("@erros", e)).toBe("@erros");
    expect(await expandMentions("@diffusion", e)).toBe("@diffusion");
    expect(await expandMentions("fale com a@diff.com", e)).toBe("fale com a@diff.com");
  });

  it("caminho absoluto ou com .. fica como texto, com a nota", async () => {
    expect(await expandMentions("@arquivo:../x.ts", env())).toBe("@arquivo:../x.ts (fora do workspace)");
    expect(await expandMentions("@arquivo+:/etc/hosts", env())).toBe("@arquivo+:/etc/hosts (fora do workspace)");
  });

  it("pontuação no fim não entra no caminho; leitura que falha avisa", async () => {
    expect(await expandMentions("veja @arquivo:src/a.ts.", env())).toBe("veja src/a.ts.");
    expect(await expandMentions("@arquivo+:src/a.ts", env())).toBe("(não consegui ler: src/a.ts)");
  });
});
