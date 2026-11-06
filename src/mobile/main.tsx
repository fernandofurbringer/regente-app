import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { createRoot } from "react-dom/client";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { ApiError, call, checkApproval, isPairCode, pairTokenFromHash, saveToken, storedToken, wsUrl } from "./api";
import { openNotices, showNotice } from "./notices";
import { missionsInUse } from "./inUse";
import "./mobile.css";

// Tipos do que o servidor devolve (ver src-tauri/src/mobile.rs). Todo texto vindo do servidor é exibido como texto.
interface Workspace { id: string; name: string; path: string }
interface Mission { id: string; name: string; mode: string; status: string }
interface Pane { id: string; name: string; kind: string; status: string; permission: string; cli: string | null; costUsd: number; missionId: string | null }
interface Gate { id: string; paneId: string; title: string; summary: string; options: string[] }
interface Resumable { id: string; name: string; cli: string | null; closedAt: number | null }
/** Conversa de um painel Claude: falas em ordem; ferramenta só com nome e estado (o servidor não manda a saída). */
type ConvItem = { kind: "text"; role: "voce" | "claude"; text: string } | { kind: "tool"; name: string; status: "ok" | "erro" | "pendente" };

type Api = <T>(method: "GET" | "POST", path: string, body?: unknown) => Promise<T>;
type View = { t: "workspaces" } | { t: "workspace"; ws: Workspace } | { t: "pane"; ws: Workspace; pane: Pane } | { t: "gates" };

const PENDING = "aguardando confirmação no computador";

function useApi(token: string, onAuthLost: () => void, onPending: () => void): Api {
  return useCallback<Api>(
    async (method, path, body) => {
      try {
        return await call(method, path, body, token);
      } catch (e) {
        if (e instanceof ApiError && e.status === 401) onAuthLost();
        if (e instanceof ApiError && e.status === 403 && e.code === "pending_approval") onPending();
        throw e;
      }
    },
    [token, onAuthLost, onPending],
  ) as Api;
}

/** Tela de aguardando aprovação: consulta a cada 3 s até aprovar (200) ou recusar (401). */
function Waiting({ token, onApproved, onRefused }: { token: string; onApproved: () => void; onRefused: () => void }) {
  const [error, setError] = useState(false);
  useEffect(() => {
    let alive = true;
    const tick = async () => {
      const s = await checkApproval(token);
      if (!alive) return;
      if (s === "approved") onApproved();
      else if (s === "refused") onRefused();
      else setError(s === "error");
    };
    void tick();
    const id = window.setInterval(tick, 3000);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [token, onApproved, onRefused]);
  return (
    <main className="screen">
      <h1>Aguardando aprovação no computador…</h1>
      <p className="muted">Confirme o pedido de pareamento no Regente do computador. Esta tela atualiza sozinha.</p>
      {error && <p className="alert" role="alert">Sem conexão com o computador. Tentando de novo…</p>}
    </main>
  );
}

/** Lista que se atualiza sozinha enquanto a tela está aberta. */
function usePoll<T>(api: Api, path: string | null, ms = 5000): { data: T | null; error: string | null; reload: () => void } {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!path) return;
    let alive = true;
    const run = () =>
      api<T>("GET", path)
        .then((d) => {
          if (alive) {
            setData(d);
            setError(null);
          }
        })
        .catch((e) => {
          if (alive) setError(String(e.message ?? e));
        });
    run();
    const id = window.setInterval(run, ms);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, path, ms, tick]);
  return { data, error, reload: () => setTick((n) => n + 1) };
}

