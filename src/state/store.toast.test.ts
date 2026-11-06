import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/ipc", () => ({
  api: {},
  errMsg: (e: unknown) => (e && typeof e === "object" && "message" in e ? String((e as { message: unknown }).message) : String(e)),
  errCode: (e: unknown) => (e && typeof e === "object" && "code" in e ? String((e as { code: unknown }).code) : null),
}));

import { useStore } from "./store";

describe("avisos de erro pelo código do backend", () => {
  beforeEach(() => useStore.setState({ toasts: [], toastLog: [] }));
  const last = () => { const ts = useStore.getState().toasts; return ts[ts.length - 1]; };

  it("orçamento e pasta inexistente ganham a ação certa pelo código", () => {
    useStore.getState().toastErr({ code: "budget_exceeded", message: "sem a frase" });
    expect(last().action?.label).toBe("Ajustar orçamento");
    useStore.getState().toastErr({ code: "bad_cwd", message: "pasta inexistente: x" });
    expect(last().action?.label).toBe("Escolher pasta");
  });

  it("bad_path (abrir workspace) e texto parecido não ganham ação", () => {
    useStore.getState().toastErr({ code: "bad_path", message: "pasta inexistente: x" });
    expect(last().action).toBeUndefined();
    useStore.getState().toast("orçamento da missão atingido: x", "erro");
    expect(last().action).toBeUndefined();
  });

  it("app fechando vira aviso informativo", () => {
    useStore.getState().toastErr({ code: "shutting_down", message: "o app está fechando" });
    expect(last().kind).toBe("info");
    expect(last().text).toContain("fechando");
  });
});
