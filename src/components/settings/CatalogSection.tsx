import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { filterAgents, installMark } from "../../lib/catalog";
import { favKey, sortFavFirst } from "../../lib/favorites";
import { api, errMsg, type AgentDef, type BaseSkillStatus, type InstalledIn, type MarketItem, type SkillDef } from "../../lib/ipc";
import { useStore } from "../../state/store";
import Modal from "../Modal";
import FavButton from "./FavButton";
import StacksTab from "./StacksTab";

/** Referência estável (um `[]` novo por leitura faz o React 19 laçar). */
const NO_FAVS: string[] = [];
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const SOURCE: Record<string, string> = { original: "original", regente: "Regente", claude: "Claude", codex: "Codex" };
const TARGET: Record<string, string> = { claude: "Claude", codex: "Codex", antigravity: "Antigravity", regente: "Regente" };
const MAX_ITEMS = 50;

/** Skill do Regente: as skills de papel na pasta de skills do Claude, para valerem fora dos painéis. */
function BaseSkillCard() {
  const [st, setSt] = useState<BaseSkillStatus | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const load = () => api.baseSkillStatus().then(setSt).catch(toast);
  useEffect(() => { load(); }, []);
  const run = (f: () => Promise<unknown>) => { setBusy(true); f().then(load).catch(toast).finally(() => setBusy(false)); };
  async function remove() {
    if (!st) return;
    const ok = await ask(`Remover as skills do Regente de ${st.target}? Os arquivos vão para a Lixeira.`, { title: "Remover skill do Regente", kind: "warning" });
    if (ok) run(() => api.baseSkillRemove());
  }
  return (
    <div className="card" style={{ marginBottom: 12 }}>
      <b>Skill do Regente</b>
      <p className="meta">As skills de papel (orquestrar, executar, revisar, pesquisar, planejar, testar) valem também fora dos painéis do Regente, no Claude.</p>
      {st && (
        <p className="meta">
          {st.installed.length > 0 ? `Instaladas: ${st.installed.join(", ")}.` : "Nenhuma instalada."}
          {st.conflicts.length > 0 && ` Não sobrescrevemos as skills de mesmo nome que já existem: ${st.conflicts.join(", ")}.`}
        </p>
      )}
      <div className="row">
        <button className="btn primary" disabled={busy || !st || (st.missing.length === 0 && st.installed.length === 0)} onClick={() => setConfirm(true)}>{st && st.installed.length > 0 ? "Atualizar" : "Instalar"}</button>
        <button className="btn danger" disabled={busy || !st || st.installed.length === 0} onClick={remove}>Remover</button>
      </div>
      {confirm && st && (
        <Modal label="Instalar skill do Regente" onClose={() => setConfirm(false)}>
          <h2>Instalar skill do Regente</h2>
          <p>Vamos copiar {st.missing.length} {st.missing.length === 1 ? "skill nova" : "skills novas"}{st.installed.length > 0 && ` e atualizar ${st.installed.length} já instalada${st.installed.length === 1 ? "" : "s"}`} para:</p>
          <p><code>{st.target}</code></p>
          <p className="meta">Só escrevemos nas pastas novas ou nas que o Regente já instalou. Remover depois manda para a Lixeira.</p>
          <div className="actions">
            <button className="btn" onClick={() => setConfirm(false)}>Cancelar</button>
            <button className="btn primary" onClick={() => { setConfirm(false); run(() => api.baseSkillInstall().then((r) => { if (r.noSource.length) useStore.getState().toast(`Sem fonte, não instaladas: ${r.noSource.join(", ")}.`, "erro"); })); }}>{st.installed.length > 0 ? "Atualizar" : "Instalar"}</button>
          </div>
        </Modal>
      )}
    </div>
  );
}

