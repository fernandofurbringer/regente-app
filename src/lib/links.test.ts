import { describe, expect, it } from "vitest";
import { browserUrl, fileUriToPath, findLinks, findLocalhost, findPortInUse, inkContinuation, isImagePath, resolvePath, spanToCells, spanToRange, splitLocation, stripAnsi } from "./links";

const textos = (l: string) => findLinks(l).map((f) => f.text);

describe("links do Claude Code", () => {
  it("caminho no fim da frase, com acento e com ~", () => {
    expect(textos("Veja src/lib/links.ts.")).toEqual(["src/lib/links.ts"]);
    expect(textos("em tools_agentic.rs:333.")).toEqual(["tools_agentic.rs:333"]);
    expect(textos("o plano em docs/decisões/plano.md, e só")).toEqual(["docs/decisões/plano.md"]);
    expect(textos("memória em ~/.claude/CLAUDE.md")).toEqual(["~/.claude/CLAUDE.md"]);
  });
  it("sufixos de linha e coluna de vários formatos", () => {
    expect(textos("a.ts#L10 b.ts#L3C7 c.ts#L1-L20 d.cs(12,5) e.rs:4-9")).toEqual(["a.ts#L10", "b.ts#L3C7", "c.ts#L1-L20", "d.cs(12,5)", "e.rs:4-9"]);
    expect(splitLocation("a.ts#L3C7")).toEqual({ raw: "a.ts", line: 3, col: 7 });
    expect(splitLocation("d.cs(12,5)")).toEqual({ raw: "d.cs", line: 12, col: 5 });
    expect(splitLocation("e.rs:4-9")).toEqual({ raw: "e.rs", line: 4 });
    expect(splitLocation("src/a.ts:12:5")).toEqual({ raw: "src/a.ts", line: 12, col: 5 });
    expect(splitLocation("README.md")).toEqual({ raw: "README.md" });
  });
  it("caminho absoluto com espaço, com a cauda sem espaço como alternativa", () => {
    const [f] = findLinks("salvei em C:\\Users\\Ana Silva\\AppData\\memory\\x.md agora");
    expect(f.text).toBe("C:\\Users\\Ana Silva\\AppData\\memory\\x.md");
    expect(f.alts).toEqual(["Silva\\AppData\\memory\\x.md"]);
  });
  it("caminho entre aspas com espaço e link markdown", () => {
    expect(textos('abra "docs/meu arquivo.md" depois')).toEqual(["docs/meu arquivo.md"]);
    const md = findLinks("ver [MissionBar.tsx](src/components/MissionBar.tsx#L10) e [site](https://x.com/a)");
    expect(md.map((f) => [f.kind, f.text])).toEqual([["path", "src/components/MissionBar.tsx#L10"], ["url", "https://x.com/a"]]);
    expect(md[0].start).toBe(4);
  });
  it("Node.js não é arquivo; Dockerfile e caminho /d/ são", () => {
    expect(textos("feito em Node.js e Next.js")).toEqual([]);
    expect(textos("veja lib/node.js")).toEqual(["lib/node.js"]);
    expect(textos("o Dockerfile e /d/pessoal/a.ts")).toEqual(["Dockerfile", "/d/pessoal/a.ts"]);
  });
  it("file:// vira caminho do Windows", () => {
    expect(fileUriToPath("file:///D:/a%20b/c.md")).toBe("D:\\a b\\c.md");
    expect(fileUriToPath("src/a.ts")).toBe("src/a.ts");
  });
  it("cwd com / e .. não perdem a unidade", () => {
    expect(resolvePath("../x.ts", "D:/a/b")).toEqual({ path: "D:\\a\\x.ts" });
    expect(resolvePath("src/a.ts#L4", "D:\\p")).toEqual({ path: "D:\\p\\src\\a.ts", line: 4 });
  });
  it("continuação do Ink: a linha de cima encosta na margem e a de baixo vem recuada", () => {
    expect(inkContinuation("veja src-tauri/src/mcp/tools_ag", "  entic.rs:333 para", 32)).toBe("entic.rs:333 para");
    expect(inkContinuation("linha curta", "  outra", 80)).toBeNull();
    expect(inkContinuation("x".repeat(80), "sem recuo", 80)).toBeNull();
  });
  it("célula de cada caractere (caractere largo e linhas juntadas)", () => {
    const cells = [{ x: 1, y: 1 }, { x: 3, y: 1 }, { x: 4, y: 1 }, { x: 3, y: 2 }];
    expect(spanToCells(cells, 1, 4)).toEqual({ start: { x: 3, y: 1 }, end: { x: 3, y: 2 } });
    expect(spanToCells(cells, 3, 9)).toBeNull();
  });
});

