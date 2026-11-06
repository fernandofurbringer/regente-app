import { describe, expect, it } from "vitest";
import { approvalState, isPairCode, pairTokenFromHash, wsUrl } from "./api";

describe("pareamento do celular", () => {
  it("lê o token do link e recusa o que não é um token de 64 hex", () => {
    const token = "ab".repeat(32);
    expect(pairTokenFromHash(`#pair=${token}`)).toBe(token);
    expect(pairTokenFromHash("#pair=xyz")).toBeNull();
    expect(pairTokenFromHash(`#pair=${token}&x=1`)).toBeNull();
    expect(pairTokenFromHash("")).toBeNull();
  });

  it("aceita só código de 6 dígitos", () => {
    expect(isPairCode("048213")).toBe(true);
    expect(isPairCode(" 048213 ")).toBe(true);
    expect(isPairCode("04821")).toBe(false);
    expect(isPairCode("04821a")).toBe(false);
  });

  it("monta o WebSocket com o mesmo esquema e host da página", () => {
    expect(wsUrl("/ws/panes/1", { protocol: "https:", host: "192.168.0.5:8789" })).toBe("wss://192.168.0.5:8789/ws/panes/1");
    expect(wsUrl("/ws/panes/1", { protocol: "http:", host: "localhost:8789" })).toBe("ws://localhost:8789/ws/panes/1");
  });
});

describe("aprovação de celular pareado por código", () => {
  it("aguardando enquanto o computador não aprova (403 pending_approval)", () => {
    expect(approvalState(403, "pending_approval")).toBe("waiting");
  });

  it("aprovado com 200 e recusado com 401 (o celular foi revogado)", () => {
    expect(approvalState(200)).toBe("approved");
    expect(approvalState(401, "unauthorized")).toBe("refused");
  });

  it("outros erros não mudam o estado: continua tentando", () => {
    expect(approvalState(403, "outro")).toBe("error");
    expect(approvalState(500)).toBe("error");
    expect(approvalState(null)).toBe("error");
  });
});
