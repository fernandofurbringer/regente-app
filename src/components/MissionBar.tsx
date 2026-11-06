import { ask } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { api, errCode, errMsg, on, type ChangedFile, type GitBranches, type GitEntry, type Mission, type StatusEvent, type UsageEvent, type WorktreeState } from "../lib/ipc";
import { branchOptions, GIT_CHANGED, gitChanged, gitChangedOf, syncLabel, type BranchOption } from "../lib/gitflow";
import { acceptedOf, DIRTY_NOTE, dirtyIn, unreviewed } from "../lib/review";
import { stateLine } from "../lib/worktreeState";
import { useStore } from "../state/store";
import ProfileChip from "./ProfileChip";
import { VerifyChip } from "./VerifyChip";
import { PhaseChip } from "./MissionPerformance";
import "./MissionBar.css";

export interface GitState { files: ChangedFile[]; entries: GitEntry[]; wt: WorktreeState | null; /** worktree adotado cuja pasta sumiu (apagada por fora) */ wtSumiu: boolean }

/**
 * Estado git da missão (arquivos contra a base e alterações não comitadas) ou do repositório do workspace (sem missão).
 * Relê quando a missão muda de status, a revisão, um diálogo git ou a revisão fecha, e (throttle 3 s) quando um painel
 * da missão muda de status ou de uso. Quem usa a missão recebe o mesmo estado por prop: uma só leitura por gatilho.
 */
