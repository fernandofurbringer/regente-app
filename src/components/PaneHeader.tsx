import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { createPortal } from "react-dom";
import { closeQuestion, paneStateChip } from "../lib/attention";
import { byHeadroom, otherAccounts, showAccountBadge } from "../lib/accounts";
import { costLabel } from "../lib/costLabel";
import { fmtTokens, fmtUsd } from "../lib/format";
import { accountPeak, colorFor } from "../lib/redline";
import { cacheTip, paneUsageText, usageTotalText } from "../lib/usageLabel";
import { api, errMsg, type PaneDetails, type PaneRow } from "../lib/ipc";
import { isOrchestrator, roleLabel } from "../lib/roles";
import { isRemote } from "../lib/remote";
import { startPointerDrag } from "../lib/pointerDrag";
import { fixPrompt } from "../lib/paneErrors";
import type { ErrorBlock } from "../lib/errors";
import { useStore, type SendTo } from "../state/store";
import HibernateReasons from "./HibernateReasons";
import PromptPicker from "./PromptPicker";
import Modal from "./Modal";
import ProfileChip from "./ProfileChip";
import ContextMeter from "./ContextMeter";
import { ALIVE, terms } from "./TerminalPane";
import { continueInAccount } from "./trocaConta";

// "pend" não passa argumento: vale o modo configurado no CLI (o selo não promete "perguntar")
const PERM: Record<string, string> = { danger: "Perigo", auto: "Auto", pend: "Padrão do CLI", plan: "Plano" };
const KIND: Record<string, string> = { terminal: "Terminal", browser: "Navegador", image: "Imagem", video: "Vídeo", agent: "IA" };
const STATUS: Record<string, string> = {
  iniciando: "Iniciando", trabalhando: "Trabalhando", ocioso: "Ocioso", concluido: "Concluído", erro: "Erro",
  fechando: "Fechando", hibernado: "Hibernado", acordando: "Acordando", pergunta: "Aguardando resposta",
};
/** Estado escrito ao lado do ponto (o ponto só tem cor): "Aguardando resposta", "Erro", "Hibernado" e o tempo de "Trabalhando". */
function PaneStateChip({ pane }: { pane: PaneRow }) {
  const since = useStore((s) => s.statusSince[pane.id]);
  const working = pane.status === "trabalhando";
  const [, tick] = useState(0);
  useEffect(() => {
    if (!working) return;
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [working]);
  const chip = paneStateChip(pane.status, since, Date.now());
  return chip ? <span className="state-chip" data-tone={chip.tone}>{chip.text}</span> : null;
}

// zustand 5 + React 19: seletor não pode devolver um [] novo a cada chamada (loop de render)
const NO_ACCOUNTS: string[] = [];
const stop = (e: SyntheticEvent) => e.stopPropagation();

export default function PaneHeader({ pane, index, standalone = false }: {
  pane: PaneRow; index: number; standalone?: boolean;
}) {
  const providers = useStore((s) => s.providers);
  const accounts = useStore((s) => (pane.cli ? s.settings?.accounts[pane.cli] : undefined)) ?? NO_ACCOUNTS;
  const focused = useStore((s) => s.focus[pane.workspaceId] === pane.id);
  const errors = useStore((s) => s.paneErrors[pane.id]);
  const lastError = errors?.length ? errors[errors.length - 1] : null;
  const limits = useStore((s) => s.limits);
  const acctPct = limits && pane.kind === "agent" ? accountPeak(limits, pane.cli, pane.account || "principal") : null;
  const voiceTarget = useStore((s) => s.voiceTarget === pane.id);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(pane.name);
  /** posição (fixed) do menu ⋯; null = fechado */
  const [menu, setMenu] = useState<{ right: number; top?: number; bottom?: number } | null>(null);
  const [mem, setMem] = useState<number | null>(null);
  const [details, setDetails] = useState<PaneDetails | null>(null);
  // execuções de shell deste painel do Claude desligadas (estado do back-end; vale até o app reiniciar)
  const [execOff, setExecOff] = useState(false);
  useEffect(() => {
    if (pane.kind === "agent" && pane.cli === "claude") api.execDesligado(pane.id).then(setExecOff).catch(() => {});
  }, [pane.id, pane.kind, pane.cli]);
  const menuRef = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  /** o renomear já foi salvo ou cancelado (Enter → blur; Esc → cancela sem salvar) */
  const renameDone = useRef(false);
  const { patch, patchPane, toast } = useStore.getState();
  const cli = providers?.clis.find((c) => c.id === pane.cli);
  const route = providers?.routes.find((r) => r.id === pane.route);
  const alive = ALIVE.includes(pane.status);
  // tokens = entrada+saída; o cache (re-cobrado a cada turno) só aparece na dica e no menu
  const cacheNote = cacheTip(pane.tokCacheR, pane.tokCacheW);
  // remoto: só o custo do Claude é lido (por ssh); os outros CLIs ficam sem custo
  const remoteNoCost = isRemote(pane.cwd) && pane.cli !== "claude";
  const costText = costLabel({ cliCost: cli?.cost, remoteNoCost, usd: pane.costUsd });
  const status = STATUS[pane.status] ?? pane.status;
  const role = roleLabel(pane.role, pane.agent);
  const orchestrator = isOrchestrator(pane.role);
  const missionTotal = useStore((s) => (pane.missionId ? s.missionTotals[pane.missionId] : undefined));
  const [reasonsOpen, setReasonsOpen] = useState(false);
  const [promptsOpen, setPromptsOpen] = useState(false);

  useEffect(() => setName(pane.name), [pane.name]);

  useEffect(() => {
    if (!menu) return;
    const inside = (t: EventTarget | null) => t instanceof Node && (!!menuRef.current?.contains(t) || !!trigger.current?.contains(t));
    const onDoc = (e: MouseEvent) => { if (!inside(e.target)) setMenu(null); };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setMenu(null);
      trigger.current?.focus();
    };
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    // o foco vai para a 1ª ação do menu, nunca para um chip (o de compactar recomeçaria a conversa com um Enter)
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  const closeMenu = () => {
    setMenu(null);
    trigger.current?.focus();
  };
  const run = (f: () => Promise<unknown>) => () => { closeMenu(); f().catch((e) => toast(errMsg(e), "erro")); };

  function startRename() {
    setMenu(null);
    renameDone.current = false;
    setEditing(true);
  }
  async function saveName() {
    if (renameDone.current) return;
    renameDone.current = true;
    const n = name.trim();
    setEditing(false);
    if (!n || n === pane.name) return setName(pane.name);
    try {
      await api.paneRename(pane.id, n);
      patchPane(pane.id, { name: n });
    } catch (e) {
      toast(errMsg(e), "erro");
      setName(pane.name);
    }
  }
  /** Abre a saída inteira do painel (disco + memória) como texto somente leitura no editor de arquivos. */
  async function openHistory() {
    const text = await api.paneHistory(pane.id);
    const st = useStore.getState();
    st.patch({
      openFile: { ...st.openFile, [pane.workspaceId]: { path: `historico-${pane.id}.txt`, text } },
      view: { ...st.view, [pane.workspaceId]: "arquivos" },
    });
  }
  function openMenu() {
    const r = trigger.current!.getBoundingClientRect();
    // abre para cima quando o botão está na metade de baixo da janela
    setMenu(r.bottom > window.innerHeight / 2
      ? { right: window.innerWidth - r.right, bottom: window.innerHeight - r.top + 2 }
      : { right: window.innerWidth - r.right, top: r.bottom + 2 });
    if (alive) api.paneDetails(pane.id).then((d) => setMem(d.memoryMb)).catch(() => setMem(null));
  }
  function toggleFocus() {
    patch({ focus: { ...useStore.getState().focus, [pane.workspaceId]: focused ? null : pane.id } });
  }
  async function fullscreen() {
    const w = getCurrentWindow();
    const on = !(await w.isFullscreen());
    await w.setFullscreen(on);
    patch({ focus: { ...useStore.getState().focus, [pane.workspaceId]: on ? pane.id : null } });
  }
  async function detach() {
    const label = `pane-${pane.id}`;
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) return existing.setFocus();
    const w = new WebviewWindow(label, { url: `index.html#/pane/${pane.workspaceId}/${pane.id}`, title: pane.name, width: 960, height: 640, dragDropEnabled: true });
    const back = () => patch({ detached: { ...useStore.getState().detached, [pane.id]: false } });
    patch({ detached: { ...useStore.getState().detached, [pane.id]: true } });
    w.once("tauri://destroyed", back);
    // a janela não abriu: o painel volta para a grade
    w.once("tauri://error", (e) => { back(); toast(`Não foi possível abrir a janela: ${errMsg(e.payload)}`, "erro"); });
  }
  async function close() {
    const q = closeQuestion(pane);
    if (q && !(await ask(q, { title: "Fechar painel", kind: "warning" }))) return;
    await api.paneClose(pane.id);
    if (standalone) await getCurrentWindow().close();
  }

  return (
    <div className="pane-head" data-voice-target={voiceTarget || undefined} data-perm={pane.permission === "danger" ? "danger" : undefined} onPointerDown={standalone || editing ? undefined : (e) => { startPointerDrag(e, { kind: "pane", data: pane.id }, pane.name); }}
      onDoubleClick={standalone ? undefined : toggleFocus}>
      <span className="dot" data-s={pane.status} role="img" title={status} aria-label={status} />
      <PaneStateChip pane={pane} />
      <span className="num">#{index + 1}</span>
      {editing ? (
        <span className="name">
          <input autoFocus aria-label="Nome do painel" value={name} onChange={(e) => setName(e.target.value)} onBlur={saveName}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") { renameDone.current = true; setName(pane.name); setEditing(false); }
            }} />
        </span>
      ) : (
        // um clique só foca; renomear é duplo clique ou F2
        <span className="name" role="button" aria-keyshortcuts="F2" tabIndex={0} title={`${pane.name} — duplo clique ou F2 para renomear`}
          onDoubleClick={(e) => { e.stopPropagation(); startRename(); }}
          onKeyDown={(e) => { if (e.key === "F2") { e.preventDefault(); startRename(); } }}>{pane.name}</span>
      )}
      {/* selos secundários em ordem de importância: os que não cabem quebram para uma 2ª linha escondida (some o selo
          inteiro, do fim para o começo, nunca pela metade) e ⋯ e ✕ ficam sempre à vista; tudo deles também está no ⋯ */}
      <div className="pane-head-extras">
        {/* âncora de largura zero e altura da linha: é ela que fica na 1ª linha quando nenhum selo cabe (um selo sozinho
            numa linha mais estreita que ele apareceria cortado) e dá a altura que centraliza os selos */}
        <span className="pane-head-ancora" aria-hidden="true" />
        {lastError && (
          // no cabeçalho, fora do Tab: escondido na 2ª linha ele receberia foco invisível; pelo teclado, o mesmo está no ⋯
          <button className="badge" data-danger tabIndex={-1} title={`Último erro (${lastError.kind}) desta saída: envia ao agente com o contexto`}
            onClick={() => useStore.getState().patch({ sendTo: fixSendTo(pane, lastError) })}>
            Erro detectado — Enviar ao agente
          </button>
        )}
        {pane.kind === "agent" && <span className="badge" data-danger={pane.permission === "danger" || undefined} title="Modo de permissão">{PERM[pane.permission]}</span>}
        {pane.kind === "agent" && <ContextMeter paneId={pane.id} semTab />}
        {pane.kind === "agent" && cli && (
          <button className="badge" tabIndex={-1} style={{ maxWidth: "14ch", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
            title={`${pane.model ?? "modelo padrão"}${pane.effort ? ` · ${pane.effort}` : ""} — trocar modelo e esforço`} aria-label={`Modelo e esforço: ${pane.model ?? "padrão"}${pane.effort ? `, ${pane.effort}` : ""}`} onClick={() => patch({ modelPane: pane.id })}>
            {pane.model ?? "modelo padrão"}{pane.effort ? ` · ${pane.effort}` : ""}
          </button>
        )}
        {/* medidor curto: só o custo; entrada, saída e cache ficam no ⋯ */}
        {orchestrator && missionTotal ? (
          <span className="meter" title={`${usageTotalText(missionTotal.tokens, missionTotal.costUsd)} · custo agregado da missão (inclui painéis fechados) · cache: ${fmtTokens(missionTotal.cacheTokens)} — não conta`}>≈{fmtUsd(missionTotal.costUsd)}</span>
        ) : pane.kind === "agent" && (
          <span className="meter" title={[paneUsageText(pane.tokIn, pane.tokOut, costText.text), costText.title, cacheNote].filter(Boolean).join(" · ") || undefined}>{costText.text}</span>
        )}
        {pane.kind === "agent" && <ProfileChip workspaceId={pane.workspaceId} missionId={pane.missionId} profileId={pane.profile} paneId={pane.id} />}
        <span className="badge">{cli?.label ?? KIND[pane.kind]}</span>
        {role && <span className="badge role" data-accent={pane.role === "piloto" || pane.role === "maestro" ? "" : undefined}>{role}</span>}
        {route && <span className="route" title={`Rota: ${route.label}`} aria-label={`Rota: ${route.label}`}>⇄</span>}
        {pane.kind === "agent" && showAccountBadge(accounts, pane.account) && <span className="badge" title={`Conta: ${pane.account || "principal"}`} aria-label={`Conta: ${pane.account || "principal"}`}>◉</span>}
        {acctPct !== null && acctPct >= 90 && <span className="badge" style={{ color: colorFor(acctPct) }} title="A conta deste painel passou de 90% do limite (Limites no topo)">Conta {Math.round(acctPct)}%</span>}
        {pane.resident && <span className="badge" title="Não hiberna" aria-label="Não hiberna">∞</span>}
      </div>
      {standalone && <button className="icon-btn" onClick={() => getCurrentWindow().close()}>Devolver à grade</button>}
      <button ref={trigger} className="icon-btn" title="Mais ações do painel" aria-label={pane.kind === "browser" || pane.kind === "image" || pane.kind === "video" ? "Mais ações" : "Mais ações do terminal"} aria-haspopup="menu" aria-expanded={!!menu} onClick={() => (menu ? setMenu(null) : openMenu())}>⋯</button>
      {menu && createPortal(
        <div className="menu" role="menu" ref={menuRef} style={{ position: "fixed", ...menu }}
          onDoubleClick={stop}
          onKeyDown={(e) => {
            if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
            e.preventDefault();
            const items = [...e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
            const n = items.length;
            const i = items.indexOf(document.activeElement as HTMLButtonElement);
            const down = e.key === "ArrowDown";
            items[i < 0 ? (down ? 0 : n - 1) : (i + (down ? 1 : n - 1)) % n]?.focus();
          }}>
          {orchestrator && missionTotal && (
            <div className="menu-info">Missão: {usageTotalText(missionTotal.tokens, missionTotal.costUsd)} (inclui painéis fechados) · cache: {fmtTokens(missionTotal.cacheTokens)} — não conta</div>
          )}
          {pane.kind === "agent" && <div className="menu-info">{paneUsageText(pane.tokIn, pane.tokOut, costText.text)}{costText.title ? ` · ${costText.title}` : ""}</div>}
          {pane.kind === "agent" && <div className="menu-info">{cacheNote}</div>}
          {alive && <div className="menu-info">Memória (processo e filhos): {mem === null ? "…" : `${mem} MB`}</div>}
          {/* tudo o que os selos do cabeçalho mostram, para quando o painel é estreito e eles não cabem */}
          <div className="menu-info">
            {[cli?.label ?? KIND[pane.kind], role, pane.kind === "agent" ? `Permissão: ${PERM[pane.permission]}` : null,
              route ? `Rota: ${route.label}` : null,
              pane.kind === "agent" && showAccountBadge(accounts, pane.account) ? `Conta: ${pane.account || "principal"}` : null,
              acctPct !== null && acctPct >= 90 ? `Conta em ${Math.round(acctPct)}% do limite` : null,
              pane.resident ? "Não hiberna" : null].filter(Boolean).join(" · ")}
          </div>
          {pane.kind === "agent" && (
            <div className="menu-info pane-menu-chips">
              Perfil <ProfileChip workspaceId={pane.workspaceId} missionId={pane.missionId} profileId={pane.profile} paneId={pane.id} />
              <ContextMeter paneId={pane.id} />
            </div>
          )}
          <div className="menu-sep" role="separator" />
          {lastError && (
            <button className="menu-item" role="menuitem" title={`Último erro (${lastError.kind}) desta saída`}
              onClick={run(async () => patch({ sendTo: fixSendTo(pane, lastError) }))}>Erro detectado — Enviar ao agente</button>
          )}
          {pane.kind === "agent" && cli && (
            <button className="menu-item" role="menuitem" onClick={run(async () => patch({ modelPane: pane.id }))}>
              Trocar modelo e esforço ({pane.model ?? "padrão"}{pane.effort ? ` · ${pane.effort}` : ""})…
            </button>
          )}
          {pane.kind === "agent" && pane.cli === "claude" && pane.sessionId && !isRemote(pane.cwd) &&
            byHeadroom(otherAccounts(accounts, pane.account), (c) => (limits ? accountPeak(limits, "claude", c) : null)).map((c) => {
              const pct = limits ? accountPeak(limits, "claude", c) : null;
              return (
                <button key={`conta-${c}`} className="menu-item" role="menuitem" title="Reinicia o painel nesta conta retomando a conversa (se parou no limite, ele continua sozinho)"
                  onClick={run(() => continueInAccount(pane.id, c))}>
                  Continuar na conta {c}{pct !== null ? ` (${Math.round(pct)}% usado)` : ""}
                </button>
              );
            })}
          <button className="menu-item" role="menuitem" onClick={startRename}>Renomear</button>
          {!standalone && <button className="menu-item" role="menuitem" onClick={run(async () => toggleFocus())}>{focused ? "Sair do foco" : "Focar este painel"}</button>}
          {!standalone && <button className="menu-item" role="menuitem" onClick={run(fullscreen)}>Tela cheia</button>}
          {!standalone && <button className="menu-item" role="menuitem" onClick={run(detach)}>Abrir em janela própria</button>}
          {pane.kind === "agent" && alive && (
            <button className="menu-item" role="menuitem" disabled={!pane.sessionId} title={pane.sessionId ? undefined : "Ainda sem sessão para retomar"}
              onClick={run(() => api.paneHibernate(pane.id, terms.get(pane.id)?.screen() ?? ""))}>Hibernar agora</button>
          )}
          {(pane.kind === "agent" || pane.kind === "terminal") && (
            <button className="menu-item" role="menuitem" onClick={run(async () => useStore.getState().patch({ exportPane: pane.id }))}>Copiar ou salvar conversa…</button>
          )}
          {(pane.kind === "agent" || pane.kind === "terminal") && alive && <button className="menu-item" role="menuitem" onClick={run(async () => setPromptsOpen(true))}>Inserir prompt…</button>}
          {(pane.kind === "agent" || pane.kind === "terminal") && <button className="menu-item" role="menuitem" onClick={run(openHistory)}>Abrir histórico completo</button>}
          {alive && <button className="menu-item" role="menuitem" onClick={run(async () => setDetails(await api.paneDetails(pane.id)))}>Detalhes do terminal</button>}
          {alive && pane.kind === "agent" && <button className="menu-item" role="menuitem" onClick={run(async () => setReasonsOpen(true))}>Por que não hiberna</button>}
          {alive && pane.kind === "agent" && <button className="menu-item" role="menuitem" onClick={run(async () => useStore.getState().patch({ composerOpen: { ...useStore.getState().composerOpen, [pane.id]: !useStore.getState().composerOpen[pane.id] } }))}>Compositor (Ctrl+Shift+Enter)</button>}
          {pane.kind === "agent" && pane.cli === "claude" && (
            <button className="menu-item" role="menuitem" onClick={run(async () => { await api.execDesligar(pane.id, !execOff); setExecOff(!execOff); })}>
              {execOff ? "Ligar execuções neste painel" : "Desligar execuções neste painel"}
            </button>
          )}
        </div>,
        document.body,
      )}
      <button className="icon-btn" title="Fechar painel" aria-label="Fechar painel" onClick={() => close().catch((e) => toast(errMsg(e), "erro"))}>✕</button>
      {promptsOpen && <PromptPicker paneId={pane.id} onClose={() => setPromptsOpen(false)} />}
      {reasonsOpen && <HibernateReasons paneId={pane.id} name={pane.name} onClose={() => setReasonsOpen(false)} />}
      {details && (
        <Modal label="Detalhes do terminal" onClose={() => setDetails(null)}>
          <h2>Detalhes do terminal</h2>
          <pre style={{ whiteSpace: "pre-wrap", fontFamily: "var(--mono)", fontSize: 12 }}>
            {[
              `Programa: ${details.program}`,
              `Argumentos: ${details.args.join(" ")}`,
              `Pasta: ${details.cwd}`,
              `Sessão: ${details.sessionId ?? "—"}`,
              `Rota: ${details.routeLabel ?? "—"}`,
              `Skills carregadas: ${details.skills.length ? details.skills.join(", ") : "nenhuma"}`,
              `PID: ${details.pid ?? "—"} · Memória: ${details.memoryMb ?? "—"} MB`,
              "Ambiente:",
              ...details.env.map(([k, v]) => `  ${k}=${v}`),
            ].join("\n")}
          </pre>
          <div className="actions"><button className="btn" onClick={() => setDetails(null)}>Fechar</button></div>
        </Modal>
      )}
    </div>
  );
}

/** "Corrija este erro": o erro, o painel, a pasta e a tela vão para o diálogo de envio ao agente. */
function fixSendTo(pane: PaneRow, block: ErrorBlock): SendTo {
  return { workspaceId: pane.workspaceId, missionId: pane.missionId, title: "Corrigir isto", text: fixPrompt(pane, block, terms.get(pane.id)?.screen() ?? ""), fromPane: pane.id };
}