export default function CatalogSection() {
  const [tab, setTab] = useState<"agentes" | "skills" | "stacks" | "marketplace">("agentes");
  return (
    <>
      <h3>Catálogo</h3>
      <BaseSkillCard />
      <div className="row" style={{ marginBottom: 12 }}>
        {([["agentes", "Agentes"], ["skills", "Skills"], ["stacks", "Stacks"], ["marketplace", "Marketplace"]] as const).map(([id, label]) => (
          <button key={id} className={`tb-btn ${tab === id ? "on" : ""}`} aria-pressed={tab === id} onClick={() => setTab(id)}>{label}</button>
        ))}
      </div>
      {tab === "agentes" && <AgentsTab />}
      {tab === "skills" && <SkillsTab />}
      {tab === "stacks" && <StacksTab />}
      {tab === "marketplace" && <MarketplaceTab />}
    </>
  );
}

function AgentsTab() {
  const [agents, setAgents] = useState<AgentDef[] | null>(null);
  const [skills, setSkills] = useState<SkillDef[]>([]);
  const [q, setQ] = useState("");
  const [source, setSource] = useState("todos");
  const [onlyRole, setOnlyRole] = useState(false);
  const [sel, setSel] = useState<string | null>(null);
  const [draft, setDraft] = useState<AgentDef | null>(null);
  const providers = useStore((s) => s.providers);
  const favs = useStore((s) => s.settings?.favorites ?? NO_FAVS);

  const load = () => Promise.all([api.catalogAgents(), api.catalogSkills()])
    .then(([a, s]) => { setAgents(a); setSkills(s); })
    .catch(toast);
  useEffect(() => { load(); }, []);

  if (agents === null) return <div className="meta">Carregando agentes…</div>;
  const shown = sortFavFirst(filterAgents(agents, { q, source, onlyRole }), (a) => favKey("agent", a.name), favs);
  const cur = agents.find((a) => a.name === sel) ?? null;
  const editable = cur?.source === "regente";
  const blank = (): AgentDef => ({ name: "", description: "", cli: "claude", model: null, effort: null, role: null, skills: [], body: "", source: "regente", path: "" });

  return (
    <>
      <div className="row" style={{ marginBottom: 8 }}>
        <input placeholder="Buscar agente" aria-label="Buscar agente" value={q} onChange={(e) => setQ(e.target.value)} />
        <select aria-label="Origem" value={source} onChange={(e) => setSource(e.target.value)}>
          <option value="todos">Todas as origens</option>
          <option value="original">Originais</option>
          <option value="regente">Regente</option>
          <option value="claude">Claude</option>
        </select>
        <label className="row"><input type="checkbox" checked={onlyRole} onChange={(e) => setOnlyRole(e.target.checked)} /> Tem tarefa</label>
        <span className="spacer" />
        <button className="btn primary" onClick={() => setDraft(blank())}>Novo agente</button>
      </div>
      {agents.length === 0 ? (
        <div className="card">Nenhum agente ainda. Crie um agente ou instale pelo Marketplace.</div>
      ) : shown.length === 0 ? (
        <div className="meta">Nenhum agente com esses filtros.</div>
      ) : (
        <div className="row" style={{ alignItems: "flex-start" }}>
          <ul style={{ listStyle: "none", margin: 0, padding: 0, flex: "1 1 240px", display: "grid", gap: 4 }} aria-label="Agentes">
            {shown.map((a) => (
              <li key={a.name} style={{ display: "flex", alignItems: "center", gap: 4 }}>
                <button className={`rail-item ${sel === a.name ? "sel" : ""}`} aria-current={sel === a.name} onClick={() => setSel(a.name)}>
                  <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{a.name}</span>
                  <span className="meta">{SOURCE[a.source] ?? a.source}{a.role ? ` · ${a.role}` : ""}</span>
                </button>
                <FavButton favKey={favKey("agent", a.name)} label={a.name} />
              </li>
            ))}
          </ul>
          {cur && (
            <div className="card" style={{ flex: "2 1 320px", minWidth: 0 }}>
              <strong>{cur.name}</strong> <span className="chip">{SOURCE[cur.source] ?? cur.source}</span>
              <p>{cur.description}</p>
              <p className="meta">
                IA: {cur.cli ?? "padrão"} · Modelo: {cur.model ?? "padrão"} · Esforço: {cur.effort ?? "padrão"}
                {cur.role ? ` · Tarefa: ${cur.role}` : ""}
              </p>
              <p className="meta">Skills: {cur.skills.length ? cur.skills.join(", ") : "nenhuma"}</p>
              <pre style={{ whiteSpace: "pre-wrap", fontFamily: "var(--mono)", fontSize: 12, maxHeight: 260, overflow: "auto" }}>{cur.body}</pre>
              <div className="row">
                {editable
                  ? <button className="btn" onClick={() => setDraft({ ...cur })}>Editar</button>
                  : <button className="btn" onClick={() => setDraft({ ...cur, name: `${cur.name}-meu`, source: "regente", path: "" })}>Duplicar para editar</button>}
                {!editable && <span className="meta">Originais vêm com o Regente e não mudam.</span>}
              </div>
            </div>
          )}
        </div>
      )}
      {draft && (
        <AgentForm draft={draft} isNew={!draft.path} providers={providers?.clis ?? []} skills={skills} onClose={() => setDraft(null)}
          onSaved={(name) => { setDraft(null); setSel(name); load(); useStore.getState().toast("Agente salvo", "ok"); }} />
      )}
    </>
  );
}

