import { describe, expect, it } from "vitest";
import { attentionChips, attentionTooltip, busyCount, closeQuestion, elapsedText, isCleared, needsCloseConfirm, paneBusy, paneStateChip, waitingPanes } from "./attention";
import type { GateOpen, MissionAttention, PaneRow } from "./ipc";

const base: MissionAttention = { workspaceId: "w", missionId: "m", blocked: false, ready: false, reasons: [] };

describe("attentionChips", () => {
  it("missão tranquila não tem chip", () => {
    expect(attentionChips(base)).toEqual([]);
    expect(attentionChips(null)).toEqual([]);
  });

  it("bloqueada com pergunta e falha, nessa ordem", () => {
    const chips = attentionChips({ ...base, blocked: true, reasons: ["pergunta", "falha"] });
    expect(chips.map((c) => c.text)).toEqual(["bloqueada", "pergunta", "falha"]);
  });

  it("pronta para integrar aparece sozinha", () => {
    expect(attentionChips({ ...base, ready: true }).map((c) => c.text)).toEqual(["pronta para integrar"]);
  });

  it("permissão tem o próprio rótulo", () => {
    expect(attentionChips({ ...base, blocked: true, reasons: ["permissao"] })[1].text).toBe("permissão");
  });

  it("estado zerado é o que o store tira da lista", () => {
    expect(isCleared({ ...base, blocked: true })).toBe(false);
    expect(isCleared({ ...base, reasons: ["falha"] })).toBe(false);
    expect(isCleared(base)).toBe(true);
  });

  it("tooltip explica cada motivo numa linha", () => {
    const tip = attentionTooltip({ ...base, blocked: true, reasons: ["falha"] });
    expect(tip.split("\n")).toHaveLength(2);
    expect(tip).toContain("falha: um painel parou com erro");
  });
});

describe("painéis que esperam você (B-04, B-06)", () => {
  const p = (id: string, status: string, over: Partial<PaneRow> = {}) => ({ id, status, closedAt: null, ...over }) as PaneRow;

  it("ordena por tempo no estado: o mais antigo primeiro, só pergunta e erro", () => {
    const panes = [p("a", "pergunta"), p("b", "trabalhando"), p("c", "erro"), p("d", "pergunta")];
    const since = { a: 300, c: 100, d: 200, b: 1 };
    expect(waitingPanes(panes, since).map((x) => x.id)).toEqual(["c", "d", "a"]);
  });

  it("ignora painéis fechados e gate aberto conta desde a criação do gate", () => {
    const panes = [p("a", "pergunta", { closedAt: 9 }), p("b", "trabalhando"), p("c", "concluido")];
    const gate = { paneId: "b", createdAt: 50, decidedAt: null } as never;
    expect(waitingPanes(panes, { a: 1, b: 900, c: 1 }, [gate]).map((x) => x.id)).toEqual(["b"]);
    const decided = { paneId: "b", createdAt: 50, decidedAt: 60 } as never;
    expect(waitingPanes(panes, {}, [decided])).toEqual([]);
  });

  it("painel sem tempo conhecido vai para o fim", () => {
    const panes = [p("sem", "pergunta"), p("com", "pergunta")];
    expect(waitingPanes(panes, { com: 5 }).map((x) => x.id)).toEqual(["com", "sem"]);
  });

  it("elapsedText: segundos e minutos com dois dígitos", () => {
    expect(elapsedText(45_000)).toBe("45s");
    expect(elapsedText(130_000)).toBe("2m10");
    expect(elapsedText(3_605_000)).toBe("60m05");
  });

  it("paneStateChip: texto e tom dos estados com ação ou trabalho; ocioso não tem chip", () => {
    expect(paneStateChip("pergunta", undefined, 0)).toEqual({ text: "Aguardando resposta", tone: "pergunta" });
    expect(paneStateChip("erro", undefined, 0)?.text).toBe("Erro");
    expect(paneStateChip("hibernado", undefined, 0)?.text).toBe("Hibernado");
    expect(paneStateChip("trabalhando", 1_000, 131_000)?.text).toBe("Trabalhando 2m10");
    expect(paneStateChip("trabalhando", undefined, 0)?.text).toBe("Trabalhando");
    expect(paneStateChip("ocioso", 1, 2)).toBeNull();
  });
});

describe("confirmação ao fechar (B-07, B-14)", () => {
  const a = (id: string, status: string, over: Partial<PaneRow> = {}) => ({ id, kind: "agent", status, closedAt: null, ...over }) as PaneRow;
  const gate = (paneId: string, decidedAt: number | null = null) => ({ id: `g-${paneId}`, paneId, decidedAt }) as GateOpen;

  it("só agente trabalhando ou aguardando resposta está ocupado", () => {
    expect(paneBusy(a("x", "trabalhando"))).toBe(true);
    expect(paneBusy(a("x", "pergunta"))).toBe(true);
    expect(paneBusy(a("x", "ocioso"))).toBe(false);
    expect(paneBusy(a("x", "erro"))).toBe(false);
    expect(paneBusy({ ...a("x", "trabalhando"), kind: "terminal" } as PaneRow)).toBe(false);
    expect(paneBusy(a("x", "trabalhando", { closedAt: 5 }))).toBe(false);
  });

  it("conta cada painel uma vez, mesmo com gate aberto", () => {
    const panes = [a("1", "trabalhando"), a("2", "pergunta"), a("3", "ocioso"), a("4", "ocioso")];
    expect(busyCount(panes, [gate("1"), gate("3")])).toBe(3);
  });

  it("gate já decidido não conta", () => {
    expect(busyCount([a("3", "ocioso")], [gate("3", 10)])).toBe(0);
  });

  it("app sem agentes ocupados fecha direto; com um, pede confirmação", () => {
    expect(needsCloseConfirm([a("1", "ocioso"), a("2", "concluido")], [])).toBe(false);
    expect(needsCloseConfirm([a("1", "ocioso")], [gate("1")])).toBe(true);
    expect(needsCloseConfirm([a("1", "trabalhando")], [])).toBe(true);
  });
});

describe("pergunta ao fechar um painel (B-14)", () => {
  const a = (status: string) => ({ id: "1", name: "Claude 2", kind: "agent", status, closedAt: null }) as PaneRow;

  it("agente ocupado pergunta com o nome do painel", () => {
    expect(closeQuestion(a("trabalhando"))).toBe('Fechar "Claude 2"? Ele está trabalhando agora.');
    expect(closeQuestion(a("pergunta"))).toBe('Fechar "Claude 2"? Ele está esperando sua resposta.');
  });

  it("ocioso, concluído ou terminal fecha direto", () => {
    expect(closeQuestion(a("ocioso"))).toBeNull();
    expect(closeQuestion({ ...a("trabalhando"), kind: "terminal" } as PaneRow)).toBeNull();
  });
});