describe("links", () => {
  it("urls and paths without overlap", () => {
    const f = findLinks("veja https://x.com/a.js e src/App.tsx:12:5 ou C:\\proj\\README.md");
    expect(f.map((l) => [l.kind, l.text])).toEqual([
      ["url", "https://x.com/a.js"],
      ["path", "src/App.tsx:12:5"],
      ["path", "C:\\proj\\README.md"],
    ]);
  });
  it("images flagged", () => {
    expect(findLinks("salvo em assets/hero.png")[0].kind).toBe("image");
    expect(isImagePath("a/b.WEBP")).toBe(true);
    expect(isImagePath("a/b.ts")).toBe(false);
  });
  it("ignores version-like words", () => expect(findLinks("versão v1.4 e.g. ok")).toEqual([]));
  it("50K-char tokens return fast", () => {
    for (const tok of ["a".repeat(50_000), `https://${"a".repeat(50_000)}`, "Zm9vYmFy+/".repeat(5_000)]) {
      const t = performance.now();
      findLinks(`veja ${tok} e src/a.ts`);
      expect(performance.now() - t).toBeLessThan(50);
    }
  });
  it("localhost", () => {
    expect(findLocalhost("  ➜  Local:   http://localhost:5173/ ")).toBe("http://localhost:5173/");
    expect(findLocalhost("http://127.0.0.1:8000")).toBe("http://127.0.0.1:8000");
    expect(findLocalhost("https://example.com")).toBeNull();
  });
  it("wildcard hosts are detected and opened as localhost", () => {
    expect(findLocalhost("ready at http://0.0.0.0:8080/app")).toBe("http://0.0.0.0:8080/app");
    expect(findLocalhost("ready at http://[::]:5173")).toBe("http://[::]:5173");
    expect(browserUrl("http://0.0.0.0:8080/app")).toBe("http://localhost:8080/app");
    expect(browserUrl("http://[::]:5173")).toBe("http://localhost:5173");
    expect(browserUrl("http://127.0.0.1:8000")).toBe("http://127.0.0.1:8000");
  });
  it("stripAnsi", () => expect(stripAnsi("\x1b[32mok\x1b[0m \x1b]0;titulo\x07fim")).toBe("ok fim"));
  it("resolvePath", () => {
    expect(resolvePath("src/a.ts:12:5", "D:\\p")).toEqual({ path: "D:\\p\\src\\a.ts", line: 12, col: 5 });
    expect(resolvePath("C:/x/y.md", "D:\\p")).toEqual({ path: "C:\\x\\y.md" });
    expect(resolvePath("./b/../c.ts", "D:\\p")).toEqual({ path: "D:\\p\\c.ts" });
  });
  it("spanToRange across wrapped rows", () => {
    expect(spanToRange(78, 84, 10, 80)).toEqual({ start: { x: 79, y: 10 }, end: { x: 4, y: 11 } });
  });
});

describe("findPortInUse", () => {
  it("lê a porta de erros de servidor comuns", () => {
    expect(findPortInUse("Error: listen EADDRINUSE: address already in use :::3000")).toBe(3000);
    expect(findPortInUse("Error: listen EADDRINUSE: address already in use 127.0.0.1:8080")).toBe(8080);
    expect(findPortInUse("\u001b[31mError: address already in use 0.0.0.0:4000\u001b[0m")).toBe(4000);
  });
  it("ignora saída comum e erro sem porta", () => {
    expect(findPortInUse("ready on http://localhost:3000")).toBeNull();
    expect(findPortInUse("EADDRINUSE sem número")).toBeNull();
    expect(findPortInUse("Port 5173 is in use, trying another one...")).toBeNull();
  });
});
