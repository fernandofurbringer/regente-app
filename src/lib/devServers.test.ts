import { describe, expect, it } from "vitest";
import { dicaDePorta, marcarParando, portaDaUrl, registrar, remover, type DevServer, type DevMap } from "./devServers";

const srv = (paneId: string, url: string, since = 1): DevServer => ({ paneId, url, since, source: "terminal" });

describe("registro de servidores", () => {
  it("adiciona e mantém o início quando a mesma URL aparece de novo", () => {
    const m1 = registrar({}, srv("p1", "http://localhost:3000", 100));
    const m2 = registrar(m1, srv("p1", "http://localhost:3000", 999));
    expect(m2).toBe(m1);
    expect(m2.p1.since).toBe(100);
  });

  it("troca a URL quando o painel anuncia outra", () => {
    const m = registrar(registrar({}, srv("p1", "http://localhost:3000")), srv("p1", "http://localhost:5173", 50));
    expect(m.p1.url).toBe("http://localhost:5173");
    expect(m.p1.since).toBe(50);
  });

  it("some quando o painel fecha, e fechar um painel sem servidor não muda nada", () => {
    const m: DevMap = registrar(registrar({}, srv("p1", "http://localhost:3000")), srv("p2", "http://localhost:4000"));
    const sem = remover(m, "p1");
    expect(Object.keys(sem)).toEqual(["p2"]);
    expect(remover(sem, "p1")).toBe(sem);
  });
});

describe("parar", () => {
  it("marca como parando sem tirar a entrada, e não mexe em painel sem servidor", () => {
    const m = registrar({}, srv("p1", "http://localhost:3000"));
    const p = marcarParando(m, "p1");
    expect(p.p1.parando).toBe(true);
    expect(p.p1.url).toBe("http://localhost:3000");
    expect(marcarParando(p, "p1")).toBe(p);
    expect(marcarParando(m, "p9")).toBe(m);
  });
});

describe("porta", () => {
  it("lê a porta da URL e aceita ausência", () => {
    expect(portaDaUrl("http://localhost:3000/rota")).toBe(3000);
    expect(portaDaUrl("http://localhost/")).toBeNull();
    expect(portaDaUrl("não é url")).toBeNull();
  });
});

describe("dica de porta em uso", () => {
  const nomes = (id: string) => (id === "p1" ? "Servidor API" : id);
  it("nomeia o painel que já tem a porta", () => {
    const m = registrar({}, srv("p1", "http://localhost:3000"));
    expect(dicaDePorta("Error: listen EADDRINUSE: address already in use :::3000", m, "p2", nomes)).toBe("Porta 3000 em uso por outro painel: Servidor API");
  });
  it("sem painel dono, fala de outro processo", () => {
    const m = registrar({}, srv("p1", "http://localhost:3000"));
    expect(dicaDePorta("Error: listen EADDRINUSE: address already in use :::5173", m, "p2", nomes)).toMatch(/^Porta 5173 em uso por outro processo/);
  });
  it("saída sem erro de porta não tem dica", () => {
    expect(dicaDePorta("ready on http://localhost:3000", {}, "p2", nomes)).toBeNull();
  });
});
