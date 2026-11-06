import { useEffect, useRef, type CSSProperties, type ReactNode, type SyntheticEvent } from "react";
import { createPortal } from "react-dom";

const stop = (e: SyntheticEvent) => e.stopPropagation();
const FOCUSABLE = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])';

/** Diálogo modal no body: foca o primeiro campo, Tab circula dentro dele, Esc fecha, devolve o foco ao fechar.
 *  Eventos não sobem pela árvore React (ex.: para o cabeçalho arrastável do painel). */
export default function Modal({ label, onClose, backdropCloses = true, autoFocus = true, style, children }: {
  label: string; onClose(): void; backdropCloses?: boolean; autoFocus?: boolean; style?: CSSProperties; children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const el = ref.current;
    if (autoFocus) (el?.querySelector<HTMLElement>("input, select, textarea") ?? el?.querySelector<HTMLElement>("button"))?.focus();
    // sem foco automático, o próprio diálogo recebe o foco (Esc e Tab já funcionam daí)
    else el?.focus();
    // foco fora do diálogo (ex.: botão desabilitado que caiu no body): Tab leva ao primeiro campo.
    // Com foco dentro, o onKeyDown do diálogo cuida do ciclo.
    const onDocTab = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || !el || el.contains(document.activeElement)) return;
      const first = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)].find((x) => x.offsetParent !== null);
      if (!first) return;
      e.preventDefault();
      first.focus();
    };
    document.addEventListener("keydown", onDocTab);
    return () => {
      document.removeEventListener("keydown", onDocTab);
      prev?.focus();
    };
  }, []);
  return createPortal(
    <div className="modal-backdrop" onClick={(e) => { e.stopPropagation(); if (backdropCloses) onClose(); }}
      onDoubleClick={stop} onMouseDown={stop}>
      <div ref={ref} className="modal" role="dialog" aria-modal="true" aria-label={label} tabIndex={-1} style={style} onClick={stop}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
          if (e.key !== "Tab") return;
          const els = [...e.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((x) => x.offsetParent !== null);
          if (!els.length) return;
          const [first, last] = [els[0], els[els.length - 1]];
          const at = document.activeElement;
          if (e.shiftKey ? at === first || !e.currentTarget.contains(at) : at === last) {
            e.preventDefault();
            (e.shiftKey ? last : first).focus();
          }
        }}>
        {children}
      </div>
    </div>,
    document.body,
  );
}
