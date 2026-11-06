import { useEffect, useState } from "react";
import { api, errMsg, type ProjetoInfo } from "../../lib/ipc";
import { useStore, wsOf } from "../../state/store";

type Json = Record<string, unknown>;
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const PERFIS: [string, string][] = [["", "Padrão do Regente"], ["economico", "Econômico"], ["equilibrado", "Equilibrado"], ["precisao", "Máxima precisão"]];
const CACHE: [string, string][] = [
  ["cargoTargetCompartilhado", "Compartilhar o cache de build do Cargo entre worktrees"],
  ["nodeModulesHardlink", "Usar hardlink para node_modules nos worktrees"],
  ["nodeModulesJuncao", "Se o hardlink falhar, ligar node_modules por junção"],
  ["venvCompartilhado", "Compartilhar o venv do Python (opt-in; ignorado se o projeto tem instalação editável)"],
  ["preaquecer", "Pré-aquecer o build (roda o build.rs/scripts do projeto confiado)"],
];
// padrões do Rust (`worktree_cache::CacheBuild::de`): junção e venv compartilhado começam desligadas
const CACHE_DEFAULT: Json = { cargoTargetCompartilhado: true, nodeModulesHardlink: true, nodeModulesJuncao: false, venvCompartilhado: false, preaquecer: true };

/// Comandos que a confiança cobre: os do arquivo salvo (não os da edição em andamento).
function comandosSalvos(valor: Json | null): [string, string][] {
  const c = (valor?.comandos ?? {}) as Json;
  const out: [string, string][] = [];
  if (typeof c.build === "string") out.push(["build", c.build]);
  if (typeof c.testar === "string") out.push(["testar", c.testar]);
  for (const [k, v] of Object.entries((c.relacionados ?? {}) as Record<string, string>)) out.push([`relacionados (${k})`, v]);
  return out;
}

