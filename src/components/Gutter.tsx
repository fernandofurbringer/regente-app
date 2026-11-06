import { useRef, type CSSProperties, type KeyboardEvent, type PointerEvent } from "react";

const PASSO = 24;

/** Divisor arrastável (bloco C): arrasta com o ponteiro, setas movem 24 px, duplo clique volta ao padrão.
 *  `onStart` guarda o tamanho no começo; `onMove` recebe o deslocamento total desde o começo; `onEnd` salva. */
export default function Gutter({ axis, label, onStart, onMove, onEnd, onReset, className = "", style }: {
  axis: "x" | "y"; label: string;
  onStart(): void; onMove(delta: number): void; onEnd(): void; onReset(): void;
  className?: string; style?: CSSProperties;
}) {
  const inicio = useRef<number | null>(null);
  const pos = (e: PointerEvent) => (axis === "x" ? e.clientX : e.clientY);
  const down = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    inicio.current = pos(e);
    // iframes (navegador) e o modal não engolem o ponteiro durante o arrasto (mesma classe do arrasto de painéis)
    document.body.classList.add("pointer-drag");
    document.body.style.cursor = axis === "x" ? "col-resize" : "row-resize";
    onStart();
  };
  const move = (e: PointerEvent<HTMLDivElement>) => {
    // clique sem arrasto não fixa tamanho nenhum: só movimento de verdade muda a grade
    if (inicio.current !== null && pos(e) !== inicio.current) onMove(pos(e) - inicio.current);
  };
  const up = () => {
    if (inicio.current === null) return;
    inicio.current = null;
    document.body.classList.remove("pointer-drag");
    document.body.style.cursor = "";
    onEnd();
  };
  const key = (e: KeyboardEvent<HTMLDivElement>) => {
    const menos = axis === "x" ? "ArrowLeft" : "ArrowUp";
    const mais = axis === "x" ? "ArrowRight" : "ArrowDown";
    if (e.key !== menos && e.key !== mais) return;
    e.preventDefault();
    onStart();
    onMove(e.key === mais ? PASSO : -PASSO);
    onEnd();
  };
  return (
    <div role="separator" aria-orientation={axis === "x" ? "vertical" : "horizontal"} aria-label={label} tabIndex={0}
      title={`${label}: arraste; setas ajustam; duplo clique volta ao padrão`}
      className={`gutter gutter-${axis} ${className}`.trim()} style={style}
      onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={up} onKeyDown={key} onDoubleClick={onReset} />
  );
}
