import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect } from "react";
import { goToPane } from "./lib/focus";
import { openTrim } from "./components/ShotTrim";
import { api, errMsg, on, type CompactPropostaEvent, type GateOpen, type JarvisConfirm, type MissionAttention, type PaneContextEvent, type PaneGroup, type PaneRow, type PaneStatus, type SlotEvent, type StatusEvent, type UsageEvent } from "./lib/ipc";
import { formatDroppedPaths, quoted } from "./lib/paths";
import { remover } from "./lib/devServers";
import { busyCount } from "./lib/attention";
import type { RPayload } from "./lib/redline";
import { takeRecovery } from "./lib/recovery";
import { byHeadroom } from "./lib/accounts";
import { accountPeak } from "./lib/redline";
import { continueInAccount } from "./components/trocaConta";
import { useStore, wsOf, type OpenFile } from "./state/store";
import { pendingNoticeToast } from "./lib/notices";

const WRITABLE = ["terminal", "agent"];
const RUNNING: PaneStatus[] = ["iniciando", "trabalhando", "ocioso", "pergunta"];

/** Cola texto no painel (sem Enter) se for um terminal rodando; senão avisa. */
function pasteInto(paneId: string | null, text: string, noTarget: string) {
  const st = useStore.getState();
  const pane = paneId ? st.panes[paneId] : undefined;
  if (!pane || !WRITABLE.includes(pane.kind)) return st.toast(noTarget, "erro");
  if (!RUNNING.includes(pane.status)) return st.toast(`${pane.name} não está rodando`, "erro");
  api.paneWrite(pane.id, text).catch((e) => st.toast(errMsg(e), "erro"));
}

/** Arquivos soltos do Explorer (drop nativo do Tauri): sobre um painel viram caminhos colados nele; sobre a árvore de
 *  arquivos são copiados para a pasta sob o cursor (a árvore relê com `regente:fs-changed`). */
export function useFileDrop() {
  useEffect(() => {
    let lit: Element | null = null;
    const light = (el: Element | null) => {
      if (el === lit) return;
      lit?.classList.remove("drop-over");
      lit = el;
      el?.classList.add("drop-over");
    };
    const unlisten = getCurrentWebview().onDragDropEvent(({ payload }) => {
      if (payload.type === "leave") return light(null);
      // a posição vem em pixels físicos; o elementFromPoint quer CSS
      const dpr = window.devicePixelRatio || 1;
      const alvo = document.elementFromPoint(payload.position.x / dpr, payload.position.y / dpr);
      const pasta = alvo?.closest<HTMLElement>('[data-drop="file-move"][data-dir]') ?? null;
      const cell = pasta ?? alvo?.closest("[data-pane-id]") ?? null;
      if (payload.type !== "drop") return light(cell);
      light(null);
      if (pasta?.dataset.dir) return void copiarParaArvore(payload.paths, pasta.dataset.dir);
      pasteInto(cell?.getAttribute("data-pane-id") ?? null, formatDroppedPaths(payload.paths), "Solte sobre um painel de terminal ou sobre a árvore de arquivos");
    });
    return () => {
      light(null);
      unlisten.then((f) => f());
    };
  }, []);
}

/** Copia os arquivos soltos do Explorer para a pasta da árvore; um toast diz quantos foram (e o primeiro erro). */
async function copiarParaArvore(paths: string[], dir: string) {
  const { toast } = useStore.getState();
  let ok = 0;
  let erro: unknown = null;
  for (const p of paths) {
    try {
      await api.fsCopy(p, dir);
      ok++;
    } catch (e) {
      erro ??= e;
    }
  }
  window.dispatchEvent(new CustomEvent("regente:fs-changed", { detail: { root: null, dirs: [dir] } }));
  if (ok) toast(ok === 1 ? "Arquivo copiado para a pasta" : `${ok} itens copiados para a pasta`, "ok");
  if (erro) toast(errMsg(erro), "erro");
}

