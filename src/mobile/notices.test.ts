import { describe, expect, it } from "vitest";
import { noticeText, parseNotice } from "./notices";

describe("avisos do celular", () => {
  it("texto fixo por tipo, sem nada do agente", () => {
    expect(noticeText("asks")?.body).not.toMatch(/prompt|pergunta:/i);
    expect(noticeText("finished")?.title).toBe("Agente terminou");
    expect(noticeText("gate")?.title).toBe("Gate aguardando decisão");
    expect(noticeText("outro")).toBeNull();
  });

  it("aceita só quadro de aviso conhecido com painel", () => {
    expect(parseNotice('{"kind":"asks","paneId":"p1"}')).toEqual({ kind: "asks", paneId: "p1" });
    expect(parseNotice('{"kind":"x","paneId":"p1"}')).toBeNull();
    expect(parseNotice('{"kind":"asks"}')).toBeNull();
    expect(parseNotice("não é json")).toBeNull();
  });
});
