import { afterEach, describe, expect, it, vi } from "vitest";
import { playAudio } from "./speech";

describe("playAudio", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("ao terminar a fala, revoga o blob da URL", async () => {
    const revoke = vi.fn();
    let audio: { onended: (() => void) | null } | null = null;
    vi.stubGlobal("Audio", class {
      onended: (() => void) | null = null;
      constructor() { audio = this; }
      play() { return Promise.resolve(); }
      pause() {}
    });
    URL.createObjectURL = vi.fn(() => "blob:mp3");
    URL.revokeObjectURL = revoke;
    await playAudio("audio/mpeg", btoa("mp3"));
    expect(revoke).not.toHaveBeenCalled();
    audio!.onended!();
    expect(revoke).toHaveBeenCalledWith("blob:mp3");
  });

  it("monta o blob com o mime recebido (WAV da voz local)", async () => {
    vi.stubGlobal("Audio", class {
      onended: (() => void) | null = null;
      play() { return Promise.resolve(); }
      pause() {}
    });
    const create = vi.fn((_b: Blob) => "blob:wav");
    URL.createObjectURL = create;
    URL.revokeObjectURL = vi.fn();
    await playAudio("audio/wav", btoa("RIFF"));
    expect(create.mock.calls[0][0].type).toBe("audio/wav");
  });
});
