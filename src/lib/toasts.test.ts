import { describe, expect, it } from "vitest";
import { pushLog, TOAST_LOG_MAX, visibleToasts } from "./toasts";

describe("fila de avisos", () => {
  it("mostra no máximo 4 e conta o resto", () => {
    const q = [1, 2, 3, 4, 5, 6];
    expect(visibleToasts(q)).toEqual({ shown: [1, 2, 3, 4], hidden: 2 });
    expect(visibleToasts([1, 2])).toEqual({ shown: [1, 2], hidden: 0 });
    expect(visibleToasts<number>([])).toEqual({ shown: [], hidden: 0 });
  });
  it("sino guarda os 20 últimos, mais novo primeiro", () => {
    let log: number[] = [];
    for (let i = 1; i <= 25; i++) log = pushLog(log, i);
    expect(log).toHaveLength(TOAST_LOG_MAX);
    expect(log[0]).toBe(25);
    expect(log[19]).toBe(6);
  });
});
