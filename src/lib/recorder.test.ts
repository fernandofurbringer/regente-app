import { describe, expect, it } from "vitest";
import { micMessage, paraWav16k, rms, wavPcm16 } from "./recorder";

describe("gravação de voz", () => {
  it("rms de um sinal cheio é 1 e de silêncio é 0", () => {
    expect(rms(new Float32Array([1, -1, 1, -1]))).toBeCloseTo(1);
    expect(rms(new Float32Array(8))).toBe(0);
    expect(rms(new Float32Array(0))).toBe(0);
  });

  it("negar o microfone vira a orientação de privacidade do Windows", () => {
    expect(micMessage({ name: "NotAllowedError" })).toContain("Privacidade → Microfone");
    expect(micMessage({ name: "NotFoundError" })).toBe("Nenhum microfone encontrado.");
    expect(micMessage(new Error("boom"))).toBe("boom");
  });
});

describe("wavPcm16", () => {
  it("monta o cabeçalho de WAV 16 kHz mono 16-bit", () => {
    const b = new DataView(wavPcm16(new Float32Array([0, 1, -1, 2]), 16000));
    const txt = (o: number) => String.fromCharCode(...[0, 1, 2, 3].map((i) => b.getUint8(o + i)));
    expect(txt(0)).toBe("RIFF");
    expect(txt(8)).toBe("WAVE");
    expect(b.getUint16(22, true)).toBe(1);
    expect(b.getUint32(24, true)).toBe(16000);
    expect(b.getUint32(40, true)).toBe(8);
    expect(b.getInt16(46, true)).toBe(32767);
    expect(b.getInt16(48, true)).toBe(-32768);
    expect(b.getInt16(50, true)).toBe(32767);
  });
});

describe("paraWav16k", () => {
  it("devolve o original quando não dá para decodificar", async () => {
    const original = new Blob([new Uint8Array([1, 2, 3])], { type: "audio/webm" });
    expect(await paraWav16k(original)).toBe(original);
  });
});