export function useGitState(wsId: string, mission: Mission | null, hidden = false): GitState | null {
  const [st, setSt] = useState<GitState | null>(null);
  const [tick, setTick] = useState(0);
  const mid = mission?.id ?? null;
  const diff = !!mission?.worktreePath;
  const status = mission?.status;
  const reviewed = useStore((s) => (mid ? s.reviewed[mid] : undefined));
  const dialog = useStore((s) => s.gitDialog);
  const review = useStore((s) => s.reviewMission);
  // outra missão (ou workspace): não mostra o estado anterior enquanto lê o novo
  useEffect(() => setSt(null), [wsId, mid]);
  // trocou de branch, puxou ou sincronizou pelo rodapé, ou o HEAD mudou por fora (`gitChangedOf`): só este workspace
  useEffect(() => {
    const bump = (e: Event) => { if (gitChangedOf(e, wsId)) setTick((n) => n + 1); };
    window.addEventListener(GIT_CHANGED, bump);
    return () => window.removeEventListener(GIT_CHANGED, bump);
  }, [wsId]);
  // aba escondida não relê; ao voltar a aparecer e ao a janela ganhar foco (git usado no terminal), relê
  const estavaEscondida = useRef(hidden);
  useEffect(() => {
    if (hidden) {
      estavaEscondida.current = true;
      return;
    }
    // ao montar visível a leitura já acontece; só a volta de uma aba escondida pede outra
    if (estavaEscondida.current) setTick((n) => n + 1);
    estavaEscondida.current = false;
    const onFocus = () => setTick((n) => n + 1);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [hidden]);
  // o estado do worktree muda por fora (commits do agente, ambiente.sh): relê a cada minuto
  useEffect(() => {
    if (!diff || hidden) return;
    const t = window.setInterval(() => setTick((n) => n + 1), 60_000);
    return () => window.clearInterval(t);
  }, [diff, hidden]);
  useEffect(() => {
    if (!mid) return;
    let timer: number | undefined;
    let last = 0;
    const bump = (paneId: string) => {
      if (useStore.getState().panes[paneId]?.missionId !== mid || timer !== undefined) return;
      timer = window.setTimeout(() => { timer = undefined; last = Date.now(); setTick((n) => n + 1); }, Math.max(0, last + 3000 - Date.now()));
    };
    const subs = Promise.all([on<StatusEvent>("pane-status", (e) => bump(e.id)), on<UsageEvent>("pane-usage", (e) => bump(e.id))]);
    return () => {
      window.clearTimeout(timer);
      subs.then((us) => us.forEach((u) => u()));
    };
  }, [mid]);
  useEffect(() => {
    let live = true;
    const gmid = diff ? mid : null;
    Promise.all([
      // adotada com a pasta apagada: diff e status falham; a barra ainda mostra o aviso (wtSumiu) em vez de sumir
      diff && mid ? (mission?.adopted ? api.missionDiffFiles(mid).catch(() => [] as ChangedFile[]) : api.missionDiffFiles(mid)) : Promise.resolve([] as ChangedFile[]),
      mission?.adopted ? api.gitStatus(wsId, gmid).catch(() => [] as GitEntry[]) : api.gitStatus(wsId, gmid),
      mission?.adopted && mid
        ? api.missionWorktreeState(mid).then((wt) => ({ wt, wtSumiu: false })).catch((e) => ({ wt: null, wtSumiu: errCode(e) === "no_worktree" }))
        : Promise.resolve({ wt: null, wtSumiu: false }),
    ])
      .then(([files, entries, estado]) => {
        if (!live) return;
        setSt({ files, entries, ...estado });
        // a branch do worktree mudou por fora (ambiente.sh): o back já gravou; o rail e o rodapé leem do store
        if (mission?.adopted && estado.wt && estado.wt.branch !== mission.branch) useStore.getState().loadMissions(wsId).catch(() => {});
      })
      .catch(() => { if (live) setSt(null); });
    return () => { live = false; };
  }, [wsId, mid, diff, status, reviewed, dialog, review, tick, mission?.branch]);
  return st;
}

/** Confirma a integração: avisa arquivos não revisados e editor sujo (o merge recusa worktree sujo). */
export async function confirmIntegrate(m: Mission, wsId: string): Promise<boolean> {
  const reviewed = useStore.getState().reviewed[m.id] ?? acceptedOf(m.id);
  const pending = await api.missionDiffFiles(m.id).then((fs) => unreviewed(fs, reviewed).length).catch(() => 0);
  const notes = [
    pending ? `${pending} ${pending > 1 ? "arquivos ainda não revisados" : "arquivo ainda não revisado"} (Revisar mudanças).` : "",
    dirtyIn(useStore.getState().dirty[wsId] ?? [], m.worktreePath) ? DIRTY_NOTE : "",
  ].filter(Boolean).join(" ");
  if (!notes) return true;
  return ask(`${notes} Integrar "${m.name}" mesmo assim?`, { title: "Integrar missão", kind: "warning" });
}

/** Seletor de branch do rodapé: filtra as locais e as do remoto (busca o remoto ao abrir); um nome novo cria a branch
 *  a partir da atual. Setas escolhem, Enter troca, Esc fecha. */
function BranchPicker({ wsId, onPick, onClose }: { wsId: string; onPick(o: BranchOption): void; onClose(): void }) {
  const [b, setB] = useState<GitBranches | null>(null);
  const [fetching, setFetching] = useState(true);
  const [q, setQ] = useState("");
  const [cur, setCur] = useState(0);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    let live = true;
    let fetched = false;
    // a lista local sai na hora; a da busca no remoto chega depois e vale mais
    api.gitBranches(wsId, false).then((x) => { if (live && !fetched) setB(x); }).catch(() => {});
    api.gitBranches(wsId, true)
      .then((x) => { fetched = true; if (live) setB(x); })
      .catch(() => {})
      .finally(() => { if (live) setFetching(false); });
    const onDoc = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) onClose(); };
    document.addEventListener("mousedown", onDoc);
    return () => { live = false; document.removeEventListener("mousedown", onDoc); };
  }, [wsId, onClose]);
  const opts = b ? branchOptions(b, q) : [];
  const key = (e: ReactKeyboardEvent) => {
    if (e.key === "Escape") return onClose();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setCur((c) => Math.max(0, Math.min(opts.length - 1, c + (e.key === "ArrowDown" ? 1 : -1))));
    }
    if (e.key === "Enter" && opts[cur]) onPick(opts[cur]);
  };
  return (
    <div ref={ref} className="menu" role="dialog" aria-label="Trocar branch" style={{ right: 0, bottom: 34, width: 340 }}>
      <input autoFocus aria-label="Filtrar ou criar branch" placeholder="Filtrar, ou nome de uma branch nova" value={q} style={{ width: "100%", boxSizing: "border-box" }}
        onChange={(e) => { setQ(e.target.value); setCur(0); }} onKeyDown={key} />
      <div className="menu-info" role="status">{fetching ? "Buscando no remoto…" : b?.fetchError ? `Sem acesso ao remoto (lista da última busca): ${b.fetchError}` : ""}</div>
      <div role="listbox" aria-label="Branches" style={{ maxHeight: 300, overflowY: "auto" }}>
        {opts.map((o, i) => (
          <button key={`${o.kind}:${o.name}`} className="menu-item" role="option" aria-selected={i === cur} style={i === cur ? { background: "var(--bg4)" } : undefined}
            onMouseEnter={() => setCur(i)} onClick={() => onPick(o)}>
            {o.kind === "nova" ? `Criar branch “${o.name}” a partir de ${b?.current || "HEAD"}` : o.name}
            {o.name === b?.current && " ✓"}
            {o.kind === "remota" && <span className="menu-info"> remota</span>}
          </button>
        ))}
        {b && opts.length === 0 && <div className="menu-info">Nenhuma branch com esse nome.</div>}
      </div>
    </div>
  );
}

/**
 * Branch atual lida do arquivo HEAD (comando `git_head`, sem processo git): ao montar, ao ganhar foco e a cada 2 s com a
 * janela em foco e a aba visível. Mudou por fora (checkout no terminal, agente): avisa o `useGitState` para reler tudo.
 * `set` grava na hora o nome que a troca pelo app devolveu.
 */
