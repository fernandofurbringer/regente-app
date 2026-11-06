import { ask } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { fmtTokens, fmtUsd } from "../lib/format";
import { isInlineSubagent } from "../lib/activity";
import { usageTotalText } from "../lib/usageLabel";
import { api, errCode, errMsg, type Mission, type PaneStatus, type WorktreeInfo } from "../lib/ipc";
import { adotadasComBranchMudada, branchChip, fechaDireto } from "../lib/worktreeState";
import { isRemote } from "../lib/remote";
import { useStore } from "../state/store";
import { DIRTY_NOTE, dirtyIn } from "../lib/review";
import { attentionChips, attentionTooltip } from "../lib/attention";
import ExternalSessions from "./ExternalSessions";
import FloatMenu from "./FloatMenu";
import { confirmIntegrate } from "./MissionBar";

const STATUS: Record<string, string> = { ativa: "Ativa", pronta: "Pronta para integrar", bloqueada: "Bloqueada", concluida: "Concluída", arquivada: "Arquivada" };
const PRIORITY: PaneStatus[] = ["erro", "pergunta", "trabalhando", "acordando", "iniciando", "ocioso", "hibernado", "concluido"];
const NO_MISSIONS: Mission[] = []; // seletor estável (zustand 5)
/** ids dos menus da faixa que não são de uma missão (ids de missão são UUID: não colidem) */
const ADOTAR = "__adotar";
const ARQUIVADAS = "__arquivadas";
const SESSOES = "__sessoes";
/** último segmento do caminho (nome da pasta do worktree) */
const pasta = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

/** Faixa de missões acima da grade (no lugar do rail lateral): "Todos", uma aba fina por missão (estado, branch do
 *  worktree, atenção, entregas e custo), "+ Adotar", "+ Missão", as arquivadas e, no remoto, as sessões externas.
 *  Clique escolhe; duplo clique renomeia; clique direito ou ⋯ abre as ações. Rola na horizontal (roda do mouse também). */
