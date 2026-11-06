import { describe, expect, it } from "vitest";
import type { PaneRow } from "./ipc";
import { agentTargets, defaultTarget } from "./sendToAgent";

const pane = (o: Partial<PaneRow>): PaneRow => ({ id: "x", workspaceId: "ws", missionId: null, kind: "agent", status: "ocioso", ord: 0, ...o }) as PaneRow;

describe("destinos do envio ao agente", () => {
  const panes = [
    pane({ id: "a", ord: 2, missionId: "m1" }),
    pane({ id: "b", ord: 1, missionId: "m1", status: "hibernado" }),
    pane({ id: "c", ord: 3, missionId: "m2" }),
    pane({ id: "t", ord: 4, missionId: "m1", kind: "terminal" }),
    pane({ id: "d", ord: 5, missionId: null }),
  ];

  it("só painéis de IA vivos da mesma missão, em ordem", () => {
    expect(agentTargets(panes, "ws", "m1").map((p) => p.id)).toEqual(["a"]);
  });

  it("sem missão, só os de fora de missão do workspace", () => {
    expect(agentTargets(panes, "ws", null).map((p) => p.id)).toEqual(["d"]);
  });

  it("o último usado vale se ainda está na lista; senão o primeiro", () => {
    const list = [pane({ id: "a" }), pane({ id: "b" })];
    expect(defaultTarget(list, "b")).toBe("b");
    expect(defaultTarget(list, "gone")).toBe("a");
    expect(defaultTarget([], "a")).toBeNull();
  });

  it("painel esperando resposta não é destino do envio", () => {
    const list = [pane({ id: "a", ord: 1, missionId: "m1" }), pane({ id: "q", ord: 2, missionId: "m1", status: "pergunta" })];
    expect(agentTargets(list, "ws", "m1").map((p) => p.id)).toEqual(["a"]);
  });
});