export default function ProjectSection() {
  const all = useStore((s) => s.workspaces);
  const workspaces = all.filter((w) => !w.path.startsWith("ssh://"));
  const active = useStore((s) => s.active);
  const [wsId, setWsId] = useState<string | null>(active ?? workspaces[0]?.id ?? null);
  const [info, setInfo] = useState<ProjetoInfo | null>(null);
  const [trusted, setTrusted] = useState(false);
  const [draft, setDraft] = useState<Json>({});
  const ws = wsOf(wsId);

  async function load(id: string) {
    try {
      const [i, t] = await Promise.all([api.projetoGet(id), api.projetoTrusted(id)]);
      setInfo(i);
      setTrusted(t);
      setDraft(i.valor ?? {});
    } catch (e) {
      setInfo(null);
      toast(e);
    }
  }

  useEffect(() => {
    if (wsId) void load(wsId);
    else setInfo(null);
  }, [wsId]);

  const valor = info?.valor ?? null;
  const comandos = (draft.comandos ?? {}) as Json;
  const relacionados = (comandos.relacionados ?? {}) as Record<string, string>;
  const gitCfg = (draft.git ?? {}) as Json;
  const cache = (draft.cacheBuild ?? CACHE_DEFAULT) as Json;
  const setCmd = (k: string, v: string) => setDraft({ ...draft, comandos: { ...comandos, [k]: v } });
  const setRel = (k: string, v: string) => setDraft({ ...draft, comandos: { ...comandos, relacionados: { ...relacionados, [k]: v } } });

  async function criar() {
    if (!wsId || !info) return;
    const doc = { versao: 1, stacks: info.detectado.stacks, comandos: info.detectado.comandos, cacheBuild: CACHE_DEFAULT };
    try {
      await api.projetoSave(wsId, doc);
      await load(wsId);
    } catch (e) {
      toast(e);
    }
  }

  async function salvar() {
    if (!wsId) return;
    const next: Json = { ...draft, comandos: { ...comandos, relacionados } };
    if (!next.perfil) delete next.perfil;
    if (!next.glossario) delete next.glossario;
    if (!(next.git as Json | undefined)?.base) delete next.git;
    try {
      await api.projetoSave(wsId, next);
      useStore.getState().toast("Projeto salvo.", "ok");
      await load(wsId);
    } catch (e) {
      toast(e);
    }
  }

  // confia no hash que está na tela; se o arquivo mudou desde a leitura, o backend recusa e a tela recarrega
  async function confiar(on: boolean) {
    if (!wsId) return;
    try {
      await api.projetoTrust(wsId, on, info?.hash ?? null);
      setTrusted(await api.projetoTrusted(wsId));
    } catch (e) {
      toast(e);
      void load(wsId);
    }
  }

  async function abrirGlossario() {
    if (!wsId) return;
    const rel = typeof draft.glossario === "string" && draft.glossario ? draft.glossario : ".regente/glossario.md";
    try {
      const abs = await api.projetoGlossario(wsId, rel);
      const st = useStore.getState();
      st.patch({ openFile: { ...st.openFile, [wsId]: { path: abs } }, view: { ...st.view, [wsId]: "arquivos" } });
    } catch (e) {
      toast(e);
    }
  }

  const temComandos = !!(comandos.build || comandos.testar);

  return (
    <section aria-label="Projeto">
      <h3>Projeto</h3>
      <p className="meta">Configuração do projeto em <code>.regente/projeto.json</code>. Este arquivo pode ser commitado para a equipe usar a mesma configuração.</p>
      <label className="row">
        Workspace{" "}
        <select aria-label="Workspace do projeto" value={wsId ?? ""} onChange={(e) => setWsId(e.target.value || null)}>
          {workspaces.map((w) => <option key={w.id} value={w.id}>{w.name}</option>)}
        </select>
      </label>
      {!ws && <p className="meta">Abra um workspace local para configurar o projeto.</p>}
      {ws && info && (
        <>
          <p className="meta">Stacks detectadas: {info.detectado.stacks.length ? info.detectado.stacks.join(", ") : "nenhuma"}</p>
          {!valor && (
            <div className="row">
              <span>Este workspace ainda não tem o arquivo.</span>
              <button onClick={criar}>Criar com a configuração detectada</button>
            </div>
          )}
          {valor && (
            <>
              <label>Comando de build <input aria-label="Comando de build" value={(comandos.build as string) ?? ""} onChange={(e) => setCmd("build", e.target.value)} /></label>
              <label>Comando de testes <input aria-label="Comando de testes" value={(comandos.testar as string) ?? ""} onChange={(e) => setCmd("testar", e.target.value)} /></label>
              {Object.keys(relacionados).map((k) => (
                <label key={k}>Testes relacionados ({k}) <input aria-label={`Testes relacionados ${k}`} value={relacionados[k]} onChange={(e) => setRel(k, e.target.value)} /></label>
              ))}
              <label>Branch base do Git (git.base)
                <input aria-label="Branch base do Git" placeholder="detectada pelo remoto (ex.: main)" value={(gitCfg.base as string) ?? ""}
                  onChange={(e) => setDraft({ ...draft, git: { ...gitCfg, base: e.target.value } })} />
              </label>
              <p className="meta">De onde nascem as branches de trabalho e para onde vão os PRs (ex.: <code>dev</code>). As missões adotadas comparam com <code>origin/&lt;base&gt;</code>.</p>
              <div className="row">
                <label>Glossário <input aria-label="Caminho do glossário" value={(draft.glossario as string) ?? ".regente/glossario.md"} onChange={(e) => setDraft({ ...draft, glossario: e.target.value })} /></label>
                <button onClick={() => void abrirGlossario()}>Abrir glossário</button>
              </div>
              <label>Perfil preferido{" "}
                <select aria-label="Perfil preferido" value={(draft.perfil as string) ?? ""} onChange={(e) => setDraft({ ...draft, perfil: e.target.value })}>
                  {PERFIS.map(([id, nome]) => <option key={id} value={id}>{nome}</option>)}
                </select>
              </label>
              {CACHE.map(([k, texto]) => (
                <label key={k} className="row">
                  <input type="checkbox" checked={(cache[k] ?? CACHE_DEFAULT[k]) === true} onChange={(e) => setDraft({ ...draft, cacheBuild: { ...cache, [k]: e.target.checked } })} /> {texto}
                </label>
              ))}
              <button onClick={salvar}>Salvar</button>
            </>
          )}
          {info.erro && <p className="meta">Não li o arquivo: {info.erro}</p>}
          {temComandos && (
            <>
              <label className="row">
                <input type="checkbox" aria-label="Confiar nos comandos deste projeto" checked={trusted} disabled={!info.hash} onChange={(e) => confiar(e.target.checked)} /> Confiar nos comandos deste projeto
              </label>
              <ul className="meta" aria-label="Comandos do projeto">
                {comandosSalvos(valor).map(([nome, cmd]) => <li key={nome}>{nome}: <code>{cmd}</code></li>)}
              </ul>
              {/* preaquecer entra no hash da confiança: a pessoa vê o que está confiando */}
              <p className="meta">Pré-aquecer o build ao abrir: {((valor?.cacheBuild as Json | undefined)?.preaquecer ?? true) ? "ligado" : "desligado"} (também entra na confiança).</p>
              <p className="meta">Confiar executa estes comandos exatamente como estão; scripts do próprio projeto (package.json, build.rs, Makefile) podem mudar com um git pull.</p>
            </>
          )}
          {temComandos && !trusted && <p className="meta">Este projeto define comandos (npm test, cargo build). Permitir que o Regente os rode automaticamente? Se os comandos mudarem, a confiança é pedida de novo.</p>}
        </>
      )}
    </section>
  );
}
