import { useEffect, useRef, useState } from "react";
import { api } from "../lib/ipc";
import { colorFor, labelOf, resetText } from "../lib/redline";
import { usageChips } from "../lib/usageChips";
import { useStore } from "../state/store";

/** Uso do Claude fixo no cabeçalho (/usage): um chip por janela (5h, 7d) das contas no topo, com percentual, barra, cor e
 *  tempo até reiniciar. O clique abre o detalhe de todas as contas e janelas, com "Atualizar agora" e o histórico. */
export default function UsageChips() {
  const limits = useStore((s) => s.limits);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  // o "reinicia em" anda a cada minuto, mesmo sem leitura nova (a leitura vem a cada 5 min)
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(t);
  }, []);
  const ref = useRef<HTMLDivElement>(null);
  // o popover fecha ao clicar fora dele ou com Esc
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    const esc = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);
  const chips = usageChips(limits, now);

  async function refresh() {
    setBusy(true);
    try {
      useStore.getState().patch({ limits: await api.redlineNow() });
      setNow(Date.now());
    } catch (e) {
      useStore.getState().toastErr(e);
    } finally {
      setBusy(false);
    }
  }

  // sem leitura ainda: "…"; com leitura mas sem conta do Claude no topo: "—" (sem token ou conta desligada)
  const empty = limits === null ? "…" : chips.length === 0 ? "—" : null;
  const emptyTitle = limits === null ? "Lendo os limites do Claude…" : "Sem leitura do Claude: confira o login do Claude Code e Configurações → Limites de uso";
  return (
    <div className="limit-wrap" ref={ref}>
      {/* o botão é quem fala pelo leitor de tela: os chips dentro dele são só visuais */}
      <button className="tb-btn usage-btn" aria-expanded={open} aria-haspopup="dialog"
        aria-label={empty ? "Uso do Claude" : `Uso do Claude: ${chips.map((c) => `${c.account ? `${c.account} ` : ""}${c.label} ${c.pct}%${c.stale ? " (leitura antiga)" : ""}`).join(", ")}`}
        title={empty ? emptyTitle : "Uso do Claude (/usage): clique para o detalhe"}
        onClick={() => setOpen((o) => !o)}>
        {empty ? <span className="usage-chip"><span className="meta">uso {empty}</span></span> : chips.map((c) => (
          <span key={c.key} className="usage-chip" title={c.title}>
            {c.account && <span className="meta">{c.account}</span>}
            <span>{c.label}</span>
            <span className="usage-bar"><i style={{ width: `${Math.min(100, c.pct)}%`, background: c.color }} /></span>
            <span style={{ color: c.color }}>{c.stale && "~"}{c.pct}%</span>
            {c.reset && <span className="reset">{c.reset}</span>}
          </span>
        ))}
      </button>
      {open && limits && (
        <div className="limit-pop" role="dialog" aria-label="Limites das contas">
          {limits.windows.length === 0 && <div className="meta">Nenhuma leitura ainda.</div>}
          {limits.windows.map((w) => {
            const reset = resetText(w.resetsAt, now);
            const stale = limits.stale.some((st) => st.provider === w.provider && st.account === w.account);
            return (
              <div key={`${w.provider}/${w.account}/${w.window}`} className="limit-row">
                <div className="row" style={{ justifyContent: "space-between", gap: 8 }}>
                  <span>{w.provider.startsWith("claude") ? "Claude" : "Codex"} · {w.account} · {labelOf(w.window)}</span>
                  <strong style={{ color: colorFor(w.usedPercent) }}>{stale && "~"}{Math.round(w.usedPercent)}%</strong>
                </div>
                <div className="limit-bar"><i style={{ width: `${Math.min(100, w.usedPercent)}%`, background: colorFor(w.usedPercent) }} /></div>
                {reset && <span className="meta">{reset}</span>}
              </div>
            );
          })}
          <div className="actions">
            <button className="btn" disabled={busy} onClick={() => void refresh()}>{busy ? "Atualizando…" : "Atualizar agora"}</button>
            <button className="btn" onClick={() => { setOpen(false); api.redlineOpen().catch(() => {}); }}>Abrir histórico</button>
          </div>
        </div>
      )}
    </div>
  );
}
