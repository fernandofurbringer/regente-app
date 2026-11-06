import { useStore } from "../state/store";

/** Microfone no WebView2: MediaRecorder (webm/opus) para o áudio e AnalyserNode para o nível (barra VOL). O áudio sai em WAV 16 kHz mono (ver stop). */

/** A gravação para sozinha depois disso (o áudio fica dentro do limite de 25 MB da transcrição). */
export const MAX_RECORD_MS = 5 * 60 * 1000;

export class VoiceRecorder {
  private stream: MediaStream | null = null;
  private rec: MediaRecorder | null = null;
  private ctx: AudioContext | null = null;
  private analyser: AnalyserNode | null = null;
  private samples = new Float32Array(1024);
  private chunks: Blob[] = [];
  private limit: number | null = null;

  /** `onLimit` é chamado quando a gravação bate o limite: quem gravou decide o que fazer (enviar). */
  constructor(private onLimit?: () => void) {}

  /** Rejeita com DOMException quando o microfone é negado ou inexistente (ver micMessage). */
  async start(): Promise<void> {
    this.stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    const type = "audio/webm;codecs=opus";
    this.rec = MediaRecorder.isTypeSupported(type) ? new MediaRecorder(this.stream, { mimeType: type }) : new MediaRecorder(this.stream);
    this.rec.ondataavailable = (e) => {
      if (e.data.size) this.chunks.push(e.data);
    };
    this.rec.start(250);
    this.ctx = new AudioContext();
    this.analyser = this.ctx.createAnalyser();
    this.analyser.fftSize = 1024;
    this.ctx.createMediaStreamSource(this.stream).connect(this.analyser);
    this.limit = window.setTimeout(() => this.onLimit?.(), MAX_RECORD_MS);
  }

  /** Nível atual, 0..1 (RMS do sinal com ganho para fala normal). */
  level(): number {
    if (!this.analyser) return 0;
    this.analyser.getFloatTimeDomainData(this.samples);
    return Math.min(1, rms(this.samples) * 4);
  }

  /** Para a gravação e devolve o áudio em WAV 16 kHz mono (ou o original, se não decodificar). Se o MediaRecorder já
   * parou sozinho, devolve o que foi gravado. */
  async stop(): Promise<{ blob: Blob; mime: string }> {
    const rec = this.rec;
    if (!rec) throw new Error("a gravação não estava ativa");
    const bruto = rec.state === "inactive"
      ? this.take()
      : await new Promise<{ blob: Blob; mime: string }>((resolve) => {
          rec.onstop = () => resolve(this.take());
          rec.stop();
        });
    if (!bruto.blob.size) return bruto;
    // nuvem recebe o webm/opus (menor); o WAV 16 kHz só serve ao motor local
    const engine = useStore.getState().voice?.engine;
    if (engine === "groq" || engine === "openai") return bruto;
    const wav = await paraWav16k(bruto.blob);
    return { blob: wav, mime: wav === bruto.blob ? bruto.mime : "audio/wav" };
  }

  /** Para e descarta o áudio. */
  cancel(): void {
    if (this.rec && this.rec.state !== "inactive") {
      this.rec.onstop = null;
      this.rec.stop();
    }
    this.release();
  }

  /** Junta o áudio gravado e libera o microfone. */
  private take(): { blob: Blob; mime: string } {
    const mime = (this.rec?.mimeType || "audio/webm").split(";")[0];
    const blob = new Blob(this.chunks, { type: mime });
    this.release();
    return { blob, mime };
  }

  private release(): void {
    if (this.limit !== null) clearTimeout(this.limit);
    this.limit = null;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.ctx?.close().catch(() => {});
    this.stream = null;
    this.rec = null;
    this.ctx = null;
    this.analyser = null;
    this.chunks = [];
  }
}

export function rms(samples: Float32Array): number {
  let sum = 0;
  for (const v of samples) sum += v * v;
  return Math.sqrt(sum / (samples.length || 1));
}

/** Amostras f32 (-1..1) → WAV PCM 16-bit mono. */
export function wavPcm16(samples: Float32Array, rate = 16000): ArrayBuffer {
  const buf = new ArrayBuffer(44 + samples.length * 2);
  const v = new DataView(buf);
  const str = (o: number, s: string) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  str(0, "RIFF"); v.setUint32(4, 36 + samples.length * 2, true); str(8, "WAVE");
  str(12, "fmt "); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, rate, true); v.setUint32(28, rate * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  str(36, "data"); v.setUint32(40, samples.length * 2, true);
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    v.setInt16(44 + i * 2, s < 0 ? s * 0x8000 : s * 0x7fff, true);
  }
  return buf;
}

/** O que o Whisper usa: WAV 16 kHz mono (o motor local dispensa o ffmpeg). O decodificador do navegador já reamostra
 * para a taxa do contexto. Se não decodificar, devolve o áudio original (o backend converte). */
export async function paraWav16k(blob: Blob): Promise<Blob> {
  try {
    const ctx = new OfflineAudioContext(1, 1, 16000);
    const buf = await ctx.decodeAudioData(await blob.arrayBuffer());
    const mono = new Float32Array(buf.length);
    for (let c = 0; c < buf.numberOfChannels; c++) {
      const ch = buf.getChannelData(c);
      for (let i = 0; i < ch.length; i++) mono[i] += ch[i] / buf.numberOfChannels;
    }
    return new Blob([wavPcm16(mono, 16000)], { type: "audio/wav" });
  } catch {
    return blob;
  }
}

/** Mensagem para o usuário quando o microfone não abre. */
export function micMessage(e: unknown): string {
  const name = (e as { name?: string } | null)?.name;
  if (name === "NotAllowedError" || name === "SecurityError") return "Permita o microfone: Configurações do Windows → Privacidade → Microfone";
  if (name === "NotFoundError" || name === "OverconstrainedError") return "Nenhum microfone encontrado.";
  return e instanceof Error ? e.message : String(e);
}
