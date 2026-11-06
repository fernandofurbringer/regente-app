import { useEffect, useState } from "react";
import { api, errMsg, type McpListing, type McpProfile, type MemoryMode, type PaneSpec, type Permission, type RecipeDef } from "../lib/ipc";
import { lastPickFor, rememberPick } from "../lib/lastAgent";
import { installSpec } from "../lib/install";
import { defaultCli } from "../lib/quickstart";
import { favKey, sortFavFirst } from "../lib/favorites";
import { isSingleStack, slotFromStack } from "../lib/stacks";
import InstallButton from "./InstallButton";
import { useStore, wsOf } from "../state/store";
import Modal from "./Modal";
import { focusNewPane } from "./TerminalPane";
import { useLastAccount } from "../lib/lastAccount";

/** Referência estável (um `[]` novo por leitura faz o React 19 laçar). */
const NO_FAVS: string[] = [];

export type SlotSpec = Omit<PaneSpec, "workspaceId" | "cwd">;
export const defaultSlot = (cli = "claude"): SlotSpec => ({ kind: "agent", cli, permission: "pend" });
export const PERMISSIONS: { id: Permission; label: string; hint: string }[] = [
  { id: "pend", label: "Padrão do CLI", hint: "usa o modo de permissão configurado no próprio CLI (perguntar, automático…)" },
  { id: "auto", label: "Auto", hint: "aceita edições automaticamente" },
  { id: "plan", label: "Plano", hint: "só planeja, não edita" },
  { id: "danger", label: "Perigo", hint: "pula todas as permissões" },
];

const MCP_PROFILES: { id: McpProfile; label: string; hint: string }[] = [
  { id: "full", label: "Completo", hint: "tudo que a IA já tem configurado" },
  { id: "minimal", label: "Mínimo", hint: "só o Regente; os outros servidores ficam de fora" },
  { id: "custom", label: "Escolher", hint: "o Regente e os servidores que você marcar" },
];

/** Perfil de servidores MCP do painel. Só o Claude filtra; nos outros CLIs mostra "não suportado nesta IA". */
function McpProfileField({ value, set, cwd }: { value: SlotSpec; set(p: Partial<SlotSpec>): void; cwd: string }) {
  const [listing, setListing] = useState<McpListing | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const cli = value.cli ?? "";
  useEffect(() => {
    if (!cli) return;
    let alive = true;
    setListing(null);
    setErr(null);
    api.mcpServersList(cli, cwd).then((l) => alive && setListing(l)).catch((e) => alive && setErr(errMsg(e)));
    return () => { alive = false; };
  }, [cli, cwd]);
  if (!cli) return null;
  if (err) return <div className="meta">MCP: {err}</div>;
  if (!listing) return null;
  if (!listing.supported) return <div className="meta">Servidores MCP: não suportado nesta IA</div>;
  const profile = value.mcpProfile ?? "full";
  const kept = value.mcpServers ?? [];
  return (
    <>
      <label className="field">
        <span>Servidores MCP</span>
        <select value={profile} onChange={(e) => set({ mcpProfile: e.target.value as McpProfile, mcpServers: e.target.value === "custom" ? kept : null })}>
          {MCP_PROFILES.map((p) => <option key={p.id} value={p.id} title={p.hint}>{p.label}</option>)}
        </select>
      </label>
      {profile === "custom" && (
        <div className="field">
          {listing.servers.length === 0 && <span className="meta">Nenhum servidor MCP configurado no Claude deste projeto.</span>}
          {listing.servers.map((s) => (
            <label key={s.name} className="row">
              <input type="checkbox" checked={kept.includes(s.name)}
                onChange={(e) => set({ mcpServers: e.target.checked ? [...kept, s.name] : kept.filter((n) => n !== s.name) })} />
              <span>{s.name}</span>
              <span className="meta">{s.source} · {s.transport}</span>
            </label>
          ))}
          <span className="meta">A config MCP do painel, com o env dos servidores escolhidos, é gravada na pasta do painel enquanto ele existir.</span>
        </div>
      )}
    </>
  );
}

/** pilotOnly: só IA (cli, modelo, esforço, conta), sem tipo, permissão nem rota — o piloto de uma missão agêntica.
 * cwd: pasta do projeto, para listar os servidores MCP do Claude (sem ele, o perfil não aparece). */
