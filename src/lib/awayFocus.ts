import { useEffect } from "react";
import { useStore } from "../state/store";

const MINUTO = 60_000;

/** Ao voltar para a janela depois de `ausencia.minutos` sem foco, o digest abre com o que aconteceu desde a saída. */
export function useAwayFocus() {
  useEffect(() => {
    const onBlur = () => useStore.getState().patch({ blurredAt: Date.now() });
    const onFocus = () => {
      const st = useStore.getState();
      const saiu = st.blurredAt;
      const minutos = st.settings?.ausencia?.minutos ?? 10;
      st.patch({ blurredAt: null });
      if (saiu && minutos > 0 && st.active && Date.now() - saiu >= minutos * MINUTO) st.patch({ digestSince: saiu });
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    return () => {
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
    };
  }, []);
}
