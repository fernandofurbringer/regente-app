import { api } from "../lib/ipc";
// ▼ trilha C
import DevServers from "./DevServers";
// ▲ trilha C
import UsageChips from "./UsageChips";
import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import ShotGallery from "./ShotGallery";
import ResourceMonitor from "./ResourceMonitor";
import { fmtMem } from "../lib/resources";
import Modal from "./Modal";
import VoiceButton from "./VoiceButton";
import { budgetBand, fmtUsdPt } from "../lib/format";
import { hojeTip } from "../lib/usageLabel";
import { browserUrl } from "../lib/links";
import { focusNewPane } from "./TerminalPane";
import { useStore, wsOf } from "../state/store";
import { waitingPanes } from "../lib/attention";
import { isInlineSubagent } from "../lib/activity";
import { resolveShortcuts } from "../lib/shortcuts";
import { MODE_LABEL, SHELL_DEFAULT, comArranjo, type GridMode } from "../lib/layout";
import { gridPrefsOf, setGridPrefs, setShellPrefs, useGridPrefs } from "../lib/layoutStore";
import { goToAttention } from "./CommandPalette";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Botão com menu (clique fora e Esc fecham; setas andam pelos itens, como o menu do cabeçalho). */
function BarMenu({ label, ariaLabel, title, align = "right", dot, children }: {
  label: ReactNode; ariaLabel: string; title?: string; align?: "left" | "right"; dot?: boolean; children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = () => { setOpen(false); trigger.current?.focus(); };
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    // o foco vai para o primeiro item, não para o botão que abriu o menu
    ref.current?.querySelector<HTMLButtonElement>(".menu button")?.focus();
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  const arrows = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    const down = e.key === "ArrowDown";
    items[i < 0 ? (down ? 0 : items.length - 1) : (i + (down ? 1 : items.length - 1)) % items.length]?.focus();
  };
  return (
    <div className="bar-menu" ref={ref}>
      <button ref={trigger} className="tb-btn" aria-haspopup="menu" aria-expanded={open} aria-label={ariaLabel} title={title}
        onClick={() => setOpen((o) => !o)}>{label}{dot && <span className="dot" aria-label="Página pronta" style={{ marginLeft: 6, background: "var(--info)" }} />}</button>
      {open && (
        // Tab ou clique fora dos itens tira o foco do menu, e o menu fecha
        <div className="menu" role="menu" data-align={align} onKeyDown={arrows}
          onBlur={(e) => { const to = e.relatedTarget as Node | null; if (!e.currentTarget.contains(to) && to !== trigger.current) setOpen(false); }}
          onMouseDown={(e) => { if (e.target instanceof HTMLElement && e.target.tagName !== "BUTTON") e.preventDefault(); }}>{children(close)}</div>
      )}
    </div>
  );
}

