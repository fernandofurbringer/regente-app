/** Toca um áudio que o backend devolveu em base64 (WAV da voz local, mp3 do ElevenLabs de Mídia). */

let current: HTMLAudioElement | null = null;
let currentUrl: string | null = null;

/** Plays audio that the backend returned in base64 (`audio/wav` da voz local, `audio/mpeg` do ElevenLabs). Replaces whatever is playing. */
export async function playAudio(mime: string, base64: string): Promise<void> {
  stopSpeaking();
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  currentUrl = URL.createObjectURL(new Blob([bytes], { type: mime }));
  const audio = new Audio(currentUrl);
  current = audio;
  // ao terminar a fala, libera o blob (sem isso a URL fica viva até a próxima fala); só se ainda for esta
  audio.onended = () => {
    if (current === audio) stopSpeaking();
  };
  await audio.play();
}

function stopSpeaking(): void {
  current?.pause();
  current = null;
  if (currentUrl) URL.revokeObjectURL(currentUrl);
  currentUrl = null;
}
