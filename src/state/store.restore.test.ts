import { beforeEach, describe, expect, it, vi } from "vitest";

const restoreApply = vi.fn();
vi.mock("../lib/ipc", () => ({
  api: { restoreApply: (...a: unknown[]) => restoreApply(...a) },
  errMsg: (e: unknown) => (e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e)),
  errCode: () => null,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => true) }));

import { useStore } from "./store";

const row = (id: string) => ({ id }) as never;

describe("Agora não só esconde o aviso", () => {
  beforeEach(() => {
    restoreApply.mockReset();
    useStore.setState({ restore: { ws1: [row("p1"), row("p2")] }, restoreDetail: true, restoreDismissed: false, toasts: [] });
  });

  it("decideRestore(false) não chama restoreApply e mantém o aviso escondido na sessão", async () => {
    await useStore.getState().decideRestore(false);
    expect(restoreApply).not.toHaveBeenCalled();
    expect(useStore.getState().restore).toEqual({});
    expect(useStore.getState().restoreDetail).toBe(false);
    expect(useStore.getState().restoreDismissed).toBe(true);
  });

  it("Descartar (com confirmação) é o único caminho que chama restoreApply(false)", async () => {
    await useStore.getState().discardRestore();
    expect(restoreApply).toHaveBeenCalledWith("ws1", false);
    expect(useStore.getState().restore).toEqual({});
  });
});
