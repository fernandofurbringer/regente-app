import { useEffect, useRef, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { matches, parseHotkey } from "../lib/hotkey";
import { api, errCode, errMsg } from "../lib/ipc";
import { micMessage, VoiceRecorder } from "../lib/recorder";
import { shownPanes } from "../lib/visible";
import { useStore } from "../state/store";

type Phase = "idle" | "gravando" | "transcrevendo";

/**
 * Voz: ditado no painel ativo. Segurar a tecla grava enquanto ela está apertada; alternar
 * começa e envia no segundo aperto; Esc cancela. O botão faz o mesmo que a tecla em modo alternar.
 */
export default function VoiceButton() {
  const voice = useStore((s) => s.voice);
  const targetName = useStore((s) => (s.voiceTarget ? s.panes[s.voiceTarget]?.name : undefined));
  const [phase, setPhase] = useState<Phase>("idle");
  const [level, setLevel] = useState(0);
  const ctl = useRef<{ click(): void } | null>(null);

  useEffect(() => {
    api.voiceGet().then((c) => useStore.getState().patch({ voice: c })).catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  }, []);

  useEffect(() => {
    const toast = (text: string, kind: "info" | "erro" = "erro") => useStore.getState().toast(text, kind);
    let rec: VoiceRecorder | null = null;
    let target: string | null = null; // painel de destino, fixado quando a gravação começa
    let busy = false; // o microfone está abrindo
    let pending: boolean | null = null; // envio/cancelamento pedido enquanto o microfone abria
    let timer: number | null = null;
    let held = false; // a tecla de segurar está apertada

    const stopMeter = () => {
      if (timer !== null) clearInterval(timer);
      timer = null;
      setLevel(0);
    };

    async function start() {
      if (busy || rec) return;
      const st = useStore.getState();
      const pane = st.activePane;
      const row = pane ? st.panes[pane] : undefined;
      if (!pane || !row) return toast("Nenhum painel ativo: clique num terminal antes de ditar.", "info");
      // o texto vai para o painel ativo mesmo que ele esteja oculto (outra missão, grupo recolhido, outra aba): avisa antes
      const visible = st.active === row.workspaceId && (st.view[st.active] ?? "terminais") === "terminais" && shownPanes(row.workspaceId).some((p) => p.id === pane);
      if (!visible && !(await ask(`O painel de destino "${row.name}" está oculto. Ditar mesmo assim?`, { title: "Painel oculto", kind: "warning" }))) return;
      busy = true;
      const r = new VoiceRecorder(() => finish(true));
      try {
        await r.start();
        rec = r;
        target = pane;
        useStore.getState().patch({ voiceTarget: pane });
        setPhase("gravando");
        timer = window.setInterval(() => setLevel(r.level()), 80);
      } catch (e) {
        r.cancel();
        toast(micMessage(e));
      } finally {
        busy = false;
      }
      if (pending !== null) {
        const send = pending;
        pending = null;
        if (rec) finish(send);
      }
    }

    async function finish(send: boolean) {
      if (busy) {
        pending = send;
        return;
      }
      const r = rec;
      if (!r) return;
      rec = null;
      stopMeter();
      useStore.getState().patch({ voiceTarget: null });
      if (!send) {
        r.cancel();
        setPhase("idle");
        return;
      }
      setPhase("transcrevendo");
      try {
        const { blob, mime } = await r.stop();
        // quebras de linha viram espaço: no terminal um Enter enviaria o texto antes da hora
        const text = (await api.voiceTranscribe(blob, mime, "pane")).replace(/\s*\n\s*/g, " ").trim();
        if (!text) return toast("Nada foi reconhecido.", "info");
        const auto = useStore.getState().voice?.autoEnter;
        await api.paneWrite(target!, text + (auto ? "\r" : ""));
      } catch (e) {
        // sem a chave do motor de voz, o erro leva direto às configurações de voz
        if (errCode(e) === "missing_secret") useStore.getState().toast(errMsg(e), "erro", { label: "Configurar voz", run: () => useStore.getState().patch({ settingsOpen: true, settingsSection: "voz" }) });
        else toast(errMsg(e));
      } finally {
        setPhase("idle");
      }
    }

    const onKeyDown = (e: KeyboardEvent) => {
      const st = useStore.getState();
      const cfg = st.voice;
      if (!cfg || st.settingsOpen) return;
      if (e.code === "Escape" && (rec || busy)) {
        e.preventDefault();
        e.stopPropagation();
        finish(false);
        return;
      }
      if (!matches(e, cfg.hotkey)) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      if (cfg.hold) held = true;
      if (!cfg.hold && rec) finish(true);
      else start();
    };
    const onKeyUp = (e: KeyboardEvent) => {
      const st = useStore.getState();
      if (!st.voice?.hold || st.settingsOpen) return;
      if (e.code === parseHotkey(st.voice.hotkey).code) {
        held = false;
        finish(true);
      }
    };
    // a janela perdeu o foco com a tecla segurada: o keyup pode nunca chegar, então a gravação é descartada
    const onBlur = () => {
      if (!held) return;
      held = false;
      finish(false);
    };

    ctl.current = { click: () => (rec ? finish(true) : start()) };
    // capture: a tecla chega antes do xterm, que não deve receber o atalho
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("blur", onBlur);
      stopMeter();
      rec?.cancel();
      rec = null;
      useStore.getState().patch({ voiceTarget: null });
      ctl.current = null;
    };
  }, []);

  const label = phase === "gravando" ? `● VOZ → ${targetName ?? "painel"}` : phase === "transcrevendo" ? "transcrevendo…" : "VOZ";
  return (
    <>
      {phase === "gravando" && (
        <span className="vol" role="meter" aria-label="Nível do microfone" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(level * 100)}>
          <span className="lvl-bar"><i style={{ width: `${level * 100}%` }} /></span>
        </span>
      )}
      <button
        className={`tb-btn voice ${phase === "gravando" ? "on" : ""}`}
        aria-pressed={phase === "gravando"}
        disabled={phase === "transcrevendo"}
        title={`Ditar no painel ativo (${voice?.hotkey ?? "Ctrl+Space"})`}
        onClick={() => ctl.current?.click()}
      >
        {label}
      </button>
    </>
  );
}