/** standalone: painel fora de missão; aí a memória é a do workspace (padrão) ou nenhuma, por painel. */
export function SlotFields({ value, onChange, pilotOnly = false, cwd = null, standalone = false }: {
  value: SlotSpec; onChange(v: SlotSpec): void; pilotOnly?: boolean; cwd?: string | null; standalone?: boolean;
}) {
  const providers = useStore((s) => s.providers);
  const detected = useStore((s) => s.detected);
  const settings = useStore((s) => s.settings);
  if (!providers) return null;
  const cli = providers.clis.find((c) => c.id === value.cli);
  const installed = (id: string) => detected.some((d) => d.kind === "cli" && d.id === id && d.path);
  const set = (p: Partial<SlotSpec>) => onChange({ ...value, ...p });
  const accounts = cli ? ["principal", ...(settings?.accounts[cli.id] ?? [])] : [];
  const models = value.route ? providers.routes.find((r) => r.id === value.route)?.models ?? [] : cli?.models ?? [];
  const listId = `models-${value.cli}-${value.route ?? "direto"}`;
  const danger = !pilotOnly && value.kind === "agent" && value.permission === "danger";
  return (
    <>
    <div className="row" style={{ alignItems: "flex-end" }}>
      {!pilotOnly && <label className="field">
        <span>Tipo</span>
        <select value={value.kind} onChange={(e) => set(e.target.value === "agent"
          ? { kind: "agent", cli: "claude" }
          : { kind: "terminal", cli: null, model: null, effort: null, route: null, account: null })}>
          <option value="agent">IA</option>
          <option value="terminal">Terminal</option>
        </select>
      </label>}
      {value.kind === "agent" ? (
        <>
          <label className="field">
            <span>IA</span>
            <select value={value.cli ?? ""} onChange={(e) => set({ cli: e.target.value, model: null, effort: null, route: null, account: null })}>
              {providers.clis.map((c) => <option key={c.id} value={c.id}>{c.label}{installed(c.id) ? "" : " (não instalado)"}</option>)}
            </select>
          </label>
          <label className="field">
            <span>Modelo</span>
            <input list={listId} value={value.model ?? ""} placeholder="padrão do CLI" onChange={(e) => set({ model: e.target.value || null })} />
            <datalist id={listId}>{models.map((m) => <option key={m} value={m} />)}</datalist>
          </label>
          {cli && cli.efforts.length > 0 && (
            <label className="field">
              <span>Esforço</span>
              <select value={value.effort ?? ""} onChange={(e) => set({ effort: e.target.value || null })}>
                <option value="">padrão</option>
                {cli.efforts.map((x) => <option key={x}>{x}</option>)}
              </select>
            </label>
          )}
          {!pilotOnly && value.kind === "agent" && cwd && <McpProfileField value={value} set={set} cwd={cwd} />}
          {!pilotOnly && value.kind === "agent" && standalone && (
            <label className="field">
              <span>Memória</span>
              <select value={value.memoryMode ?? "standalone"} onChange={(e) => set({ memoryMode: e.target.value as MemoryMode })}>
                <option value="standalone">Memória do workspace</option>
                <option value="off">Sem memória</option>
              </select>
            </label>
          )}
          {!pilotOnly && <label className="field">
            <span>Permissão</span>
            <select value={value.permission ?? "pend"} onChange={(e) => set({ permission: e.target.value as Permission })}>
              {PERMISSIONS.map((p) => <option key={p.id} value={p.id} title={p.hint}>{p.label}</option>)}
            </select>
          </label>}
          {accounts.length > 1 && (
            <label className="field">
              <span>Conta</span>
              <select value={value.account ?? "principal"} onChange={(e) => set({ account: e.target.value })}>
                {accounts.map((a) => <option key={a}>{a}</option>)}
              </select>
            </label>
          )}
          {cli?.routable && !pilotOnly && (
            <label className="field">
              <span>Rota</span>
              <select value={value.route ?? ""} onChange={(e) => set({ route: e.target.value || null })}>
                <option value="">direto (login do CLI)</option>
                {providers.routes.map((r) => <option key={r.id} value={r.id}>{r.label}</option>)}
              </select>
            </label>
          )}
        </>
      ) : (
        <label className="field">
          <span>Shell</span>
          <select value={value.shell ?? settings?.defaultShell ?? "powershell"} onChange={(e) => set({ shell: e.target.value })}>
            {providers.shells.map((s) => <option key={s.id} value={s.id}>{s.label}</option>)}
          </select>
        </label>
      )}
    </div>
    {danger && <p className="meta" role="note" style={{ color: "var(--err)" }}>Pula todas as permissões da CLI. Use em worktree ou pasta descartável.</p>}
    </>
  );
}

/** Com a IA escolhida, o modelo e o esforço da última vez que ela foi usada (P1.8). */
function withLastPick(s: SlotSpec): SlotSpec {
  const pick = s.kind === "agent" ? lastPickFor(s.cli ?? null) : null;
  return pick ? { ...s, model: pick.model, effort: pick.effort } : s;
}

