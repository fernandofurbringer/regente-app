import { useEffect, useState } from "react";
import { intakeToPairs, pairsToIntake, type IntakePair } from "../../lib/recipes";
import { favKey, sortFavFirst } from "../../lib/favorites";
import FavButton from "./FavButton";
import { copyId } from "../../lib/squads";
import { api, errMsg, type RecipeDef, type SquadDef } from "../../lib/ipc";
import { useStore } from "../../state/store";

/** Referência estável (um `[]` novo por leitura faz o React 19 laçar). */
const NO_FAVS: string[] = [];

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const MODES: { id: string; label: string }[] = [
  { id: "livre", label: "Livre" },
  { id: "squad", label: "Squad" },
  { id: "agentico", label: "Agêntico" },
];
const blank = (): RecipeDef => ({
  id: "", name: "", description: "", mode: "livre", squad: null, pilot: null, slots: [], intake: {}, worktree: null, style: null, source: "regente",
});

export default function RecipesSection() {
  const favs = useStore((s) => s.settings?.favorites ?? NO_FAVS);
  const [recipes, setRecipes] = useState<RecipeDef[] | null>(null);
  const [squads, setSquads] = useState<SquadDef[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [draft, setDraft] = useState<RecipeDef | null>(null);
  const [pairs, setPairs] = useState<IntakePair[]>([]);
  const [busy, setBusy] = useState(false);
  const providers = useStore((s) => s.providers);

  const load = () => api.recipesList().then((l) => { setRecipes(l); return l; }).catch((e) => { setRecipes([]); toast(e); return [] as RecipeDef[]; });
  useEffect(() => {
    load();
    api.squadsList().then(setSquads).catch(toast);
  }, []);

  function open(r: RecipeDef) {
    setSel(r.id);
    setDraft(JSON.parse(JSON.stringify(r)) as RecipeDef);
    setPairs(intakeToPairs(r.intake));
  }
  function duplicate(r: RecipeDef) {
    const copy = { ...(JSON.parse(JSON.stringify(r)) as RecipeDef), id: copyId(r.id, (recipes ?? []).map((x) => x.id)), source: "regente" };
    setSel(null);
    setDraft(copy);
    setPairs(intakeToPairs(copy.intake));
  }
  async function save() {
    if (!draft || busy) return;
    setBusy(true);
    try {
      const next = { ...draft, intake: pairsToIntake(pairs) };
      await api.recipeSave(next);
      useStore.getState().toast("Receita salva", "ok");
      const list = await load();
      setSel(next.id);
      const saved = list.find((x) => x.id === next.id);
      if (saved) open(saved);
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  const readonly = draft?.source === "original";
  const isNew = draft !== null && !(recipes ?? []).some((r) => r.id === draft.id);
  const pilotCli = providers?.clis.find((c) => c.id === draft?.pilot?.cli);

  return (
    <>
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 10 }}>
        <h3 style={{ margin: 0 }}>Receitas</h3>
        <button className="btn primary" onClick={() => { setSel(null); setDraft(blank()); setPairs([]); }}>Nova receita</button>
      </div>
      <div className="row" style={{ alignItems: "flex-start" }}>
        <div style={{ flex: "1 1 200px" }}>
          {recipes === null ? <div className="meta">Carregando receitas…</div>
            : recipes.length === 0 ? <div className="meta">Nenhuma receita ainda.</div>
            : (
              <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: 4 }} aria-label="Receitas">
                {sortFavFirst(recipes, (r) => favKey("recipe", r.id), favs).map((r) => (
                  <li key={r.id} style={{ display: "flex", alignItems: "center", gap: 4 }}>
                    <button className={`rail-item ${sel === r.id ? "sel" : ""}`} aria-current={sel === r.id} onClick={() => open(r)}>
                      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{r.name}</span>
                      <span className="meta">{r.source === "original" ? "original" : "Regente"}</span>
                    </button>
                    <FavButton favKey={favKey("recipe", r.id)} label={r.name} />
                  </li>
                ))}
              </ul>
            )}
        </div>
        {draft && (
          <div className="card" style={{ flex: "3 1 420px", minWidth: 0 }}>
            {readonly && (
              <div className="row" style={{ justifyContent: "space-between" }}>
                <span className="meta">Original: somente leitura.</span>
                <button className="btn" onClick={() => duplicate(draft)}>Duplicar para editar</button>
              </div>
            )}
            <fieldset disabled={readonly} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
              <div className="row">
                <label className="field"><span>Id</span><input value={draft.id} disabled={!isNew} placeholder="ex.: landing-rapida" onChange={(e) => setDraft({ ...draft, id: e.target.value })} /></label>
                <label className="field" style={{ flex: 1 }}><span>Nome</span><input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></label>
              </div>
              <label className="field"><span>Descrição</span><input value={draft.description} onChange={(e) => setDraft({ ...draft, description: e.target.value })} /></label>
              <div className="row">
                <label className="field"><span>Modo</span>
                  <select value={draft.mode} onChange={(e) => setDraft({ ...draft, mode: e.target.value as RecipeDef["mode"] })}>
                    {MODES.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
                  </select>
                </label>
                {draft.mode === "squad" && (
                  <label className="field"><span>Squad</span>
                    <select value={draft.squad ?? ""} onChange={(e) => setDraft({ ...draft, squad: e.target.value || null })}>
                      <option value="">escolha uma squad</option>
                      {squads.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
                    </select>
                  </label>
                )}
                <label className="field"><span>Worktree</span>
                  <select value={draft.worktree === null ? "" : String(draft.worktree)} onChange={(e) => setDraft({ ...draft, worktree: e.target.value === "" ? null : e.target.value === "true" })}>
                    <option value="">padrão do modo</option>
                    <option value="true">sim</option>
                    <option value="false">não</option>
                  </select>
                </label>
                {draft.mode === "agentico" && (
                  <label className="field"><span>Estilo</span>
                    <select value={draft.style ?? ""} onChange={(e) => setDraft({ ...draft, style: e.target.value || null })}>
                      <option value="">padrão</option>
                      <option value="chain">Chain</option>
                      <option value="paralelo">Paralelo</option>
                      <option value="torre">Torre de controle</option>
                      <option value="plano">Plano → Execução</option>
                    </select>
                  </label>
                )}
              </div>
              {draft.mode === "agentico" && (
                <div className="row">
                  <label className="field"><span>IA do piloto</span>
                    <select value={draft.pilot?.cli ?? ""} onChange={(e) => setDraft({ ...draft, pilot: { ...draft.pilot, cli: e.target.value || null } })}>
                      <option value="">padrão</option>
                      {(providers?.clis ?? []).map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
                    </select>
                  </label>
                  <label className="field"><span>Modelo</span><input value={draft.pilot?.model ?? ""} placeholder="padrão" onChange={(e) => setDraft({ ...draft, pilot: { ...draft.pilot, model: e.target.value || null } })} /></label>
                  {(pilotCli?.efforts.length ?? 0) > 0 ? (
                    <label className="field"><span>Quanto pensa</span>
                      <select value={draft.pilot?.effort ?? ""} onChange={(e) => setDraft({ ...draft, pilot: { ...draft.pilot, effort: e.target.value || null } })}>
                        <option value="">padrão</option>
                        {pilotCli?.efforts.map((x) => <option key={x} value={x}>{x}</option>)}
                      </select>
                    </label>
                  ) : (
                    <label className="field"><span>Quanto pensa</span><input value={draft.pilot?.effort ?? ""} placeholder="padrão" onChange={(e) => setDraft({ ...draft, pilot: { ...draft.pilot, effort: e.target.value || null } })} /></label>
                  )}
                </div>
              )}

              <h4>Respostas do intake</h4>
              <p className="meta">Pergunta → resposta. Quem usa a receita não é perguntado de novo.</p>
              {pairs.map((p, i) => (
                <div key={i} className="row">
                  <input aria-label={`Pergunta ${i + 1}`} value={p.q} placeholder="pergunta" style={{ flex: 1 }} onChange={(e) => setPairs(pairs.map((x, j) => (j === i ? { ...x, q: e.target.value } : x)))} />
                  <input aria-label={`Resposta ${i + 1}`} value={p.a} placeholder="resposta" style={{ flex: 1 }} onChange={(e) => setPairs(pairs.map((x, j) => (j === i ? { ...x, a: e.target.value } : x)))} />
                  <button className="icon-btn" aria-label={`Remover resposta ${i + 1}`} onClick={() => setPairs(pairs.filter((_, j) => j !== i))}>✕</button>
                </div>
              ))}
              <button className="btn" onClick={() => setPairs([...pairs, { q: "", a: "" }])}>+ Resposta</button>
            </fieldset>
            {!readonly && (
              <div className="actions">
                <button className="btn primary" disabled={busy || !draft.id.trim() || !draft.name.trim()} onClick={save}>Salvar</button>
              </div>
            )}
          </div>
        )}
      </div>
    </>
  );
}
