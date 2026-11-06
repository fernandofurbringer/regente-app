import { describe, expect, it } from "vitest";
import { ATTACH_MAX_TRIES, attachRetryDelay, scrollbackLines } from "./terminalOpts";

describe("scrollbackLines", () => {
  it("usa 5000 como padrão e limita a 1000–20000", () => {
    expect(scrollbackLines(undefined)).toBe(5000);
    expect(scrollbackLines(NaN)).toBe(5000);
    expect(scrollbackLines(0)).toBe(1000);
    expect(scrollbackLines(10000)).toBe(10000);
    expect(scrollbackLines(99999)).toBe(20000);
  });
});

describe("attachRetryDelay", () => {
  it("faz backoff 40/80/160/320 ms e depois fica em 500 ms", () => {
    expect([1, 2, 3, 4, 5, 6].map(attachRetryDelay)).toEqual([40, 80, 160, 320, 500, 500]);
    expect(ATTACH_MAX_TRIES * 500).toBeGreaterThanOrEqual(60_000);
  });
});