export default function NewPaneDialog({ wsId }: { wsId: string }) {
  const [slot, setSlotState] = useState<SlotSpec>(() => withLastPick(defaultSlot(defaultCli(useStore.getState().detected) ?? "claude")));
  // trocar de IA traz o modelo e o esforço que ela usou por último
  // o perfil de MCP é de uma IA: trocar de IA volta ao completo
  const setSlot = (v: SlotSpec) => setSlotState(v.cli !== slot.cli ? withLastPick({ ...v, mcpProfile: null, mcpServers: null }) : v);
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  // stacks de um painel (Configurações → Catálogo → Stacks): escolher uma preenche IA, modelo, esforço, agente e skill
  const [stacks, setStacks] = useState<RecipeDef[]>([]);
  const [stackId, setStackId] = useState("");
  const favs = useStore((s) => s.settings?.favorites ?? NO_FAVS);
  const providers = useStore((s) => s.providers);
  const detected = useStore((s) => s.detected);
  const inMission = useStore((s) => !!s.missionSel[wsId]);
  const { patch, upsertPane, toast, toastErr } = useStore.getState();
  const cli = providers?.clis.find((c) => c.id === slot.cli);
  const missing = slot.kind === "agent" && !!cli && !detected.some((d) => d.id === cli.id && d.path);
  const close = () => patch({ dialog: null });
  const missionId = () => useStore.getState().missionSel[wsId] ?? null;

  useEffect(() => { api.providersDetect().then((d) => patch({ detected: d })).catch(() => {}); }, [patch]);
  // conta ainda não escolhida (null): a última usada nesta pasta com esta IA, em vez da principal sempre
  const ultima = useLastAccount(wsId, slot.kind === "agent" ? slot.cli : null);
  useEffect(() => {
    if (ultima !== undefined && slot.kind === "agent" && slot.account == null && ultima !== "principal") setSlotState((s) => (s.account == null ? { ...s, account: ultima } : s));
  }, [ultima, slot.kind, slot.account]);
  useEffect(() => { api.recipesList().then((l) => setStacks(l.filter(isSingleStack))).catch(() => {}); }, []);
  function applyStack(id: string) {
    setStackId(id);
    const s = stacks.find((r) => r.id === id);
    const next = s ? slotFromStack(s) : null;
    if (next) setSlotState(next);
  }

  async function submit() {
    const ws = wsOf(wsId);
    if (!ws || busy || missing) return;
    setBusy(true);
    try {
      const row = await api.paneSpawn({ ...slot, name: name.trim() || null, prompt: slot.kind === "agent" ? prompt.trim() || null : null, missionId: missionId(), workspaceId: ws.id, cwd: ws.path });
      upsertPane(row);
      focusNewPane(row.id);
      if (slot.kind === "agent" && slot.cli) rememberPick(slot.cli, { model: slot.model ?? null, effort: slot.effort ?? null });
      close();
    } catch (e) {
      toastErr(e);
    } finally {
      setBusy(false);
    }
  }
  async function install() {
    const ws = wsOf(wsId);
    if (!ws || !cli?.install) return;
    try {
      upsertPane(await api.paneSpawn({ ...installSpec(cli.label, cli.install), workspaceId: ws.id, missionId: missionId(), cwd: ws.path }));
      toast("Quando a instalação terminar, abra o painel de novo para detectar a IA.", "info");
      close();
    } catch (e) {
      toastErr(e);
    }
  }

  return (
    <Modal label="Novo painel" onClose={close}>
      <form onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <h2>Novo painel</h2>
        {missing && (
          <div className="card">
            {cli!.label} não foi encontrado neste computador.{" "}
            {cli!.install ? <InstallButton label={cli!.label} command={cli!.install} onConfirm={install} /> : "Instale manualmente e reabra este diálogo."}
          </div>
        )}
        {stacks.length > 0 && (
          <label className="field">
            <span>Stack (opcional)</span>
            <select value={stackId} onChange={(e) => applyStack(e.target.value)}>
              <option value="">sem stack: escolher à mão</option>
              {sortFavFirst(stacks, (r) => favKey("recipe", r.id), favs).map((r) => (
                <option key={r.id} value={r.id}>{favs.includes(favKey("recipe", r.id)) ? "★ " : ""}{r.name}</option>
              ))}
            </select>
          </label>
        )}
        <SlotFields value={slot} onChange={setSlot} cwd={wsOf(wsId)?.path ?? null} standalone={!inMission} />
        {slot.kind === "agent" && (
          <label className="field">
            <span>Pedido inicial (opcional)</span>
            <textarea rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          </label>
        )}
        <label className="field"><span>Nome (opcional)</span><input value={name} onChange={(e) => setName(e.target.value)} /></label>
        <div className="actions">
          <button type="button" className="btn" onClick={close}>Cancelar</button>
          <button type="submit" className="btn primary" disabled={busy || missing}>Abrir</button>
        </div>
      </form>
    </Modal>
  );
}
