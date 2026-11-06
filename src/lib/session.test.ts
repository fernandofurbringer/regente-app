import { describe, expect, it } from "vitest";
import { parseSession, type SessionSnap } from "./session";

describe("parseSession", () => {
  it("sem valor ou texto inválido vira null", () => {
    expect(parseSession(null)).toBeNull();
    expect(parseSession("{quebrado")).toBeNull();
    expect(parseSession("[1,2]")).toBeNull();
    expect(parseSession("\"texto\"")).toBeNull();
  });

  it("faz o ida e volta do estado salvo", () => {
    const snap: SessionSnap = {
      tabs: ["ws1", "ws2"], active: "ws2",
      missionSel: { ws1: "m1" }, view: { ws1: "arquivos" },
    };
    expect(parseSession(JSON.stringify(snap))).toEqual(snap);
  });

  it("campos ruins caem no padrão sem derrubar os bons", () => {
    const raw = JSON.stringify({
      tabs: ["ws1", 7, null, "ws2"], active: "fantasma",
      missionSel: { ws1: "m1", ws2: 3, ws3: "" }, view: { ws1: "arquivos", ws2: "xyz" },
      railCollapsed: "sim", // campo de versões antigas: ignorado
    });
    expect(parseSession(raw)).toEqual({
      tabs: ["ws1", "ws2"], active: null,
      missionSel: { ws1: "m1" }, view: { ws1: "arquivos" },
    });
  });

  it("objeto sem campos vira sessão vazia (Início, sem abas)", () => {
    expect(parseSession("{}")).toEqual({ tabs: [], active: null, missionSel: {}, view: {} });
  });
});
