import type { PointerEvent as ReactPointerEvent } from "react";

/** Payload do `regente-drop`: o que foi arrastado e o tipo de arrasto (cada zona filtra o `kind` que aceita). */
export interface DragPayload { kind: string; data: string }

/** Distância (px) que o ponteiro precisa andar para virar arrasto; abaixo disso é clique. */
export const DRAG_THRESHOLD = 5;

/** Pura: o ponteiro já andou o bastante para começar o arrasto? */
export function pastThreshold(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) >= DRAG_THRESHOLD;
}

/** Pura: a zona de soltar sob (x, y) é o `[data-drop]` mais interno. `pick` é o elementFromPoint (injetável). */
export function dropTargetAt(
  x: number,
  y: number,
  pick: (x: number, y: number) => { closest(sel: string): Element | null } | null = (x, y) => document.elementFromPoint(x, y),
): Element | null {
  return pick(x, y)?.closest("[data-drop]") ?? null;
}

// controles e campos não iniciam arrasto: o clique neles continua sendo clique
const INTERACTIVE = "button,input,select,textarea,a,[contenteditable]";

/**
 * Arrasto por ponteiro: substitui o drag HTML5, que o WebView2 desliga quando o drop nativo está ligado.
 * Começa depois de DRAG_THRESHOLD px; Esc cancela; resolve quando o arrasto termina.
 * Ao soltar sobre uma zona `[data-drop]`, dispara `regente-drop` nela (com `bubbles`).
 */
export function startPointerDrag(e: PointerEvent | ReactPointerEvent, payload: DragPayload, label: string): Promise<void> {
  if (e.button !== 0 || (e.target as Element).closest?.(INTERACTIVE)) return Promise.resolve();
  const x0 = e.clientX;
  const y0 = e.clientY;
  return new Promise<void>((done) => {
    let ghost: HTMLDivElement | null = null;
    let over: Element | null = null;
    const light = (el: Element | null) => {
      if (el === over) return;
      over?.classList.remove("drop-over");
      over = el;
      el?.classList.add("drop-over");
    };
    const move = (ev: PointerEvent) => {
      if (!ghost) {
        if (!pastThreshold(ev.clientX - x0, ev.clientY - y0)) return;
        ghost = document.createElement("div");
        ghost.className = "drag-ghost";
        ghost.textContent = label;
        document.body.append(ghost);
        document.body.classList.add("pointer-drag");
        document.body.style.cursor = "grabbing";
      }
      ghost.style.transform = `translate(${ev.clientX + 14}px, ${ev.clientY + 14}px)`;
      light(dropTargetAt(ev.clientX, ev.clientY));
    };
    const finish = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", finish);
      window.removeEventListener("keydown", onKey, true);
      light(null);
      ghost?.remove();
      document.body.classList.remove("pointer-drag");
      document.body.style.cursor = "";
      done();
    };
    const up = (ev: PointerEvent) => {
      const drop = ghost ? dropTargetAt(ev.clientX, ev.clientY) : null;
      if (ghost) swallowClick();
      finish();
      drop?.dispatchEvent(new CustomEvent("regente-drop", { detail: payload, bubbles: true }));
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.stopPropagation(); // o Esc cancela o arrasto; não fecha o modal de trás
      finish();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", finish);
    window.addEventListener("keydown", onKey, true);
  });
}

/** Um arrasto real termina com o clique do ponteiro solto: engole esse clique para não ativar o item. */
function swallowClick() {
  const stop = (ev: Event) => { ev.stopPropagation(); ev.preventDefault(); };
  window.addEventListener("click", stop, { capture: true, once: true });
  setTimeout(() => window.removeEventListener("click", stop, { capture: true }), 0);
}