function useHead(wsId: string, missionId: string | null, hidden: boolean): { value: string | null; set(b: string): void } {
  const [value, setValue] = useState<string | null>(null);
  const last = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    last.current = undefined;
    setValue(null);
  }, [wsId, missionId]);
  useEffect(() => {
    if (hidden) return;
    let live = true;
    const ler = () => api.gitHead(wsId, missionId).then((h) => {
      if (!live) return;
      setValue(h);
      if (last.current !== undefined && h !== last.current) window.dispatchEvent(gitChanged(wsId));
      last.current = h;
    }).catch(() => {});
    void ler();
    const t = window.setInterval(() => { if (document.hasFocus()) void ler(); }, 2000);
    // o watcher (fswatch.rs) avisa quando o HEAD de uma raiz vigiada muda: relê na hora, sem esperar os 2 s
    window.addEventListener("focus", ler);
    window.addEventListener("regente:git-head", ler);
    return () => { live = false; window.clearInterval(t); window.removeEventListener("focus", ler); window.removeEventListener("regente:git-head", ler); };
  }, [wsId, missionId, hidden]);
  return { value, set: (b: string) => { last.current = b; setValue(b); } };
}

/** O que era da barra da missão e ficou no rodapé: estado do worktree adotado e os chips (perfil, verificação, fase). */
function MissionFooterInfo({ wsId, mission, git }: { wsId: string; mission: Mission; git: GitState | null }) {
  const reviewed = useStore((s) => s.reviewed[mission.id]) ?? acceptedOf(mission.id);
  const pending = git ? unreviewed(git.files, reviewed).length : 0;
  return (
    <div className="gitfoot-mission">
      {mission.adopted && git?.wt && <span className="gitfoot-state" title={`${stateLine(git.wt, Date.now())} · HEAD ${git.wt.head}`}>{stateLine(git.wt, Date.now())}</span>}
      {mission.adopted && git?.wtSumiu && <span className="gitfoot-state" style={{ color: "var(--err)" }} title="A pasta do worktree foi apagada por fora; o git ainda a lista (git worktree prune)">worktree não existe mais</span>}
      {pending > 0 && (
        <button className="btn" title="Revisar as mudanças da missão" onClick={() => useStore.getState().patch({ reviewMission: mission.id })}>
          {pending} {pending === 1 ? "arquivo" : "arquivos"} para revisar
        </button>
      )}
      <ProfileChip workspaceId={wsId} missionId={mission.id} />
      <VerifyChip workspaceId={wsId} missionId={mission.id} />
      <PhaseChip missionId={mission.id} />
    </div>
  );
}

/** Rodapé da grade. Sem missão (repositório do workspace): `⎇ main` troca de branch, `3 alterados` abre Commitar,
 *  `Sincronizar ↓2 ↑1` recebe e envia, e `Git ▾` tem Puxar, Commitar e Abrir PR. Com missão: os chips da missão,
 *  Commitar, Abrir PR e Revisar mudanças (integrar fica no menu da aba da missão). */
