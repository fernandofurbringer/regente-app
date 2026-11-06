import { describe, expect, it } from "vitest";
import { missionsInUse } from "./inUse";

describe("missões em uso no celular", () => {
  it("mostra só missões com painel vivo; hibernado e sem painel ficam de fora", () => {
    const missions = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }];
    const panes = [
      { missionId: "a", status: "ocioso" },
      { missionId: "b", status: "hibernado" },
      { missionId: "c", status: "trabalhando" },
      { missionId: null, status: "ocioso" },
      { missionId: "c", status: "hibernado" },
    ];
    expect(missionsInUse(missions, panes).map((m) => m.id)).toEqual(["a", "c"]);
  });

  it("sem painéis nenhuma missão está em uso", () => {
    expect(missionsInUse([{ id: "a" }], [])).toEqual([]);
  });
});
