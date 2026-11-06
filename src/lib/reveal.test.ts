import { describe, expect, it } from "vitest";
import type { PaneGroup } from "./ipc";
import { revelar } from "./reveal";

const pane = { id: "p", missionId: "m1", groupId: "g" };
const base = { missionSel: null, groups: [] as PaneGroup[], focus: null, view: "terminais" };

describe("ir para o painel", () => {
  it("à vista: nada a mudar", () => {
    expect(revelar(pane, base)).toEqual({});
    expect(revelar(pane, { ...base, missionSel: "m1", focus: "p" })).toEqual({});
  });
  it("outra missão filtrada, grupo recolhido, outro em foco e arquivos abertos", () => {
    const groups = [{ id: "g", collapsed: true } as PaneGroup];
    expect(revelar(pane, { missionSel: "m2", groups, focus: "q", view: "arquivos" })).toEqual({ missao: "m1", abrirGrupo: "g", sairDoFoco: true, terminais: true });
  });
  it("painel sem missão com uma missão filtrada: volta para todas", () => {
    expect(revelar({ ...pane, missionId: null }, { ...base, missionSel: "m2" })).toEqual({ missao: null });
  });
});