function AgentForm({ draft, isNew, providers, skills, onClose, onSaved }: {
  draft: AgentDef; isNew: boolean; providers: { id: string; label: string; efforts: string[]; models: string[] }[]; skills: SkillDef[];
  onClose(): void; onSaved(name: string): void;
}) {
  const [a, setA] = useState<AgentDef>(draft);
  const [busy, setBusy] = useState(false);
  const cli = providers.find((c) => c.id === a.cli);
  const set = (p: Partial<AgentDef>) => setA((x) => ({ ...x, ...p }));
  async function save() {
    if (busy) return;
    setBusy(true);
    try {
      await api.catalogSaveAgent({ ...a, name: a.name.trim(), description: a.description.trim(), cli: a.cli || null, model: a.model || null, effort: a.effort || null, role: a.role?.trim() || null });
      onSaved(a.name.trim());
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal label={isNew ? "Novo agente" : "Editar agente"} onClose={onClose}>
      <form onSubmit={(e) => { e.preventDefault(); save(); }}>
        <h2>{isNew ? "Novo agente" : "Editar agente"}</h2>
        <label className="field"><span>Nome</span><input value={a.name} disabled={!isNew} placeholder="ex.: revisor-de-texto" onChange={(e) => set({ name: e.target.value })} /></label>
        <label className="field"><span>Descrição</span><input value={a.description} onChange={(e) => set({ description: e.target.value })} /></label>
        <div className="row">
          <label className="field"><span>IA</span>
            <select value={a.cli ?? ""} onChange={(e) => set({ cli: e.target.value || null, effort: null })}>
              <option value="">padrão</option>
              {providers.map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
            </select>
          </label>
          <label className="field"><span>Modelo</span><input list="catalog-models" value={a.model ?? ""} placeholder="padrão" onChange={(e) => set({ model: e.target.value })} />
            <datalist id="catalog-models">{(cli?.models ?? []).map((m) => <option key={m} value={m} />)}</datalist>
          </label>
          <label className="field"><span>Quanto pensa</span>
            <select value={a.effort ?? ""} onChange={(e) => set({ effort: e.target.value || null })}>
              <option value="">padrão</option>
              {(cli?.efforts ?? []).map((x) => <option key={x} value={x}>{x}</option>)}
            </select>
          </label>
          <label className="field"><span>Tarefa (função)</span><input value={a.role ?? ""} placeholder="vazio = sem tarefa" onChange={(e) => set({ role: e.target.value })} /></label>
        </div>
        <fieldset className="field" style={{ border: 0, padding: 0 }}>
          <legend>Skills</legend>
          {skills.length === 0 && <span className="meta">Nenhuma skill instalada.</span>}
          <div className="row">
            {skills.map((s) => (
              <label key={s.name} className="chip">
                <input type="checkbox" checked={a.skills.includes(s.name)} onChange={(e) => set({ skills: e.target.checked ? [...a.skills, s.name] : a.skills.filter((x) => x !== s.name) })} />
                {s.name}
              </label>
            ))}
          </div>
        </fieldset>
        <label className="field"><span>Instruções</span><textarea rows={8} value={a.body} onChange={(e) => set({ body: e.target.value })} /></label>
        <div className="actions">
          <button type="button" className="btn" onClick={onClose}>Cancelar</button>
          <button type="submit" className="btn primary" disabled={busy || !a.name.trim()}>Salvar</button>
        </div>
      </form>
    </Modal>
  );
}

function SkillsTab() {
  const favs = useStore((s) => s.settings?.favorites ?? NO_FAVS);
  const [skills, setSkills] = useState<SkillDef[] | null>(null);
  const [sel, setSel] = useState<SkillDef | null>(null);
  const [text, setText] = useState<string | null>(null);
  useEffect(() => { api.catalogSkills().then(setSkills).catch(toast); }, []);
  useEffect(() => {
    if (!sel) return;
    let alive = true;
    setText(null);
    api.catalogSkillText(sel.dir).then((t) => { if (alive) setText(t); }).catch((e) => { if (alive) { setText(""); toast(e); } });
    return () => { alive = false; };
  }, [sel]);

  if (skills === null) return <div className="meta">Carregando skills…</div>;
  if (skills.length === 0) return <div className="card">Nenhuma skill instalada. Instale pelo Marketplace.</div>;
  return (
    <div className="row" style={{ alignItems: "flex-start" }}>
      <ul style={{ listStyle: "none", margin: 0, padding: 0, flex: "1 1 240px", display: "grid", gap: 4 }} aria-label="Skills">
        {sortFavFirst(skills, (s) => favKey("skill", s.name), favs).map((s) => (
          <li key={s.dir} style={{ display: "flex", alignItems: "center", gap: 4 }}>
            <button className={`rail-item ${sel?.dir === s.dir ? "sel" : ""}`} aria-current={sel?.dir === s.dir} onClick={() => setSel(s)}>
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{s.name}</span>
              <span className="meta">{SOURCE[s.source] ?? s.source}</span>
            </button>
            <FavButton favKey={favKey("skill", s.name)} label={s.name} />
          </li>
        ))}
      </ul>
      {sel && (
        <div className="card" style={{ flex: "2 1 320px", minWidth: 0 }}>
          <strong>{sel.name}</strong> <span className="chip">{SOURCE[sel.source] ?? sel.source}</span>
          <p>{sel.description}</p>
          {text === null ? <div className="meta">Carregando SKILL.md…</div> : (
            <pre style={{ whiteSpace: "pre-wrap", fontFamily: "var(--mono)", fontSize: 12, maxHeight: 360, overflow: "auto" }}>{text || "SKILL.md vazio ou ilegível."}</pre>
          )}
        </div>
      )}
    </div>
  );
}

function MarketplaceTab() {
  const [q, setQ] = useState("");
  const [debounced, setDebounced] = useState("");
  const [kind, setKind] = useState<"" | "skill" | "agent">("");
  const [items, setItems] = useState<MarketItem[] | null>(null);
  const [refresh, setRefresh] = useState(0);
  /** "Atualizar índice" ignora o cache de 24 h só nessa busca */
  const forceNext = useRef(false);
  useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 300);
    return () => clearTimeout(t);
  }, [q]);
  useEffect(() => {
    let alive = true;
    setItems(null);
    const force = forceNext.current;
    forceNext.current = false;
    api.marketplaceSearch(debounced, kind || null, force)
      .then((r) => { if (alive) setItems(r); })
      .catch((e) => { if (alive) { setItems([]); toast(e); } });
    return () => { alive = false; };
  }, [debounced, kind, refresh]);

  return (
    <>
      <div className="row" style={{ marginBottom: 8 }}>
        <input placeholder="Buscar skill ou agente" aria-label="Buscar no marketplace" value={q} onChange={(e) => setQ(e.target.value)} />
        <select aria-label="Tipo" value={kind} onChange={(e) => setKind(e.target.value as "" | "skill" | "agent")}>
          <option value="">Todos os tipos</option>
          <option value="skill">Skill</option>
          <option value="agent">Agente</option>
        </select>
        <button className="btn" disabled={items === null} onClick={() => { forceNext.current = true; setItems(null); setRefresh((n) => n + 1); }}>Atualizar índice</button>
      </div>
      {items === null ? <div className="meta" aria-live="polite">Carregando catálogo…</div>
        : items.length === 0 ? <div className="meta">Nenhum item encontrado.</div>
        : (
          <>
            {items.length > MAX_ITEMS && <div className="meta">Mostrando {MAX_ITEMS} de {items.length}; refine a busca.</div>}
            <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: 6 }} aria-label="Resultados">
              {items.slice(0, MAX_ITEMS).map((it) => <MarketRow key={`${it.source}/${it.kind}/${it.name}`} item={it} />)}
            </ul>
          </>
        )}
    </>
  );
}

