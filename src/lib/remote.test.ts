import { describe, expect, it } from "vitest";
import { isRemote, remoteTarget, safeSessionName } from "./remote";

describe("workspace remoto", () => {
  it("monta a URL e tira a barra final", () => {
    expect(remoteTarget("srv", "/home/u/proj/")).toEqual({ url: "ssh://srv/home/u/proj" });
    expect(remoteTarget(" user@srv ", " /x ")).toEqual({ url: "ssh://user@srv/x" });
  });

  it("recusa alias com traço inicial (injeção de opção) e caracteres estranhos", () => {
    expect(remoteTarget("-oProxyCommand=x", "/a")).toHaveProperty("error");
    expect(remoteTarget("srv/x", "/a")).toHaveProperty("error");
    expect(remoteTarget("", "/a")).toHaveProperty("error");
  });

  it("recusa caminho relativo, raiz, aspas, barra invertida e '..'", () => {
    expect(remoteTarget("srv", "home/u")).toHaveProperty("error");
    expect(remoteTarget("srv", "/")).toHaveProperty("error");
    expect(remoteTarget("srv", "/a'b")).toHaveProperty("error");
    expect(remoteTarget("srv", "/a\\b")).toHaveProperty("error");
    expect(remoteTarget("srv", "/a/../b")).toHaveProperty("error");
  });

  it("a raiz é recusada com a mensagem da raiz, antes da checagem de caminho absoluto", () => {
    expect(remoteTarget("srv", "/")).toEqual({ error: "Escolha uma pasta, não a raiz do servidor." });
    expect(remoteTarget("srv", " / ")).toEqual({ error: "Escolha uma pasta, não a raiz do servidor." });
  });

  it("nomes de sessão com controle ou aspas não são suportados", () => {
    expect(safeSessionName("work")).toBe(true);
    expect(safeSessionName("a'b")).toBe(false);
    expect(safeSessionName("x\ny")).toBe(false);
    expect(safeSessionName("")).toBe(false);
  });

  it("reconhece o prefixo", () => {
    expect(isRemote("ssh://srv/x")).toBe(true);
    expect(isRemote("D:\\proj")).toBe(false);
  });
});
