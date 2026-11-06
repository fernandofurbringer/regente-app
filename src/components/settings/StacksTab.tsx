import { useEffect, useState } from "react";
import { favKey, sortFavFirst } from "../../lib/favorites";
import { formFromStack, isStack, stackFromForm, type StackForm } from "../../lib/stacks";
import { api, errMsg, type AgentDef, type RecipeDef, type SkillDef } from "../../lib/ipc";
import { useStore } from "../../state/store";
import FavButton from "./FavButton";

/** Referência estável (um `[]` novo por leitura faz o React 19 laçar). */
const NO_FAVS: string[] = [];
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const blank = (): StackForm => ({ name: "", cli: "claude", model: "", effort: "", agent: "", skill: "", permission: "pend" });
const PERMS: { id: StackForm["permission"]; label: string }[] = [
  { id: "pend", label: "Padrão do CLI" },
  { id: "auto", label: "Auto" },
  { id: "plan", label: "Plano" },
];

/** Stacks: agente + skill + modelo + esforço com um nome. Aparecem no "Novo painel"; a estrela põe a stack no topo. */
export default function StacksTab() {
  const [stacks, setStacks] = useState<RecipeDef[] | null>(null);
  const [agents, setAgents] = useState<AgentDef[]>([]);
  const [skills, setSkills] = useState<SkillDef[]>([]);
  /** id da stack aberta; "" = nova; null = nenhuma */
  const [sel, setSel] = useState<string | null>(null);
  const [form, setForm] = useState<StackForm | null>(null);
  const [busy, setBusy] = useState(false);
  const providers = useStore((s) => s.providers);
  const favs = useStore((s) => s.settings?.favorites ?? NO_FAVS);
  const cli = providers?.clis.find((c) => c.id === form?.cli);

  const load = () => api.recipesList().then((l) => setStacks(l.filter(isStack))).catch((e) => { setStacks([]); toast(e); });
  useEffect(() => {
    load();
    api.catalogAgents().then(setAgents).catch(toast);
    api.catalogSkills().then(setSkills).catch(toast);
  }, []);

  function open(r: RecipeDef) {
    setSel(r.id);
    setForm(formFromStack(r));
  }
  async function save() {
    if (!form || busy || !form.name.trim()) return;
    setBusy(true);
    try {
      const id = sel || `stack-${Date.now()}`;
      await api.recipeSave(stackFromForm(form, id));
      useStore.getState().toast("Stack salva", "ok");
      setSel(id);
      await load();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!sel || busy || !window.confirm("Remover esta stack?")) return;
    setBusy(true);
    try {
      await api.recipeDelete(sel);
      setSel(null);
      setForm(null);
      await load();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  if (stacks === null) return <div className="meta">Carregando stacks…</div>;
  return (
    <>
      <p className="meta">Uma stack junta agente, skill, modelo e esforço sob um nome. Escolha-a em "Novo painel" para abrir o painel já configurado. A skill escolhida substitui as skills do agente.</p>
      <div className="row" style={{ justifyContent: "flex-end", marginBottom: 8 }}>
        <button className="btn primary" onClick={() => { setSel(""); setForm(blank()); }}>Nova stack</button>
      </div>
      <div className="row" style={{ alignItems: "flex-start" }}>
        <div style={{ flex: "1 1 220px" }}>
          {stacks.length === 0 ? <div className="meta">Nenhuma stack ainda.</div> : (
            <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: 4 }} aria-label="Stacks">
              {sortFavFirst(stacks, (r) => favKey("recipe", r.id), favs).map((r) => (
                <li key={r.id} style={{ display: "flex", alignItems: "center", gap: 4 }}>
                  <button className={`rail-item ${sel === r.id ? "sel" : ""}`} aria-current={sel === r.id} disabled={r.slots.length !== 1}
                    title={r.slots.length !== 1 ? "Stack de vários painéis (salva pela paleta): abra pela paleta de comandos" : r.description} onClick={() => open(r)}>
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{r.name}</span>
                    <span className="meta">{r.slots.length === 1 ? r.description : `${r.slots.length} painéis`}</span>
                  </button>
                  <FavButton favKey={favKey("recipe", r.id)} label={r.name} />
                </li>
              ))}
            </ul>
          )}
        </div>
        {form && (
          <div className="card" style={{ flex: "3 1 380px", minWidth: 0 }}>
            <label className="field"><span>Nome</span><input value={form.name} placeholder="ex.: Revisor de segurança" onChange={(e) => setForm({ ...form, name: e.target.value })} /></label>
            <div className="row">
              <label className="field"><span>IA</span>
                <select value={form.cli} onChange={(e) => setForm({ ...form, cli: e.target.value, model: "", effort: "" })}>
                  {(providers?.clis ?? []).map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
                </select>
              </label>
              <label className="field"><span>Modelo</span>
                <input list="stack-models" value={form.model} placeholder="padrão do CLI" onChange={(e) => setForm({ ...form, model: e.target.value })} />
                <datalist id="stack-models">{(cli?.models ?? []).map((m) => <option key={m} value={m} />)}</datalist>
              </label>
              {(cli?.efforts.length ?? 0) > 0 && (
                <label className="field"><span>Quanto pensa</span>
                  <select value={form.effort} onChange={(e) => setForm({ ...form, effort: e.target.value })}>
                    <option value="">padrão</option>
                    {cli?.efforts.map((x) => <option key={x} value={x}>{x}</option>)}
                  </select>
                </label>
              )}
            </div>
            <div className="row">
              <label className="field"><span>Agente</span>
                <select value={form.agent} onChange={(e) => setForm({ ...form, agent: e.target.value })}>
                  <option value="">nenhum</option>
                  {agents.map((a) => <option key={a.name} value={a.name}>{a.name}</option>)}
                </select>
              </label>
              <label className="field"><span>Skill</span>
                <select value={form.skill} onChange={(e) => setForm({ ...form, skill: e.target.value })}>
                  <option value="">as do agente (padrão)</option>
                  {skills.map((s) => <option key={s.name} value={s.name}>{s.name}</option>)}
                </select>
              </label>
              <label className="field"><span>Permissão</span>
                <select value={form.permission} onChange={(e) => setForm({ ...form, permission: e.target.value as StackForm["permission"] })}>
                  {PERMS.map((p) => <option key={p.id} value={p.id}>{p.label}</option>)}
                </select>
              </label>
            </div>
            <div className="actions">
              {sel && <button className="btn danger" disabled={busy} onClick={remove}>Remover</button>}
              <button className="btn primary" disabled={busy || !form.name.trim()} onClick={save}>Salvar</button>
            </div>
          </div>
        )}
      </div>
    </>
  );
}