function MarketRow({ item }: { item: MarketItem }) {
  const [inst, setInst] = useState<InstalledIn | null>(null);
  const [target, setTarget] = useState("regente");
  const [busy, setBusy] = useState(false);
  const load = () => api.marketplaceInstalled(item).then(setInst).catch(toast);
  useEffect(() => { load(); }, [item]);

  async function install() {
    // instalar no CLI muda o que as sessões dele carregam: pede confirmação
    // a pasta do destino aparece na confirmação (o Antigravity só tem pasta de skills)
    const where = target === "antigravity" ? " (pasta ~/.gemini/antigravity/skills)" : "";
    if (target !== "regente" && !(await ask(`Instalar "${item.name}" no ${TARGET[target]}${where}? Ele passa a valer para as sessões do ${TARGET[target]} neste computador.`, { title: "Instalar no CLI", kind: "warning" }))) return;
    setBusy(true);
    try {
      await api.marketplaceInstall(item, target);
      useStore.getState().toast(`${item.name} instalado no ${TARGET[target]}`, "ok");
      await load();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!(await ask(`Remover "${item.name}" do ${TARGET[target]}?`, { title: "Remover", kind: "warning" }))) return;
    setBusy(true);
    try {
      await api.marketplaceUninstall(item.name, item.kind, target);
      await load();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }
  return (
    <li className="card" style={{ marginBottom: 0 }}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <strong>{item.name}</strong>
        <span className="chip">{item.kind === "skill" ? "Skill" : "Agente"} · {item.source}</span>
      </div>
      <p className="meta" style={{ margin: "4px 0" }}>{item.description}</p>
      <div className="row">
        <span className="meta">instalado em: Claude {installMark(inst?.claude)} · Codex {installMark(inst?.codex)} · Antigravity {installMark(inst?.antigravity)} · Regente {installMark(inst?.regente)}</span>
        <span className="spacer" />
        <label className="row">
          <span className="meta">Instalar em</span>
          <select aria-label={`Instalar ${item.name} em`} value={target} onChange={(e) => setTarget(e.target.value)} disabled={busy}>
            {Object.entries(TARGET).filter(([id]) => !(item.kind === "agent" && id === "antigravity")).map(([id, label]) => <option key={id} value={id}>{label}</option>)}
          </select>
        </label>
        <button className="btn primary" disabled={busy} onClick={install}>Instalar</button>
        <button className="btn" disabled={busy} onClick={remove}>Remover</button>
      </div>
    </li>
  );
}