function Pairing({ onPaired }: { onPaired: (token: string) => void }) {
  const [code, setCode] = useState("");
  const [name, setName] = useState("Celular");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function pair(body: Record<string, string>) {
    setBusy(true);
    setError(null);
    try {
      const r = await call<{ deviceToken: string }>("POST", "/api/pair", { ...body, deviceName: name || "Celular" });
      saveToken(r.deviceToken);
      onPaired(r.deviceToken);
    } catch (e) {
      setError(e instanceof ApiError && e.status === 429 ? "Muitas tentativas. Espere 1 minuto." : "Código ou link inválido. Gere um novo no computador.");
    } finally {
      setBusy(false);
    }
  }

  // o link do QR traz o token na fragmentação (não vai ao servidor nem ao histórico, que é limpo logo depois)
  useEffect(() => {
    const token = pairTokenFromHash(window.location.hash);
    window.history.replaceState(null, "", window.location.pathname);
    if (token) void pair({ token });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function submit(e: FormEvent) {
    e.preventDefault();
    if (isPairCode(code)) void pair({ code: code.trim() });
    else setError("Digite o código de 6 dígitos mostrado no computador.");
  }

  return (
    <main className="screen">
      <h1>Parear com o Regente</h1>
      <p className="muted">No computador: Configurações → Celular. Escaneie o QR ou digite o código de 6 dígitos.</p>
      <form onSubmit={submit} className="stack">
        <label>
          Código
          <input inputMode="numeric" autoComplete="one-time-code" maxLength={6} value={code} onChange={(e) => setCode(e.target.value)} />
        </label>
        <label>
          Nome deste celular
          <input value={name} maxLength={40} onChange={(e) => setName(e.target.value)} />
        </label>
        <button className="btn primary" disabled={busy}>{busy ? "Pareando…" : "Parear"}</button>
      </form>
      {error && <p className="alert" role="alert">{error}</p>}
    </main>
  );
}

function Workspaces({ api, go }: { api: Api; go: (v: View) => void }) {
  const { data, error } = usePoll<Workspace[]>(api, "/api/workspaces");
  return (
    <section className="screen">
      <h2>Workspaces</h2>
      {error && <p className="alert" role="alert">{error}</p>}
      {data?.length === 0 && <p className="muted">Nenhum workspace aberto no computador.</p>}
      <ul className="list">
        {data?.map((ws) => (
          <li key={ws.id}>
            <button className="row-btn" onClick={() => go({ t: "workspace", ws })}>
              <span className="grow">{ws.name}</span>
              <span className="muted small">{ws.path}</span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function WorkspaceScreen({ api, ws, go }: { api: Api; ws: Workspace; go: (v: View) => void }) {
  const missions = usePoll<Mission[]>(api, `/api/workspaces/${ws.id}/missions`);
  const panes = usePoll<Pane[]>(api, `/api/workspaces/${ws.id}/panes`);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showMission, setShowMission] = useState(false);
  const [mName, setMName] = useState("");
  const [mBrief, setMBrief] = useState("");
  // por padrão o celular mostra só as missões com painel vivo
  const [onlyInUse, setOnlyInUse] = useState(true);
  const resumable = usePoll<Resumable[]>(api, `/api/workspaces/${ws.id}/resumable`, 8000);
  const shownMissions = onlyInUse ? missionsInUse(missions.data ?? [], panes.data ?? []) : (missions.data ?? []);

  async function run(fn: () => Promise<void>) {
    setError(null);
    setNotice(null);
    try {
      await fn();
    } catch (e) {
      setError(e instanceof ApiError && e.status === 429 ? "Limite atingido. Tente em instantes." : String((e as Error).message ?? e));
    }
  }

  async function newTerminal() {
    await run(async () => {
      const r = await api<{ id: string }>("POST", "/api/panes", { workspaceId: ws.id, kind: "terminal" });
      panes.reload();
      go({ t: "pane", ws, pane: { id: r.id, name: "Terminal", kind: "terminal", status: "iniciando", permission: "pend", cli: null, costUsd: 0, missionId: null } });
    });
  }

  async function proposeMission(e: FormEvent) {
    e.preventDefault();
    await run(async () => {
      const r = await api<{ status: string }>("POST", "/api/missions", { workspaceId: ws.id, name: mName, mode: "livre", brief: mBrief });
      setNotice(r.status === PENDING ? "Missão enviada: confirme no computador." : "Missão criada.");
      setShowMission(false);
      setMName("");
      setMBrief("");
      missions.reload();
    });
  }

  return (
    <section className="screen">
      <h2>{ws.name}</h2>
      <div className="actions">
        <button className="btn" onClick={newTerminal}>Novo terminal</button>
        <button className="btn" onClick={() => setShowMission((v) => !v)}>Nova missão</button>
      </div>
      {showMission && (
        <form className="stack card" onSubmit={proposeMission}>
          <label>Nome <input value={mName} maxLength={80} onChange={(e) => setMName(e.target.value)} /></label>
          <label>O que fazer <textarea rows={4} value={mBrief} maxLength={4000} onChange={(e) => setMBrief(e.target.value)} required /></label>
          <button className="btn primary" disabled={!mBrief.trim()}>Propor missão</button>
        </form>
      )}
      {notice && <p className="notice" role="status">{notice}</p>}
      {error && <p className="alert" role="alert">{error}</p>}
      <h3>Missões</h3>
      <label className="row small">
        <input type="checkbox" checked={onlyInUse} onChange={(e) => setOnlyInUse(e.target.checked)} /> Só missões em uso
      </label>
      {missions.data && shownMissions.length === 0 && <p className="muted">{onlyInUse ? "Nenhuma missão em uso." : "Nenhuma missão."}</p>}
      <ul className="list">
        {shownMissions.map((m) => (
          <li key={m.id} className="card">
            <div className="row-between">
              <span className="grow">{m.name}</span>
              <span className="chip">{m.status}</span>
            </div>
            <button className="btn small" onClick={() => run(() => api("POST", `/api/missions/${m.id}/seen`).then(() => undefined))}>
              Marcar entregas como vistas
            </button>
          </li>
        ))}
      </ul>
      <h3>Painéis</h3>
      {panes.error && <p className="alert" role="alert">{panes.error}</p>}
      <ul className="list">
        {panes.data?.map((p) => (
          <li key={p.id}>
            <button className="row-btn" onClick={() => go({ t: "pane", ws, pane: p })}>
              <span className="grow">{p.name}</span>
              <span className="chip">{p.status}</span>
              <span className="muted small">US$ {p.costUsd.toFixed(2)}</span>
            </button>
          </li>
        ))}
      </ul>
      {panes.data?.length === 0 && <p className="muted">Nenhum painel aberto.</p>}
      <h3>Conversas fechadas</h3>
      {resumable.error && <p className="alert" role="alert">{resumable.error}</p>}
      {resumable.data?.length === 0 && <p className="muted">Nenhuma conversa fechada para retomar.</p>}
      <ul className="list">
        {resumable.data?.map((r) => (
          <li key={r.id} className="row-between">
            <span className="grow">{r.name}</span>
            <span className="muted small">{r.cli ?? "IA"}</span>
            <button
              className="btn small"
              onClick={() =>
                void run(async () => {
                  const p = await api<{ id: string; name: string }>("POST", `/api/panes/${r.id}/resume`);
                  resumable.reload();
                  go({ t: "pane", ws, pane: { id: p.id, name: p.name, kind: "agent", status: "iniciando", permission: "pend", cli: r.cli, costUsd: 0, missionId: null } });
                })
              }
            >
              Retomar
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Conversa estruturada de um painel Claude, atualizada sozinha. Sem a saída crua das ferramentas. */
function Conversation({ api, paneId }: { api: Api; paneId: string }) {
  const { data, error } = usePoll<{ items: ConvItem[] }>(api, `/api/panes/${paneId}/conversation`, 4000);
  // acompanha o fim só quem já está lá: quem leu para cima não é puxado para baixo a cada atualização
  const endRef = useRef<HTMLDivElement | null>(null);
  const atEnd = useRef(true);
  useEffect(() => {
    const onScroll = () => { atEnd.current = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 40; };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);
  useEffect(() => {
    if (atEnd.current) endRef.current?.scrollIntoView({ block: "end" });
  }, [data]);
  if (error) return <p className="alert" role="alert">{error}</p>;
  if (!data) return <p className="muted small">Carregando conversa…</p>;
  if (data.items.length === 0) return <p className="muted small">Ainda sem conversa estruturada; use o terminal.</p>;
  return (
    <>
    <ol className="conversa" aria-label="Conversa">
      {data.items.map((it, i) =>
        it.kind === "text" ? (
          <li key={i} className={`fala ${it.role}`}>
            <strong className="small">{it.role === "voce" ? "Você" : "Claude"}</strong>
            <p>{it.text}</p>
          </li>
        ) : (
          <li key={i} className="ferramenta small muted">
            Ferramenta {it.name}: {it.status}
          </li>
        ),
      )}
    </ol>
    <div ref={endRef} />
    </>
  );
}

const KEYS: [string, string][] = [
  ["Enter", "enter"],
  ["Ctrl+C", "ctrl-c"],
  ["Esc", "esc"],
  ["Tab", "tab"],
  ["↑", "up"],
  ["↓", "down"],
  ["←", "left"],
  ["→", "right"],
];

function PaneScreen({ api, pane }: { api: Api; pane: Pane }) {
  const host = useRef<HTMLDivElement>(null);
  const [status, setStatus] = useState<string>("conectando");
  const [text, setText] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  // painel Claude abre na conversa estruturada; o terminal continua ao vivo por baixo
  const [view, setView] = useState<"conversa" | "terminal">(pane.cli === "claude" ? "conversa" : "terminal");
  const token = storedToken() ?? "";

  // saída do painel: replay + ao vivo, em bytes que o xterm desenha (nada vai pelo innerHTML)
  useEffect(() => {
    if (!host.current) return;
    const term = new Terminal({ convertEol: true, disableStdin: true, fontSize: 12, scrollback: 2000, cols: 80 });
    term.open(host.current);
    const ws = new WebSocket(wsUrl(`/ws/panes/${pane.id}`, window.location));
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth", token }));
      setStatus("ao vivo");
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        try {
          const m = JSON.parse(ev.data) as { type?: string; error?: string };
          if (m.type === "error") setNotice(m.error === "desktop_only" ? "Este painel só recebe texto com confirmação no computador." : `Erro: ${m.error}`);
        } catch {
          // mensagem que não é JSON: ignorada
        }
        return;
      }
      term.write(new Uint8Array(ev.data as ArrayBuffer));
    };
    ws.onclose = () => setStatus("desconectado");
    return () => {
      ws.close();
      term.dispose();
    };
  }, [pane.id, token, attempt]);

  async function send(body: { text: string; enter: boolean } | { key: string }) {
    setNotice(null);
    try {
      const r = await api<{ status?: string }>("POST", `/api/panes/${pane.id}/input`, body);
      if (r.status === PENDING) setNotice(PENDING);
      if ("text" in body) setText("");
    } catch (e) {
      const err = e as ApiError;
      setNotice(err.code === "desktop_only" ? "Teclas só para painel de IA comum; use o computador." : String(err.message));
    }
  }

  async function wake() {
    setNotice(null);
    try {
      await api("POST", `/api/panes/${pane.id}/wake`);
      setAttempt((n) => n + 1);
      setNotice("Acordando o painel…");
    } catch (e) {
      setNotice(String((e as Error).message));
    }
  }

  return (
    <section className="screen">
      <h2>{pane.name}</h2>
      <p className="muted small">{pane.kind === "terminal" ? "Terminal" : `IA${pane.cli ? ` · ${pane.cli}` : ""}`} · {status}</p>
      {pane.cli === "claude" && (
        <div className="actions" role="group" aria-label="Modo de exibição">
          <button className="btn small" aria-pressed={view === "conversa"} onClick={() => setView("conversa")}>Conversa</button>
          <button className="btn small" aria-pressed={view === "terminal"} onClick={() => setView("terminal")}>Terminal</button>
        </div>
      )}
      {pane.cli === "claude" && view === "conversa" && <Conversation api={api} paneId={pane.id} />}
      <div className="term" ref={host} hidden={pane.cli === "claude" && view === "conversa"} />
      {(status === "desconectado" || pane.status === "hibernado" || notice?.startsWith("Erro")) && (
        <div className="actions">
          <button className="btn" onClick={wake}>Acordar / reconectar</button>
        </div>
      )}
      <form className="stack" onSubmit={(e) => { e.preventDefault(); if (text.trim()) void send({ text, enter: true }); }}>
        <label>
          Texto para o painel
          <input value={text} maxLength={4000} autoComplete="off" onChange={(e) => setText(e.target.value)} />
        </label>
        <button className="btn primary" disabled={!text.trim()}>Enviar</button>
      </form>
      <div className="keys" role="group" aria-label="Teclas">
        {KEYS.map(([label, key]) => (
          <button key={key} className="btn key" onClick={() => send({ key })}>{label}</button>
        ))}
      </div>
      {notice && <p className="notice" role="status">{notice}</p>}
    </section>
  );
}

function GatesScreen({ api }: { api: Api }) {
  const { data, error, reload } = usePoll<Gate[]>(api, "/api/gates", 4000);
  const [notes, setNotes] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<string | null>(null);

  async function decide(g: Gate, decision: string) {
    setNotice(null);
    try {
      await api("POST", `/api/gates/${g.id}`, { decision, note: notes[g.id] ?? "" });
      reload();
    } catch (e) {
      setNotice(e instanceof ApiError && e.code === "gate_closed" ? "Este gate já foi decidido." : String((e as Error).message));
      reload();
    }
  }

  return (
    <section className="screen">
      <h2>Pendências</h2>
      {error && <p className="alert" role="alert">{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}
      {data?.length === 0 && <p className="muted">Nenhum gate aguardando decisão.</p>}
      <ul className="list">
        {data?.map((g) => (
          <li key={g.id} className="card">
            <strong>{g.title}</strong>
            <p className="small">{g.summary}</p>
            <label>
              Ajuste (opcional)
              <input value={notes[g.id] ?? ""} maxLength={4000} onChange={(e) => setNotes({ ...notes, [g.id]: e.target.value })} />
            </label>
            <div className="actions">
              {g.options.map((o) => (
                <button key={o} className="btn" onClick={() => decide(g, o)}>{o}</button>
              ))}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Permissão dos avisos do navegador. Web Push (com o app fechado) não é suportado com certificado autoassinado. */
function NoticeToggle() {
  const [perm, setPerm] = useState<string>(typeof Notification === "undefined" ? "unsupported" : Notification.permission);
  if (perm === "unsupported") return <span className="meta">avisos indisponíveis neste navegador</span>;
  if (perm === "granted") return <span className="meta" title="avisos enquanto esta página está aberta">avisos ativos</span>;
  if (perm === "denied") return <span className="meta">avisos bloqueados no navegador</span>;
  return (
    <button className="btn small" title="avisos só enquanto esta página está aberta (sem Web Push neste certificado)" onClick={() => void Notification.requestPermission().then(setPerm)}>
      Ativar avisos
    </button>
  );
}

function App() {
  const [token, setToken] = useState<string | null>(storedToken());
  const [view, setView] = useState<View>({ t: "workspaces" });
  const [waiting, setWaiting] = useState(false);
  const lost = useCallback(() => {
    saveToken(null);
    setToken(null);
    setWaiting(false);
  }, []);
  const pending = useCallback(() => setWaiting(true), []);
  const approved = useCallback(() => setWaiting(false), []);
  const api = useApi(token ?? "", lost, pending);
  useEffect(() => (token ? openNotices(token, showNotice) : undefined), [token]);

  if (!token) return <Pairing onPaired={setToken} />;
  if (waiting) return <Waiting token={token} onApproved={approved} onRefused={lost} />;

  return (
    <div className="app">
      <header className="top">
        {view.t !== "workspaces" && (
          <button className="btn small" onClick={() => setView(view.t === "pane" ? { t: "workspace", ws: view.ws } : { t: "workspaces" })}>← Voltar</button>
        )}
        <strong className="grow">Regente</strong>
        <NoticeToggle />
        <button className="btn small" onClick={() => setView({ t: "gates" })}>Pendências</button>
      </header>
      {view.t === "workspaces" && <Workspaces api={api} go={setView} />}
      {view.t === "workspace" && <WorkspaceScreen api={api} ws={view.ws} go={setView} />}
      {view.t === "pane" && <PaneScreen api={api} pane={view.pane} />}
      {view.t === "gates" && <GatesScreen api={api} />}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