export default function MissionStrip({ wsId }: { wsId: string }) {
  const missions = useStore((s) => s.missions[wsId]) ?? NO_MISSIONS;
  const sel = useStore((s) => s.missionSel[wsId] ?? null);
  // subagentes inline (faixa do painel) não contam como painéis
  const panes = useStore(useShallow((s) => Object.values(s.panes).filter((p) => p.workspaceId === wsId && !isInlineSubagent(p, s.settings?.subagentPanes ?? false))));
  const totals = useStore((s) => s.missionTotals);
  const unseen = useStore((s) => s.unseen);
  const attention = useStore((s) => s.attention);
  const ws = useStore((s) => s.workspaces.find((w) => w.id === wsId));
  const [menu, setMenu] = useState<{ id: string; at: DOMRect } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  /** missão integrando/arquivando agora (sem clique duplo) */
  const [busy, setBusy] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [wts, setWts] = useState<WorktreeInfo[] | null>(null);
  const [inRepo, setInRepo] = useState(false);
  const stripRef = useRef<HTMLElement>(null);
  const { toast, loadMissions } = useStore.getState();
  const fechar = useCallback(() => setMenu(null), []);
  /** abre o menu `id` sob o elemento; de novo no mesmo botão, fecha */
  const abrir = (id: string, el: Element) => setMenu((m) => (m?.id === id ? null : { id, at: el.getBoundingClientRect() }));

  useEffect(() => {
    let live = true;
    api.gitIsRepo(wsId).then((r) => { if (live) setInRepo(r); }).catch(() => { if (live) setInRepo(false); });
    return () => { live = false; };
  }, [wsId]);
  // o ambiente.sh troca a branch de um ambiente por fora, inclusive dos que não estão escolhidos: confere todos a cada
  // 15 s com um `git worktree list` só; o estado da missão mudada grava a branch nova no banco antes de recarregar
  useEffect(() => {
    if (!inRepo) return;
    const t = setInterval(async () => {
      const ms = useStore.getState().missions[wsId] ?? [];
      if (document.hidden || !ms.some((m) => m.adopted)) return;
      const ids = adotadasComBranchMudada(ms, await api.worktreesList(wsId).catch(() => []));
      if (!ids.length) return;
      await Promise.all(ids.map((id) => api.missionWorktreeState(id).catch(() => null)));
      await useStore.getState().loadMissions(wsId).catch(() => {});
    }, 15_000);
    return () => clearInterval(t);
  }, [wsId, inRepo]);
  // a missão escolhida fica à vista na faixa (Ctrl+número, paleta e "ir para" também mudam a seleção)
  useEffect(() => {
    stripRef.current?.querySelector<HTMLElement>(".mstrip-item.sel, .mstrip-all.sel")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [sel]);

  const liveStatus = (m: Mission): PaneStatus | null => {
    const sts = panes.filter((p) => p.missionId === m.id).map((p) => p.status);
    return PRIORITY.find((s) => sts.includes(s)) ?? null;
  };
  const select = (id: string | null) => useStore.getState().selectMission(wsId, id);
  const update = (m: Mission, p: Partial<Mission>) => api.missionUpdate({ ...m, ...p });
  /** o editor tem alterações não salvas dentro do worktree da missão */
  const isDirty = (m: Mission) => dirtyIn(useStore.getState().dirty[wsId] ?? [], m.worktreePath);
  async function act(f: () => Promise<unknown>, ok?: string, id?: string) {
    setMenu(null);
    if (id) setBusy(id);
    try {
      await f();
      if (ok) toast(ok, "ok");
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      if (id) setBusy(null);
      await loadMissions(wsId).catch((e) => toast(errMsg(e), "erro"));
    }
  }
  const abrirAdotar = (el: Element) => {
    if (menu?.id === ADOTAR) return setMenu(null);
    setMenu({ id: ADOTAR, at: el.getBoundingClientRect() });
    setWts(null);
    api.worktreesList(wsId).then(setWts).catch((e) => { toast(errMsg(e), "erro"); setWts([]); });
  };
  const adotar = (w: WorktreeInfo) => act(async () => {
    const m = await api.missionAdopt(wsId, w.path);
    select(m.id);
  }, "Worktree adotado como missão");
  async function integrate(m: Mission) {
    if (!(await confirmIntegrate(m, wsId))) return setMenu(null);
    act(() => api.missionIntegrate(m.id), "Missão integrada", m.id);
  }
  async function archive(m: Mission) {
    const what = m.worktreePath
      ? " e o worktree removido; alterações não comitadas viram um commit 'wip' na branch (a branch fica)"
      : "";
    const ok = await ask(`Arquivar "${m.name}"? Os painéis serão fechados${what}.${isDirty(m) ? ` ${DIRTY_NOTE}` : ""}`, { title: "Arquivar missão", kind: "warning" });
    if (!ok) return setMenu(null);
    act(async () => {
      try {
        await api.missionArchive(m.id);
      } catch (e) {
        // arquivo em uso (dev server, build, editor de fora): o WIP já virou commit; dá para arquivar e deixar a pasta
        if (errCode(e) !== "worktree_locked") throw e;
        const mesmoAssim = await ask(`${errMsg(e)}

Arquivar mesmo assim? O trabalho já está commitado na branch; a pasta fica e o Regente tenta apagá-la na próxima vez que abrir.`, { title: "Arquivar missão", kind: "warning", okLabel: "Arquivar mesmo assim", cancelLabel: "Cancelar" });
        if (!mesmoAssim) return;
        await api.missionArchive(m.id, true);
      }
      if (useStore.getState().missionSel[wsId] === m.id) select(null);
      toast("Missão arquivada", "ok");
    }, undefined, m.id);
  }
  /** Missão adotada: fecha os painéis e some da faixa (mesmo `missionArchive`; o worktree do projeto fica como está). */
  async function close(m: Mission) {
    const sts = panes.filter((p) => p.missionId === m.id).map((p) => p.status);
    if (!fechaDireto(sts, isDirty(m))) {
      const ok = await ask(`Fechar "${m.name}"? Os painéis serão fechados; o worktree fica como está.${isDirty(m) ? ` ${DIRTY_NOTE}` : ""}`, { title: "Fechar missão", kind: "warning" });
      if (!ok) return setMenu(null);
    }
    act(async () => {
      await api.missionArchive(m.id);
      if (useStore.getState().missionSel[wsId] === m.id) select(null);
    }, "Missão fechada", m.id);
  }
  function startRename(m: Mission) {
    setMenu(null);
    setRenaming(m.id);
    setName(m.name);
  }
  const patch = (p: Parameters<ReturnType<typeof useStore.getState>["patch"]>[0]) => { setMenu(null); useStore.getState().patch(p); };

  /** As ações de uma missão (mesmas do rail antigo). */
  const acoes = (m: Mission) => {
    const working = busy === m.id;
    return (
      <>
        <div className="menu-info">{m.name} · {STATUS[m.status]}{m.statusReason ? `: ${m.statusReason}` : ""}{m.branch ? ` · ${m.branch}` : ""}</div>
        <button className="menu-item" role="menuitem" onClick={() => startRename(m)}>Renomear</button>
        <button className="menu-item" role="menuitem" onClick={() => act(() => update(m, { pinned: !m.pinned }))}>{m.pinned ? "Desafixar" : "Fixar"}</button>
        {m.worktreePath && !m.adopted && m.status !== "arquivada" && (
          <>
            {m.status !== "pronta" && m.status !== "concluida" && (
              <button className="menu-item" role="menuitem" onClick={() => act(() => update(m, { status: "pronta" }))}>Marcar pronta para integrar</button>
            )}
            {m.status !== "concluida" && (
              <button className="menu-item" role="menuitem" disabled={working} onClick={() => integrate(m)}>Integrar em {m.baseBranch}</button>
            )}
          </>
        )}
        {m.worktreePath && m.status !== "arquivada" && (
          <button className="menu-item" role="menuitem" onClick={() => patch({ reviewMission: m.id })}>Revisar mudanças</button>
        )}
        <button className="menu-item" role="menuitem" onClick={() => patch({ verifyMission: m.id })}>Verificação…</button>
        {m.worktreePath && <button className="menu-item" role="menuitem" onClick={() => patch({ timeline: { scope: m.id, title: m.name, mission: true } })}>Linha do tempo</button>}
        {m.worktreePath && m.status !== "arquivada" && (
          <>
            <button className="menu-item" role="menuitem" onClick={() => patch({ gitDialog: { kind: "commit", wsId, missionId: m.id } })}>Commitar…</button>
            <button className="menu-item" role="menuitem" onClick={() => patch({ gitDialog: { kind: "pr", wsId, missionId: m.id } })}>Abrir PR…</button>
          </>
        )}
        {m.status !== "arquivada" && (
          <button className="menu-item" role="menuitem" disabled={working} onClick={() => (m.adopted ? close(m) : archive(m))}>{m.adopted ? "Fechar" : "Arquivar"}</button>
        )}
      </>
    );
  };

  const chip = (m: Mission) => {
    if (renaming === m.id) {
      return (
        <input key={m.id} className="mstrip-rename" autoFocus aria-label="Nome da missão" value={name} onChange={(e) => setName(e.target.value)}
          onBlur={() => { setRenaming(null); if (name.trim() && name.trim() !== m.name) act(() => update(m, { name: name.trim() })); }}
          onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); if (e.key === "Escape") setRenaming(null); }} />
      );
    }
    const t = totals[m.id];
    const news = unseen[m.id] ?? 0;
    const st = liveStatus(m);
    const att = attentionChips(attention[m.id]);
    const bc = branchChip(m);
    const on = sel === m.id;
    // a aba é curta; o resto (estado, branch, atenção, uso com cache) vai na dica
    const dica = [
      m.name,
      `${STATUS[m.status]}${m.statusReason ? `: ${m.statusReason}` : ""}`,
      m.branch ? `branch ${m.branch}` : m.adopted ? "worktree livre (sem branch)" : null,
      attentionTooltip(attention[m.id]) || null,
      t ? `${usageTotalText(t.tokens, t.costUsd)} · cache ${fmtTokens(t.cacheTokens)} (não conta)` : null,
      "Duplo clique renomeia · clique direito abre as ações",
    ].filter(Boolean).join("\n");
    return (
      <div key={m.id} className={`mstrip-item ${on ? "sel" : ""}`}>
        <button className="mstrip-chip" aria-pressed={on} title={dica}
          onClick={() => select(m.id)} onDoubleClick={() => startRename(m)}
          onContextMenu={(e) => { e.preventDefault(); setMenu({ id: m.id, at: e.currentTarget.getBoundingClientRect() }); }}>
          <span className="dot" data-s={st ?? (m.status === "bloqueada" ? "erro" : "concluido")} />
          <span className="mstrip-name">{m.pinned ? "📌 " : ""}{m.name}</span>
          {bc && <span className="chip">{bc}</span>}
          {att[0] && <span className="chip" data-s={att[0].tone}>{att[0].text}{att.length > 1 ? ` +${att.length - 1}` : ""}</span>}
          {news > 0 && <span className="chip">{news} {news > 1 ? "entregas" : "entrega"}</span>}
          {busy === m.id ? <span className="mstrip-meta">…</span> : t && t.costUsd > 0 ? <span className="mstrip-meta">{fmtUsd(t.costUsd)}</span> : null}
        </button>
        {m.status !== "arquivada" && (
          <button className="mstrip-more" aria-label={`${m.adopted ? "Fechar" : "Arquivar"} a missão ${m.name}`} disabled={busy === m.id}
            title={m.adopted ? "Fechar a missão: fecha os painéis (o worktree fica)" : "Arquivar a missão: fecha os painéis e tira o worktree (o trabalho fica commitado na branch)"}
            onClick={() => (m.adopted ? close(m) : archive(m))}>✕</button>
        )}
        <button className="mstrip-more" data-menu-trigger="" aria-label={`Ações da missão ${m.name}`} aria-haspopup="menu" aria-expanded={menu?.id === m.id}
          onClick={(e) => abrir(m.id, e.currentTarget.parentElement ?? e.currentTarget)}>⋯</button>
      </div>
    );
  };

  const visible = missions.filter((m) => m.status !== "arquivada");
  const archived = missions.filter((m) => m.status === "arquivada" && !m.adopted);
  const daMissao = menu ? missions.find((m) => m.id === menu.id) : undefined;
  return (
    <nav className="mstrip" aria-label="Missões" ref={stripRef}
      // os menus são portais filhos da faixa no React: a roda dentro deles não pode rolar a faixa
      onWheel={(e) => { if (e.deltaY && !e.deltaX && e.currentTarget.contains(e.target as Node)) e.currentTarget.scrollLeft += e.deltaY; }}>
      <button className={`mstrip-all ${sel === null ? "sel" : ""}`} aria-pressed={sel === null} title="Todos os painéis do workspace" onClick={() => select(null)}>
        Todos <span className="mstrip-meta">{panes.length}</span>
      </button>
      {visible.map(chip)}
      {inRepo && (
        <button className="mstrip-btn" data-menu-trigger="" aria-haspopup="menu" aria-expanded={menu?.id === ADOTAR}
          title="Transformar um worktree já existente do repositório numa missão (nada muda no git)" onClick={(e) => abrirAdotar(e.currentTarget)}>+ Adotar</button>
      )}
      <button className="mstrip-btn" title="Nova missão (Ctrl+Shift+M)" onClick={() => useStore.getState().patch({ dialog: "missao" })}>+ Missão</button>
      {archived.length > 0 && (
        <button className="mstrip-btn" data-menu-trigger="" aria-haspopup="menu" aria-expanded={menu?.id === ARQUIVADAS} onClick={(e) => abrir(ARQUIVADAS, e.currentTarget)}>
          Arquivadas ({archived.length}) ▾
        </button>
      )}
      {ws && isRemote(ws.path) && (
        <button className="mstrip-btn" data-menu-trigger="" aria-haspopup="menu" aria-expanded={menu?.id === SESSOES} onClick={(e) => abrir(SESSOES, e.currentTarget)}>
          Sessões externas ▾
        </button>
      )}
      {menu && daMissao && <FloatMenu at={menu.at} onClose={fechar} label={`Ações da missão ${daMissao.name}`} className="mstrip-menu">{acoes(daMissao)}</FloatMenu>}
      {menu?.id === ADOTAR && (
        <FloatMenu at={menu.at} onClose={fechar} label="Adotar worktree" className="mstrip-menu">
          {wts === null && <div className="menu-info">Lendo os worktrees…</div>}
          {wts?.filter((w) => !w.adopted).map((w) => (
            <button key={w.path} className="menu-item" role="menuitem" title={w.path} onClick={() => { setMenu(null); void adotar(w); }}>
              {pasta(w.path)} <span className="meta">{w.branch ?? "livre"}</span>
            </button>
          ))}
          {wts && wts.every((w) => w.adopted) && (
            <div className="menu-info">{wts.length === 0 ? "Este repositório não tem outros worktrees." : "Todos os worktrees já são missões."}</div>
          )}
        </FloatMenu>
      )}
      {menu?.id === ARQUIVADAS && (
        <FloatMenu at={menu.at} onClose={fechar} label="Missões arquivadas" className="mstrip-menu">
          {archived.map((m) => (
            // escolher uma arquivada a seleciona e troca este menu pelas ações dela (renomear, fixar, verificação, linha do tempo)
            <button key={m.id} className="menu-item" role="menuitem" title="Selecionar e ver as ações" onClick={() => { select(m.id); setMenu({ id: m.id, at: menu.at }); }}>{m.name}</button>
          ))}
        </FloatMenu>
      )}
      {menu?.id === SESSOES && (
        <FloatMenu at={menu.at} onClose={fechar} label="Sessões externas" className="mstrip-menu">
          <ExternalSessions wsId={wsId} />
        </FloatMenu>
      )}
    </nav>
  );
}
