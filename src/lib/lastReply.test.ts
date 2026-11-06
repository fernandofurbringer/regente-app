import { describe, expect, it } from "vitest";
import { lastAgentReply } from "./lastReply";

describe("última resposta do agente", () => {
  it("pega o último turno do Claude, sem o pedido seguinte", () => {
    const md = "# Painel\n\n## Você\n\nprimeira pergunta\n\n## Claude\n\nprimeira resposta\n\n## Você\n\nsegunda\n\n## Claude\n\nsegunda resposta\n\n## Você\n\nterceira\n";
    expect(lastAgentReply(md)).toBe("segunda resposta");
  });

  it("mantém os títulos que o próprio agente escreveu", () => {
    const md = "## Claude\n\n## Resumo\n\nfeito\n";
    expect(lastAgentReply(md)).toBe("## Resumo\n\nfeito");
  });

  it("sem resposta do agente (terminal de outro CLI, conversa vazia) devolve null", () => {
    expect(lastAgentReply("# Terminal\n\n```text\n$ ls\n```\n")).toBeNull();
    expect(lastAgentReply("# Painel\n\n## Você\n\noi\n")).toBeNull();
  });
});

describe("blocos de ferramenta fora da resposta", () => {
  it("tira o <details> de ferramenta e fica só com o texto", () => {
    const md = "## Claude\n\n<details>\n<summary>Read x</summary>\n\nconteúdo\n\n</details>\n\nPronto, li o arquivo.\n";
    expect(lastAgentReply(md)).toBe("Pronto, li o arquivo.");
  });
});
