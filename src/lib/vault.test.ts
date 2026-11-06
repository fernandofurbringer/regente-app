import { describe, expect, it } from "vitest";
import { parseVaultName, scopedVaultName } from "./vault";

describe("nomes do cofre por workspace", () => {
  it("monta o nome escopado e deixa o global como está", () => {
    expect(scopedVaultName("ab-1", " route.openrouter ")).toBe("ws:ab-1:route.openrouter");
    expect(scopedVaultName(null, "route.openrouter")).toBe("route.openrouter");
  });

  it("separa o escopo do nome visível", () => {
    expect(parseVaultName("ws:ab-1:route.openrouter")).toEqual({ scope: "ab-1", name: "route.openrouter" });
    expect(parseVaultName("route.openrouter")).toEqual({ scope: null, name: "route.openrouter" });
  });
});
