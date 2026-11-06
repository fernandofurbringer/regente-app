import { useEffect } from "react";
import { api, errMsg, on } from "../lib/ipc";
import { micMessage, VoiceRecorder } from "../lib/recorder";
import { useStore } from "../state/store";

/**
 * Ditado global (qualquer app): o Rust manda `global-voice` (start, stop, cancel) pelo atalho. O microfone é
 * aberto aqui, na janela principal, mesmo com o Regente em segundo plano. Um stop que chega enquanto o
 * microfone ainda abre é guardado e aplicado assim que a gravação começar.
 */
export default function GlobalVoice() {
  useEffect(() => {
    let rec: VoiceRecorder | null = null;
    let starting = false;
    let stopAfterStart = false;
    let cancelAfterStart = false;

    async function finish() {
      const r = rec;
      if (!r) return;
      rec = null;
      try {
        const { blob, mime } = await r.stop();
        await api.voiceGlobalFinish(blob, mime);
      } catch (e) {
        useStore.getState().toast(errMsg(e), "erro");
      }
    }

    async function start() {
      starting = true;
      const r = new VoiceRecorder(() => finish());
      try {
        await r.start();
        rec = r;
      } catch (e) {
        r.cancel();
        api.voiceGlobalCancel().catch(() => {});
        useStore.getState().toast(micMessage(e), "erro");
      } finally {
        starting = false;
      }
      if (rec && cancelAfterStart) {
        rec.cancel();
        rec = null;
      } else if (rec && stopAfterStart) {
        finish();
      }
      cancelAfterStart = false;
      stopAfterStart = false;
    }

    const subs = [
      on<{ state: "start" | "stop" | "cancel" }>("global-voice", (e) => {
        if (e.state === "start") {
          if (!rec && !starting) start();
        } else if (e.state === "stop") {
          if (starting) stopAfterStart = true;
          else finish();
        } else if (e.state === "cancel") {
          if (starting) cancelAfterStart = true;
          rec?.cancel();
          rec = null;
        }
      }),
    ];
    return () => {
      subs.forEach((p) => p.then((u) => u()));
      rec?.cancel();
      rec = null;
    };
  }, []);
  return null;
}