export function GitFooter({ wsId, mission, git, hidden = false }: { wsId: string; mission: Mission | null; git: GitState | null; hidden?: boolean }) {
  const [menu, setMenu] = useState(false);
  const head = useHead(wsId, mission?.id ?? null, hidden);
  const [picker, setPicker] = useState(false);
  /** ação git em curso (rótulo do botão); uma por vez */
  const [busy, setBusy] = useState<string | null>(null);
  // sem missão, as branches do repositório (null fora de git ou em remoto: o rótulo fica "repositório")
  const [br, setBr] = useState<GitBranches | null>(null);
  const hasMission = !!mission;
  /** a pasta do workspace é repositório Git? null enquanto verifica (e em erro: mostra os botões de sempre) */
  const [repo, setRepo] = useState<boolean | null>(null);
  useEffect(() => {
    if (hasMission) return;
    let live = true;
    // um processo git só (branch, upstream, ↑↓); a lista de branches é do seletor
    api.gitSyncState(wsId).then((b) => { if (live) { setBr(b); setRepo(true); } }).catch((e) => {
      if (!live) return;
      setBr(null);
      setRepo(errCode(e) === "not_a_repo" ? false : null);
    });
    return () => { live = false; };
  }, [wsId, hasMission, git]);
  const closePicker = useCallback(() => setPicker(false), []);
  const act = (label: string, run: () => Promise<string>) => {
    setMenu(false);
    setPicker(false);
    setBusy(label);
    const { toast } = useStore.getState();
    run()
      .then((msg) => { toast(msg, "ok"); window.dispatchEvent(gitChanged(wsId)); })
      .catch((e) => toast(errMsg(e), "erro"))
      .finally(() => setBusy(null));
  };
  const pickBranch = async (o: BranchOption) => {
    if (o.kind !== "nova" && o.name === br?.current) return setPicker(false);
    // agente trabalhando na pasta do workspace: os arquivos mudariam embaixo dele
    const working = Object.values(useStore.getState().panes).some((p) => p.workspaceId === wsId && !p.missionId && p.kind === "agent" && p.status === "trabalhando");
    if (working && !(await ask("Há um agente trabalhando nesta pasta. Trocar a branch muda os arquivos embaixo dele. Trocar mesmo assim?", { title: "Trocar branch", kind: "warning" }))) return;
    act("Trocando…", () => api.gitSwitch(wsId, o.name, o.kind === "nova").then((b) => { head.set(b); return `Agora na branch ${b}`; }));
  };
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const onDoc = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setMenu(false); };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [menu]);
  const openGit = (kind: "commit" | "pr") => {
    setMenu(false);
    useStore.getState().patch({ gitDialog: { kind, wsId, missionId: mission?.worktreePath ? mission.id : null } });
  };
  const n = git?.entries.length ?? 0;
  // fora de repositório os botões de commit/PR só davam erro: no lugar deles, "Iniciar Git"
  if (!hasMission && repo === false) {
    const iniciar = async () => {
      if (!(await ask("Criar um repositório Git nesta pasta (git init)? Com Git, o Regente mostra as alterações, faz checkpoints e usa worktrees nas missões.", { title: "Iniciar Git", kind: "info" }))) return;
      api.gitInit(wsId)
        .then(() => { setRepo(true); useStore.getState().toast("Repositório Git criado", "ok"); })
        .catch((e) => useStore.getState().toast(errMsg(e), "erro"));
    };
    return (
      <div className="gitfoot">
        <button className="btn" title="Esta pasta não é um repositório Git" onClick={iniciar}>Sem Git · Iniciar Git</button>
      </div>
    );
  }
  const changes = n === 0 ? "limpo" : `${n} alterados`;
  // branch e sincronizar só no repositório do workspace: a branch da missão é do Regente
  const own = hasMission ? null : br;
  const sync = () => act("Sincronizando…", () => api.gitSync(wsId));
  return (
    <div className="gitfoot">
      {mission?.worktreePath && <MissionFooterInfo wsId={wsId} mission={mission} git={git} />}
      {own && (
        <div style={{ position: "relative" }}>
          <button className="btn" title="Trocar ou criar branch" aria-haspopup="dialog" aria-expanded={picker} disabled={!!busy} onClick={() => setPicker(!picker)}>
            ⎇ {(head.value ?? own.current) || "HEAD destacado"} ▾
          </button>
          {picker && <BranchPicker wsId={wsId} onPick={(o) => void pickBranch(o)} onClose={closePicker} />}
        </div>
      )}
      <button className="btn" title="Commitar" onClick={() => openGit("commit")}>
        {own ? changes : `${(head.value || mission?.branch) ?? (mission?.adopted ? "livre" : "repositório")} · ${changes}`}
      </button>
      {own && (
        <button className="btn" disabled={!!busy || !own.current} onClick={sync}
          title={own.upstream ? `Recebe de ${own.upstream} (rebase das suas mudanças por cima) e envia os seus commits` : "Publica a branch em origin"}>
          {busy ?? syncLabel(own)}
        </button>
      )}
      <div ref={ref} style={{ position: "relative" }}>
        <button className="btn" aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>Git ▾</button>
        {menu && (
          <div className="menu" role="menu" style={{ right: 0, bottom: 34 }}>
            {own && (
              <>
                <button className="menu-item" role="menuitem" disabled={!!busy} onClick={() => { setMenu(false); setPicker(true); }}>Trocar branch…</button>
                <button className="menu-item" role="menuitem" disabled={!!busy || !own.upstream} title="Só avanço rápido: não cria merge"
                  onClick={() => act("Puxando…", () => api.gitPull(wsId))}>Puxar (pull)</button>
                <button className="menu-item" role="menuitem" disabled={!!busy || !own.current} onClick={sync}>{own.upstream ? "Sincronizar (pull + push)" : "Publicar branch"}</button>
                <div className="menu-sep" />
              </>
            )}
            {mission?.worktreePath && (
              <button className="menu-item" role="menuitem" disabled={!git?.files.length}
                onClick={() => { setMenu(false); useStore.getState().patch({ reviewMission: mission.id }); }}>Revisar mudanças…</button>
            )}
            <button className="menu-item" role="menuitem" onClick={() => openGit("commit")}>Commitar…</button>
            <button className="menu-item" role="menuitem" onClick={() => openGit("pr")}>Abrir PR…</button>
          </div>
        )}
      </div>
    </div>
  );
}
