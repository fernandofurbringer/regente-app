import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MissionAttention } from "../lib/ipc";

const missionAttention = vi.fn();
vi.mock("../lib/ipc", () => ({
  api: { missionAttention: (ws: string) => missionAttention(ws) },
  errMsg: (e: unknown) => String(e),
}));

import { useStore } from "./store";

const a = (missionId: string, over: Partial<MissionAttention> = {}): MissionAttention => ({
  workspaceId: "w", missionId, blocked: true, ready: false, reasons: ["pergunta"], ...over,
});

describe("atenção das missões no store", () => {
  beforeEach(() => {
    useStore.setState({ attention: {} });
    missionAttention.mockReset();
  });

  it("a carga troca as missões do workspace: a que saiu da lista some, as de outro workspace ficam", async () => {
    const outro = { ...a("outra"), workspaceId: "x" };
    useStore.setState({ attention: { m: a("m"), outra: outro } });
    missionAttention.mockResolvedValue([]);
    await useStore.getState().loadAttention("w");
    expect(useStore.getState().attention).toEqual({ outra: outro });
  });

  it("estado zerado publicado tira a missão do store", () => {
    useStore.getState().setAttention(a("m"));
    expect(useStore.getState().attention.m).toBeDefined();
    useStore.getState().setAttention(a("m", { blocked: false, reasons: [] }));
    expect(useStore.getState().attention.m).toBeUndefined();
  });

  it("resposta de uma carga antiga não sobrescreve a mais nova", async () => {
    let resolveOld!: (v: MissionAttention[]) => void;
    missionAttention.mockReturnValueOnce(new Promise((r) => (resolveOld = r)));
    missionAttention.mockResolvedValueOnce([]);
    const first = useStore.getState().loadAttention("w");
    await useStore.getState().loadAttention("w");
    resolveOld([a("m")]);
    await first;
    expect(useStore.getState().attention.m).toBeUndefined();
  });

  it("mission-state que chega durante a carga vence a resposta dela", async () => {
    let resolve!: (v: MissionAttention[]) => void;
    missionAttention.mockReturnValueOnce(new Promise((r) => (resolve = r)));
    const load = useStore.getState().loadAttention("w");
    useStore.getState().setAttention(a("m", { reasons: ["permissao"] }));
    resolve([a("m")]);
    await load;
    expect(useStore.getState().attention.m?.reasons).toEqual(["permissao"]);
  });
});
