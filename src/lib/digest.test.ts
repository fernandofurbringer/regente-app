import { describe, expect, it } from "vitest";
import { buildDigest, digestText, type DigestInput } from "./digest";

const base = (over: Partial<DigestInput> = {}): DigestInput => ({
  since: 1_000,
  missions: [{ id: "m1", name: "Login", status: "ativa", files: 0, add: 0, del: 0, costUsd: 0, verify: null }],
  panes: [],
  statusSince: {},
  gates: [],
  handoffs: [],
  ...over,
});

describe("buildDigest", () => {
  it("lista só agentes que pararam depois da saída; erro conta como parou", () => {
    const secoes = buildDigest(base({
      panes: [
        { id: "p1", missionId: "m1", name: "Claude", status: "ocioso" },
        { id: "p2", missionId: "m1", name: "Codex", status: "erro" },
        { id: "p3", missionId: "m1", name: "Antigo", status: "ocioso" },
      ],
      statusSince: { p1: 2_000, p2: 3_000, p3: 500 },
    }));
    expect(secoes[0].finished).toEqual(["Claude terminou", "Codex parou com erro"]);
  });

  it("perguntas em aberto aparecem como aguardando, mesmo que a pergunta seja antiga", () => {
    const secoes = buildDigest(base({ panes: [{ id: "p1", missionId: "m1", name: "Claude", status: "pergunta" }], statusSince: { p1: 10 } }));
    expect(secoes[0].waiting).toEqual(["Claude aguarda resposta"]);
    expect(secoes[0].finished).toEqual([]);
  });

  it("verificação só conta se rodou depois da saída", () => {
    const ok = { resultado: { ok: true, durationMs: 4_200 }, em: 5_000 };
    const velha = { resultado: { ok: false, durationMs: 1_000 }, em: 900 };
    const com = buildDigest(base({ missions: [{ id: "m1", name: "A", status: "ativa", files: 0, add: 0, del: 0, costUsd: 0, verify: ok }] }));
    expect(com[0].verify).toBe("✔ 4 s");
    const sem = buildDigest(base({ missions: [{ id: "m1", name: "A", status: "ativa", files: 0, add: 0, del: 0, costUsd: 0, verify: velha }] }));
    expect(sem).toEqual([]);
  });

  it("entregas, portões e arquivos entram com o nome do painel de origem", () => {
    const secoes = buildDigest(base({
      missions: [{ id: "m1", name: "A", status: "ativa", files: 3, add: 10, del: 2, costUsd: 1.5, verify: null }],
      panes: [{ id: "p1", missionId: "m1", name: "Planejador", status: "ocioso" }],
      gates: [{ id: "g1", missionId: "m1", title: "Apagar tabela?" }, { id: "g2", missionId: "outra", title: "Outro" }],
      handoffs: [{ missionId: "m1", fromPane: "p1", summary: "plano pronto", createdAt: 2_000 }, { missionId: "m1", fromPane: "p1", summary: "velho", createdAt: 10 }],
    }));
    expect(secoes[0].gates).toEqual(["Apagar tabela?"]);
    expect(secoes[0].handoffs).toEqual(["Planejador: plano pronto"]);
    expect(secoes[0].files).toBe("3 arq. +10 −2");
    expect(secoes[0].cost).toBe("US$ 1.50 (total da missão)");
  });

  it("missão sem nada novo fica de fora", () => {
    expect(buildDigest(base())).toEqual([]);
  });
});

describe("digestText", () => {
  it("diz que nada mudou quando não há seções", () => {
    expect(digestText([])).toBe("Nada novo nas missões desde que você saiu.");
  });
  it("monta uma linha por item", () => {
    const t = digestText(buildDigest(base({ panes: [{ id: "p1", missionId: "m1", name: "Claude", status: "ocioso" }], statusSince: { p1: 2_000 } })));
    expect(t).toBe("Login\n- Claude terminou");
  });
});