export function useAppEvents() {
  useEffect(() => {
    let timer: number | undefined;
    /** workspaces com uso novo: o placar das missões deles é recarregado junto com o total */
    const dirtyWs = new Set<string>();
    const st = () => useStore.getState();
    const main = !window.location.hash.startsWith("#/");
    if (main && takeRecovery()) st().toast("A interface foi recarregada; os terminais continuaram rodando", "info");
    // limites das contas: cada janela guarda a leitura (topo, cabeçalho do painel)
    st().refreshLimits().catch(() => {});
    const subs = [
      on<RPayload>("limits", (p) => st().patch({ limits: p })),
      on<StatusEvent>("pane-status", (e) => {
        st().patchPane(e.id, { status: e.status });
        // "Parar" pedido e o painel voltou ao prompt: o servidor saiu do chip (A1.7)
        if (e.status === "ocioso" && st().devServers[e.id]?.parando) st().patch({ devServers: remover(st().devServers, e.id) });
        // every window gets the event; only the main one reports it, so an error does not toast twice
        if (e.reason && getCurrentWebviewWindow().label === "main") st().toast(`${st().panes[e.id]?.name ?? "Painel"}: ${e.reason}`, "erro");
      }),
      // medidor de contexto (Trilha A): só a última medida de cada painel
      on<PaneContextEvent>("pane-context", (e) => {
        if (e.ctxPct == null) return;
        const cur = st().paneCtx;
        st().patch({ paneCtx: { ...cur, [e.paneId]: { pct: e.ctxPct, tokens: e.ctxTokens, janela: e.windowSize } } });
      }),
      // proposta de compactação (chip do medidor): acao nula = a proposta saiu
      on<CompactPropostaEvent>("compact-proposta", (e) => {
        const cur = { ...st().compactPropostas };
        if (e.acao) cur[e.paneId] = { acao: e.acao, motivo: e.motivo };
        else delete cur[e.paneId];
        st().patch({ compactPropostas: cur });
      }),
      on<UsageEvent>("pane-usage", (e) => {
        st().patchPane(e.id, { tokIn: e.tokIn, tokOut: e.tokOut, tokCacheR: e.tokCacheR, tokCacheW: e.tokCacheW, costUsd: e.costUsd, sessionId: e.sessionId });
        const pane = st().panes[e.id];
        if (pane?.missionId) dirtyWs.add(pane.workspaceId);
        // throttle: com uso contínuo os totais ainda atualizam a cada 2 s
        if (timer === undefined) {
          timer = window.setTimeout(() => {
            timer = undefined;
            st().refreshTotals().catch(() => {});
            for (const ws of dirtyWs) st().refreshMissionTotals(ws).catch(() => {});
            dirtyWs.clear();
          }, 2000);
        }
      }),
      // painel sem missão (ex.: agendamento) com filtro de missão ativo: some da grade, então o filtro sai
      on<PaneRow>("pane-added", (p) => {
        st().upsertPane(p);
        if (!p.missionId && st().missionSel[p.workspaceId]) st().selectMission(p.workspaceId, null);
      }),
      on<{ id: string }>("pane-closed", (e) => st().removePane(e.id)),
      // grupo criado pelo app (subagentes entram no grupo do pai): o painel passa a apontar para ele
      on<{ workspaceId: string; group: PaneGroup; paneId: string; groupId: string }>("pane-group", (e) => {
        st().groupSync(e.workspaceId, e.group);
        st().patchPane(e.paneId, { groupId: e.groupId });
      }),
      on<MissionAttention>("mission-state", (a) => st().setAttention(a)),
      // recibo do perfil MCP: só nomes (mantidos e excluídos), uma vez por painel
      on<{ profile: string; kept: string[]; excluded: string[]; expostas: number }>("mcp-receipt", (e) => {
        if (main) st().toast(`MCP (${e.profile}): mantidos ${e.kept.join(", ")}; excluídos ${e.excluded.length ? e.excluded.join(", ") : "nenhum"}; ${e.expostas} ferramentas expostas`, "info");
      }),
      // servidor MCP removido do Claude desde a última abertura: o painel voltou só com os que restam
      on<{ paneId: string; missing: string[] }>("mcp-missing", (e) => {
        if (main) st().toast(`MCP: ${e.missing.join(", ")} não está mais no Claude; o painel seguiu com os demais`, "info");
      }),
      // o CLI recusou o modelo e o painel encerrou logo: oferece a troca rápida de modelo
      on<{ paneId: string; model: string | null; accountLabel: string }>("model-unavailable", (e) => {
        if (main) st().toast(`modelo ${e.model ?? "padrão"} indisponível para a conta ${e.accountLabel}`, "erro", { label: "Trocar modelo", run: () => st().patch({ modelPane: e.paneId }) });
      }),
      // o painel parou no limite da conta: oferece continuar na conta mais folgada (o aviso fica 2 min)
      on<{ paneId: string; paneName: string; account: string; text: string; others: string[] }>("account-limit", (e) => {
        if (!main) return;
        const limits = st().limits;
        const [melhor] = byHeadroom(e.others, (c) => (limits ? accountPeak(limits, "claude", c) : null));
        const msg = `${e.paneName}: limite da conta ${e.account} (${e.text})`;
        if (!melhor) return st().toast(`${msg}. Cadastre outra conta em Configurações → IAs para continuar nela.`, "erro");
        st().toast(msg, "erro", { label: `Continuar em ${melhor}`, run: () => void continueInAccount(e.paneId, melhor) }, 120_000);
      }),
      // aviso de workspace/painel remoto (o que o servidor não tem: MCP, hooks, claude instalado)
      on<{ text: string }>("remote-notice", (e) => st().toast(e.text, "info")),
      // execuções: promoção recusada pelo orçamento da missão (o painel do Claude segue)
      ...(main ? [on<{ text: string; nivel?: "info" | "erro" }>("exec-aviso", (e) => st().toast(e.text, e.nivel ?? "erro"))] : []),
      on<{ id: string; name: string }>("pane-renamed", (e) => st().patchPane(e.id, { name: e.name })),
      on<GateOpen>("gate-open", (g) => st().addGate(g)),
      on<{ id: string }>("gate-closed", (e) => st().removeGate(e.id)),
      on<{ missionId: string; handoff: { summary: string } }>("handoff", (e) => st().onHandoff(e.missionId, e.handoff.summary, main)),
      on<SlotEvent>("spawn-slot", (e) => st().setSlot(e)),
      // só a janela principal abre caminhos (segunda instância / linha de comando)
      ...(main ? [on<string>("open-path", (p) => st().openPath(p))] : []),
      // Shot: a captura salva vira aviso; falha de captura também aparece (só na janela principal)
      ...(main ? [on<{ name: string; pane?: string }>("shot-saved", (e) => st().toast(e.pane ? `${e.pane} capturou a tela: ${e.name}` : `Captura salva: ${e.name}`, "ok")), on<{ message: string }>("shot-error", (e) => st().toast(e.message, "erro")), on<{ path: string }>("shot-recorded", (e) => openTrim(e.path))] : []),
      // corte/pasta do Shot enviado pela janela do corte: cola no painel ativo desta janela
      ...(main ? [on<{ path: string }>("shot-paste", ({ path }) => pasteInto(useStore.getState().activePane, `${quoted(path)} `, "Clique num painel de terminal para escolher o destino"))] : []),
      // orçamento atingido (80% ou 100%): o toast fica na janela principal
      ...(main ? [on<{ level: number; text: string }>("budget-alert", (e) => st().toast(e.text, e.level >= 100 ? "erro" : "info", { label: "Ajustar orçamento", run: () => st().patch({ settingsOpen: true, settingsSection: "uso" }) }))] : []),
      // aviso do Windows que chegou com o Regente fora de foco: o próximo foco oferece ir ao painel
      // watcher de arquivos (fswatch.rs): a árvore relê as pastas que mudaram e o rodapé relê a branch
      on<{ root: string; dirs: string[] }>("fs-changed", (d) => window.dispatchEvent(new CustomEvent("regente:fs-changed", { detail: d }))),
      on<{ root: string }>("git-head", (d) => window.dispatchEvent(new CustomEvent("regente:git-head", { detail: d.root }))),
      // clique no aviso do Windows: o Rust já trouxe a janela; aqui vai ao painel (missão, grupo e foco certos)
      ...(main ? [on<{ paneId: string }>("notify-open", ({ paneId }) => goToPane(paneId))] : []),
      // avisos que chegaram com a janela fora de foco e não foram clicados: o mais novo e o anterior têm botão
      ...(main ? [on<{ paneId: string; paneIds?: string[] }>("notify-pending", ({ paneId, paneIds }) => {
        const t = pendingNoticeToast(paneIds?.length ? paneIds : [paneId], (id) => st().panes[id]?.name);
        if (t) st().toast(t.text, "info", { label: t.go.label, run: () => goToPane(t.go.id), ...(t.alt ? { alt: { label: t.alt.label, run: () => goToPane(t.alt!.id) } } : {}) }, 10_000);
      })] : []),
      // pedidos de confirmação (gate, escrita, missão) vindos do celular: só a janela principal mostra
      ...(main ? [on<JarvisConfirm>("jarvis-confirm", (c) => st().patch({ jarvisConfirms: [...st().jarvisConfirms.filter((x) => x.id !== c.id), c] }))] : []),
      // link de arquivo clicado num painel destacado: esta janela é a dona do workspace
      getCurrentWebviewWindow().listen<OpenFile & { wsId: string }>("open-file", async ({ payload: { wsId, ...file } }) => {
        if (!st().tabs.includes(wsId)) {
          const ws = wsOf(wsId);
          if (!ws || !(await st().openWorkspace(ws.path))) return;
        }
        st().patch({ active: wsId, openFile: { ...st().openFile, [wsId]: file }, view: { ...st().view, [wsId]: "arquivos" } });
        getCurrentWindow().setFocus().catch(() => {});
      }),
      getCurrentWindow().onCloseRequested(async (e) => {
        const s = st();
        const dirty = Object.values(s.dirty).some((d) => d.length);
        const busy = busyCount(Object.values(s.panes), s.gates);
        if (!dirty && busy === 0) return;
        e.preventDefault();
        if (dirty && !(await ask("Há alterações não salvas no editor. Fechar a janela e descartá-las?", { title: "Alterações não salvas", kind: "warning" }))) return;
        // fechar a janela mata os terminais: agentes no meio de uma tarefa pedem confirmação
        if (busy > 0 && !(await ask(`${busy === 1 ? "1 agente ainda está trabalhando" : `${busy} agentes ainda estão trabalhando`}. Fechar o Regente encerra os terminais; as sessões do Claude podem ser retomadas depois.`, { title: "Fechar o Regente?", kind: "warning", okLabel: "Fechar mesmo assim", cancelLabel: "Cancelar" }))) return;
        await getCurrentWindow().destroy();
      }),
      // ▼ trilha A
      // ajuste automático do perfil: aviso na janela principal, com Desfazer quando dá para voltar atrás
      ...(main ? [on<{ ajuste: { id: string; motivo: string; desfazivel: boolean; acao: string; depoisJson?: string | null } }>("ajuste-novo", ({ ajuste }) => {
        // proposta do piloto: aviso com Aplicar (a mesma ação do feed)
        if (ajuste.acao === "proposto" && ajuste.depoisJson) {
          st().toast(`Sugestão do piloto: ${ajuste.motivo}`, "info", {
            label: "Aplicar", run: () => { api.ajustesAplicar(ajuste.id).then(() => st().toast("Sugestão aplicada.", "ok"), (e) => st().toast(errMsg(e), "erro")); },
            alt: { label: "Ignorar", run: () => { api.ajustesIgnorar(ajuste.id).then(() => st().toast("Sugestão ignorada.", "ok"), (e) => st().toast(errMsg(e), "erro")); } },
          });
          return;
        }
        // só o que o Regente aplicou (ou o que dá para desfazer) vira aviso; pulos ficam no feed
        if (ajuste.acao !== "aplicado" && !ajuste.desfazivel) return;
        st().toast(`Ajuste automático: ${ajuste.motivo}`, "info", ajuste.desfazivel ? { label: "Desfazer", run: () => { api.ajustesUndo(ajuste.id).then(() => st().toast("Ajuste desfeito.", "ok"), (e) => st().toast(errMsg(e), "erro")); } } : undefined);
      })] : []),
      // ▲ trilha A
      // ▼ trilha B
      // ▲ trilha B
      // ▼ trilha C
      on<{ workspaceId: string; comandos: unknown }>("projeto-criado", () =>
        st().toast("Criei .regente/projeto.json com a configuração detectada. Revise em Configurações → Projeto; você pode commitá-lo para a equipe.", "info")),
      // ▲ trilha C
      // ▼ execução rápida (X1.2): o ligamento de node_modules e o tempo ganho na worktree nova
      on<{ workspaceId: string; missionId: string; texto: string }>("worktree-acelerado", ({ texto }) => st().toast(texto, "info")),
      // ▲ execução rápida
    ];
    const onRejection = (e: PromiseRejectionEvent) => st().toastErr(e.reason);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("unhandledrejection", onRejection);
      subs.forEach((p) => p.then((u) => u()));
    };
  }, []);
}
