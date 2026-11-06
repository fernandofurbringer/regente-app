import { describe, expect, it } from "vitest";
import { ErrorRing, MAX_BLOCK_LINES, detectErrorBlocks, type ErrorBlock } from "./errors";

const bloco = (n: number): ErrorBlock => ({ kind: "stack", lines: [`Error: ${n}`], startLine: n });

describe("detectErrorBlocks", () => {
  it("sem erro devolve lista vazia", () => {
    expect(detectErrorBlocks("")).toEqual([]);
    expect(detectErrorBlocks("ok\nbuild concluído em 2s")).toEqual([]);
  });

  it("erro de Rust termina na linha em branco", () => {
    const t = [
      "error[E0308]: mismatched types",
      " --> src/a.rs:1:2",
      "  |",
      "",
      "próxima coisa",
    ].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("rust");
    expect(b.startLine).toBe(0);
    expect(b.lines).toEqual(["error[E0308]: mismatched types", " --> src/a.rs:1:2", "  |"]);
  });

  it("erros de TypeScript são blocos separados, com as linhas recuadas", () => {
    const t = [
      "src/a.ts(1,2): error TS2322: tipo errado",
      "  Type 'string' is not assignable to 'number'.",
      "src/b.ts(3,1): error TS2339: propriedade",
    ].join("\n");
    const blocos = detectErrorBlocks(t);
    expect(blocos.map((b) => b.kind)).toEqual(["ts", "ts"]);
    expect(blocos[0].lines).toHaveLength(2);
    expect(blocos[1].startLine).toBe(2);
  });

  it("Traceback do Python termina na linha da exceção", () => {
    const t = [
      "Traceback (most recent call last):",
      '  File "a.py", line 3, in <module>',
      "    x()",
      "ValueError: boom",
      "depois do erro",
    ].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("python");
    expect(b.lines[b.lines.length - 1]).toBe("ValueError: boom");
    expect(b.lines).toHaveLength(4);
  });

  it("FAIL do Jest para no próximo PASS", () => {
    const t = ["FAIL src/a.test.ts", "  ● soma › caso", "", "    expect(1).toBe(2)", "", "PASS src/b.test.ts"].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("jest");
    expect(b.lines).toEqual(["FAIL src/a.test.ts", "  ● soma › caso", "", "    expect(1).toBe(2)"]);
  });

  it("FAIL do Vitest com espaço à frente para no resumo e tira a linha em branco final", () => {
    const t = [" FAIL  src/a.test.ts > soma", "AssertionError: esperado 2", "", " Test Files  1 failed (1)"].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("jest");
    expect(b.lines).toEqual([" FAIL  src/a.test.ts > soma", "AssertionError: esperado 2"]);
  });

  it("npm ERR! agrupa as linhas com o mesmo prefixo", () => {
    const t = ["npm ERR! code E1", "npm ERR! falhou", "fim"].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("npm");
    expect(b.lines).toHaveLength(2);
  });

  it("Error: com stack recuada", () => {
    const t = ["TypeError: x is undefined", "    at foo (a.js:1:2)", "    at bar", "ok"].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("stack");
    expect(b.lines).toEqual(["TypeError: x is undefined", "    at foo (a.js:1:2)", "    at bar"]);
  });

  it("erro do PowerShell com At line e CategoryInfo", () => {
    const t = ["At line:1 char:5", "+ foo", "+ CategoryInfo : ObjectNotFound: (foo:String) [], CommandNotFoundException", "", "resto"].join("\n");
    const [b] = detectErrorBlocks(t);
    expect(b.kind).toBe("powershell");
    expect(b.lines).toHaveLength(3);
  });

  it("limita cada bloco a MAX_BLOCK_LINES linhas", () => {
    const corpo = Array.from({ length: 60 }, (_, i) => `  linha ${i}`);
    const [b] = detectErrorBlocks(["src/a.ts(1,1): error TS1000: longo", ...corpo].join("\n"));
    expect(b.lines).toHaveLength(MAX_BLOCK_LINES);
  });

  it("remove ANSI e CR antes de olhar as linhas", () => {
    const [b] = detectErrorBlocks("\u001b[31merror[E0001]: boom\u001b[0m\r\n --> x.rs:1:1\r\n");
    expect(b.lines[0]).toBe("error[E0001]: boom");
    expect(b.lines[1]).toBe(" --> x.rs:1:1");
    expect(b.startLine).toBe(0);
  });

  it("encontra vários tipos na mesma saída, na ordem do texto", () => {
    const t = ["npm ERR! x", "", "error[E1]: y", "", "Error: z"].join("\n");
    expect(detectErrorBlocks(t).map((b) => [b.kind, b.startLine])).toEqual([
      ["npm", 0],
      ["rust", 2],
      ["stack", 4],
    ]);
  });
});

describe("ErrorRing", () => {
  it("guarda só os últimos 5 blocos de cada painel", () => {
    const anel = new ErrorRing();
    for (let n = 1; n <= 7; n++) anel.push("p1", bloco(n));
    const lista = anel.list("p1");
    expect(lista).toHaveLength(5);
    expect(lista[0].startLine).toBe(3);
    expect(anel.last("p1")?.startLine).toBe(7);
  });

  it("separa os painéis e limpa um deles", () => {
    const anel = new ErrorRing(2);
    anel.push("a", bloco(1));
    anel.push("b", bloco(2));
    expect(anel.last("a")?.startLine).toBe(1);
    expect(anel.last("b")?.startLine).toBe(2);
    anel.clear("a");
    expect(anel.last("a")).toBeUndefined();
    expect(anel.list("a")).toEqual([]);
    expect(anel.list("b")).toHaveLength(1);
  });

  it("list devolve cópia, não a lista interna", () => {
    const anel = new ErrorRing();
    anel.push("p", bloco(1));
    anel.list("p").pop();
    expect(anel.list("p")).toHaveLength(1);
  });
});
