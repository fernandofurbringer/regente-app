import { useEffect, useRef, useState } from "react";
import { useStore, type Toast } from "../state/store";
import { visibleToasts } from "../lib/toasts";

/** Um aviso: o tempo para sumir corre só sem o ponteiro nem o foco nele (hover e foco pausam, cada um à parte). */
function ToastItem({ t }: { t: Toast }) {
  const dismiss = useStore((s) => s.dismiss);
  const [hover, setHover] = useState(false);
  const [focus, setFocus] = useState(false);
  const [copied, setCopied] = useState(false);
  const paused = hover || focus;
  const left = useRef(t.ms);
  const since = useRef(0);
  useEffect(() => {
    if (paused) return;
    since.current = Date.now();
    const id = window.setTimeout(() => dismiss(t.id), left.current);
    return () => {
      window.clearTimeout(id);
      left.current -= Date.now() - since.current;
    };
  }, [paused, t.id, dismiss]);
  // "Copiado" aparece e o aviso some logo depois
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => dismiss(t.id), 1500);
    return () => window.clearTimeout(id);
  }, [copied, t.id, dismiss]);
  return (
    <div className="toast" data-kind={t.kind}
      onMouseEnter={() => setHover(true)} onMouseLeave={() => setHover(false)}
      onFocus={() => setFocus(true)}
      onBlur={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setFocus(false); }}>
      <span className="toast-text" onClick={() => dismiss(t.id)}>{t.text}</span>
      {t.action && <button className="btn" onClick={() => { dismiss(t.id); t.action?.run(); }}>{t.action.label}</button>}
      {t.action?.alt && <button className="btn" onClick={() => { dismiss(t.id); t.action?.alt?.run(); }}>{t.action.alt.label}</button>}
      {t.kind === "erro" && (
        <button className="btn" disabled={copied} onClick={() => { navigator.clipboard?.writeText(t.text).catch(() => {}); setCopied(true); }}>
          {copied ? "Copiado" : "Copiar"}
        </button>
      )}
      <button className="toast-x" aria-label="Fechar aviso" onClick={() => dismiss(t.id)}>×</button>
    </div>
  );
}

/** Pilha de avisos: no máximo 4 visíveis ("+N" para o resto) e o sino com os últimos 20 (fora da região viva). */
export default function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const log = useStore((s) => s.toastLog);
  const [bell, setBell] = useState(false);
  // avisos do sino vistos até este id (ids crescem; o log vem do mais novo)
  const [seenId, setSeenId] = useState(0);
  const unseen = log.filter((t) => t.id > seenId).length;
  const { shown, hidden } = visibleToasts(toasts);
  const toggleBell = () => {
    if (!bell) setSeenId(log[0]?.id ?? 0);
    setBell((b) => !b);
  };
  // a lista de avisos fecha com clique fora ou Esc (ela ficava aberta por cima dos botões)
  const pilha = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!bell) return;
    const fora = (e: MouseEvent) => { if (!pilha.current?.contains(e.target as Node)) setBell(false); };
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape") setBell(false); };
    document.addEventListener("mousedown", fora);
    document.addEventListener("keydown", esc);
    return () => { document.removeEventListener("mousedown", fora); document.removeEventListener("keydown", esc); };
  }, [bell]);
  return (
    <div className="toast-stack" ref={pilha}>
      <div role="status" aria-live="polite" style={{ display: "grid", gap: 6 }}>
        {shown.map((t) => <ToastItem key={t.id} t={t} />)}
        {hidden > 0 && <div className="toast-more">+{hidden}</div>}
      </div>
      {bell && log.length > 0 && (
        <div className="toast-log">
          <div className="row" style={{ justifyContent: "space-between", gap: 8 }}>
            <span className="meta">Avisos recentes</span>
            <button className="btn" onClick={() => { useStore.getState().clearToastLog(); setBell(false); }}>Limpar</button>
          </div>
          <ul aria-label="Avisos recentes" style={{ listStyle: "none", margin: 0, padding: 0 }}>
            {log.map((t) => <li key={t.id} data-kind={t.kind}>{t.text}</li>)}
          </ul>
        </div>
      )}
      {log.length > 0 && (
        <button className="toast-bell" aria-expanded={bell} onClick={toggleBell}>
          Avisos ({log.length}{unseen > 0 ? `, ${unseen} novos` : ""})
        </button>
      )}
    </div>
  );
}
