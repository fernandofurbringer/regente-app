import { useEffect, useRef, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** Menu flutuante preso ao corpo da página: barras que rolam (faixa de missões, abas do editor) cortariam um menu dentro
 *  delas. Abre sob `at`, ou sobre ele quando `at` está na metade de baixo da janela (rodapé). Fecha com clique fora ou Esc;
 *  o clique no botão que o abriu (marcado com `data-menu-trigger`) fica com o botão, que alterna. O primeiro botão recebe
 *  o foco; ao fechar, o foco volta a quem o tinha, se ninguém o pegou (campo de renomear, terminal clicado). */
export default function FloatMenu({ at, onClose, label, className = "", children }: {
  at: DOMRect; onClose(): void; label: string; className?: string; children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Element;
      // só o botão deste menu (o aberto: aria-expanded) fica de fora; clicar em outro gatilho fecha este, e nunca há dois abertos
      if (!ref.current?.contains(t) && !t.closest?.('[data-menu-trigger][aria-expanded="true"]')) onClose();
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
      const a = document.activeElement;
      if ((!a || a === document.body) && prev?.isConnected) prev.focus();
    };
  }, [onClose]);
  const left = Math.max(8, Math.min(at.left, window.innerWidth - 288));
  const up = at.top > window.innerHeight / 2;
  return createPortal(
    <div ref={ref} className={`menu float-menu ${className}`} role="menu" aria-label={label}
      style={{ position: "fixed", left, ...(up ? { bottom: window.innerHeight - at.top + 4 } : { top: at.bottom + 4 }) }}>
      {children}
    </div>,
    document.body,
  );
}