export default function TopBar() {
  const active = useStore((s) => s.active);
  const keys = resolveShortcuts(useStore((s) => s.settings?.shortcuts));
  const grid = useGridPrefs(active ?? "");
  // quantos painéis esperam você (todos os workspaces): mostra no título da janela principal e na barra
  const waiting = useStore((s) => waitingPanes(Object.values(s.panes).filter((p) => !isInlineSubagent(p, s.settings?.subagentPanes ?? false)), s.statusSince, s.gates).length);
  const gates = useStore((s) => s.gates.length);
  const main = getCurrentWindow().label === "main";
  useEffect(() => {
    if (!main) return;
    getCurrentWindow().setTitle(waiting ? `Regente · ● ${waiting} aguardando você` : "Regente").catch(() => {});
  }, [main, waiting]);
  const view = useStore((s) => (s.active ? s.view[s.active] ?? "terminais" : "terminais"));
  const readyUrl = useStore((s) => (s.active ? s.readyUrl[s.active] ?? null : null));
  // a galeria também abre pela paleta de comandos: o estado mora na store
  const shots = useStore((s) => s.shotGallery);
  // RAM do Regente inteiro (árvore de processos) no chip; a leitura é reaproveitada até 2 s pelo servidor
  const [ram, setRam] = useState<number | null>(null);
  const monitorOpen = useStore((s) => s.resourcesOpen);
  useEffect(() => {
    let alive = true;
    // janela escondida não consulta; 2 s só com o monitor aberto, 10 s no resto
    const load = () => {
      if (document.visibilityState !== "visible") return;
      api.resourcesSnapshot().then((s) => { if (alive) setRam(s.total.memMb); }).catch(() => {});
    };
    load();
    const t = setInterval(load, monitorOpen ? 2000 : 10_000);
    document.addEventListener("visibilitychange", load);
    return () => { alive = false; clearInterval(t); document.removeEventListener("visibilitychange", load); };
  }, [monitorOpen]);
  const setShots = (open: boolean) => useStore.getState().patch({ shotGallery: open });
  const totals = useStore((s) => s.totals);
  const settings = useStore((s) => s.settings);
  const { patch, upsertPane, toastErr } = useStore.getState();
  const daily = settings?.budget.dailyUsd || null;
  const band = daily ? budgetBand(totals.today.costUsd, daily) : null;
  // "Hoje" vira à meia-noite sem evento: relê os totais a cada minuto
  useEffect(() => {
    const id = window.setInterval(() => useStore.getState().refreshTotals().catch(() => {}), 60_000);
    return () => window.clearInterval(id);
  }, []);

  async function spawn(kind: "terminal" | "browser") {
    const ws = wsOf(active);
    if (!ws) return;
    try {
      const missionId = useStore.getState().missionSel[ws.id] ?? null;
      const row = await api.paneSpawn({ workspaceId: ws.id, missionId, kind, cwd: ws.path, urlOrPath: kind === "browser" ? browserUrl(readyUrl ?? "http://localhost:3000") : null });
      upsertPane(row);
      focusNewPane(row.id);
      if (kind === "browser") patch({ readyUrl: { ...useStore.getState().readyUrl, [ws.id]: null } });
    } catch (e) {
      toastErr(e);
    }
  }

  return (
    <header className="topbar">
      <span className="brand">REGENTE</span>
      {active && (
        <>
          <div className="seg" role="group" aria-label="Visão da janela">
            <button className={`tb-btn ${view !== "arquivos" ? "on" : ""}`} aria-pressed={view !== "arquivos"} title="Mostrar os terminais da grade"
              onClick={() => patch({ view: { ...useStore.getState().view, [active]: "terminais" } })}>Terminais</button>
            <button className={`tb-btn ${view === "arquivos" ? "on" : ""}`} aria-pressed={view === "arquivos"} title="Mostrar os arquivos do workspace"
              onClick={() => patch({ view: { ...useStore.getState().view, [active]: "arquivos" } })}>Arquivos</button>
          </div>
          <BarMenu label="⊞" ariaLabel="Grade dos painéis" title={`Grade: ${MODE_LABEL[grid.modo]} (${keys.gridMode} troca o modo)`} align="left">
            {(close) => (
              <>
                {(["igual", "principal-esq", "principal-cima", "colunas"] as GridMode[]).map((m) => (
                  <button key={m} className="menu-item" role="menuitemradio" aria-checked={grid.modo === m}
                    onClick={() => { close(); setGridPrefs(active, comArranjo(gridPrefsOf(active), m)); }}>
                    {grid.modo === m ? "✓ " : ""}{MODE_LABEL[m]}
                  </button>
                ))}
                <div className="menu-sep" role="separator" />
                <div className="menu-info">Colunas (painéis iguais)</div>
                {(["auto", 1, 2, 3] as const).map((c) => (
                  <button key={String(c)} className="menu-item" role="menuitemradio" aria-checked={grid.modo === "igual" && grid.cols === c}
                    onClick={() => { close(); setGridPrefs(active, comArranjo(gridPrefsOf(active), "igual", c)); }}>
                    {grid.modo === "igual" && grid.cols === c ? "✓ " : ""}{c === "auto" ? "Automático" : `${c} ${c === 1 ? "coluna" : "colunas"}`}
                  </button>
                ))}
                <div className="menu-sep" role="separator" />
                <button className="menu-item" role="menuitem"
                  onClick={() => { close(); setGridPrefs(active, { ...gridPrefsOf(active), colFr: [], rowPx: {} }); setShellPrefs({ ...SHELL_DEFAULT }); }}>
                  Redefinir tamanhos
                </button>
              </>
            )}
          </BarMenu>
          <div className="split">
            <button className="tb-btn split-main" title={`Novo painel de IA (${keys.newPanel} abre como o último)`} onClick={() => patch({ dialog: "painel" })}>
              + Painel <span className="kbd">{keys.newPanel}</span>
            </button>
            <BarMenu label="▾" ariaLabel="Outros tipos de painel" title="Terminal, navegador ou missão" align="left" dot={!!readyUrl}>
              {(close) => (
                <>
                  <button className="menu-item" role="menuitem" title={`Novo terminal (${keys.newTerminal})`} onClick={() => { close(); spawn("terminal"); }}>Terminal</button>
                  <button className="menu-item" role="menuitem" title={readyUrl ? `Página pronta: ${readyUrl}` : "Abrir navegador"} onClick={() => { close(); spawn("browser"); }}>
                    Navegador{readyUrl && <span className="dot" aria-label="Página pronta" style={{ marginLeft: 8, background: "var(--info)" }} />}
                  </button>
                  <button className="menu-item" role="menuitem" title={`Nova missão (${keys.newMission})`} onClick={() => { close(); patch({ dialog: "missao" }); }}>Missão…</button>
                </>
              )}
            </BarMenu>
          </div>
        </>
      )}
      <span className="spacer" />
      {main && gates > 0 && (
        <button className="tb-btn pulse" title={`Abrir o ponto de aprovação pendente (gate, ${keys.gate})`} onClick={() => patch({ gateOpen: true })}>Gate {gates}</button>
      )}
      {main && waiting > 0 && (
        <button className="tb-btn pulse" title={`Ir para o painel que aguarda resposta (${keys.attention})`} onClick={() => goToAttention()}>● {waiting}</button>
      )}
      <UsageChips />
      <VoiceButton />
      <button className="tb-stat tb-hoje" title={hojeTip(totals, daily)} onClick={() => patch({ settingsOpen: true, settingsSection: "uso" })}>
        Hoje {fmtUsdPt(totals.today.costUsd)}
        {band && (
          <span className="budget-bar" data-band={band.band} role="progressbar" aria-label="Orçamento diário usado" aria-valuenow={Math.round(band.pct)} aria-valuemin={0} aria-valuemax={100}>
            <i style={{ width: `${Math.min(100, band.pct)}%` }} />
          </span>
        )}
      </button>
      <button className="tb-stat tb-ram" title="Monitor de recursos: memória do Regente e dos painéis" onClick={() => patch({ resourcesOpen: true })}>
        RAM {ram === null ? "…" : fmtMem(ram)}
      </button>
      <BarMenu label="⋯" ariaLabel="Mais ações da barra" title="Mais ações">
        {(close) => (
          <button className="menu-item" role="menuitem" onClick={() => { close(); setShots(true); }}>Capturas de tela</button>
        )}
      </BarMenu>
      <button className="icon-btn" aria-label="Configurações" title="Configurações" onClick={() => patch({ settingsOpen: true })}>⚙</button>
      <ResourceMonitor />
      {/* ▼ trilha C */}
      <DevServers />
      {/* ▲ trilha C */}
      {shots && (
        <Modal label="Galeria de capturas" onClose={() => setShots(false)} style={{ width: "min(900px, 92vw)", maxHeight: "86vh", overflow: "auto" }}>
          <h2>Galeria de capturas</h2>
          <ShotGallery />
          <div className="actions"><button className="btn" onClick={() => setShots(false)}>Fechar</button></div>
        </Modal>
      )}
    </header>
  );
}
