import { lazy, Suspense, useState } from "react";
import { api, type PaneSpec } from "../../lib/ipc";
import { rememberPick } from "../../lib/lastAgent";
import { useStore, wsOf } from "../../state/store";
import Modal from "../Modal";
import { focusNewPane } from "../TerminalPane";

// cada seção é carregada quando aberta. Seção nova: `const X = lazy(() => import("./XSection"));` e uma linha no corpo.
const AgenticSection = lazy(() => import("./AgenticSection"));
const CronSection = lazy(() => import("./CronSection"));
const VoiceSection = lazy(() => import("./VoiceSection"));
const MediaSection = lazy(() => import("./MediaSection"));
const ShotSection = lazy(() => import("./ShotSection"));
const RedlineSection = lazy(() => import("./RedlineSection"));
const IaAuxiliarSection = lazy(() => import("./IaAuxiliarSection"));
const MemorySection = lazy(() => import("./MemorySection"));
const AisSection = lazy(() => import("./AisSection"));
const CatalogSection = lazy(() => import("./CatalogSection"));
const RecipesSection = lazy(() => import("./RecipesSection"));
const SquadsSection = lazy(() => import("./SquadsSection"));
const GeneralSection = lazy(() => import("./GeneralSection"));
const LogsSection = lazy(() => import("./LogsSection"));
const PricesSection = lazy(() => import("./PricesSection"));
const UsageSection = lazy(() => import("./UsageSection"));
const VaultSection = lazy(() => import("./VaultSection"));
const ConectoresSection = lazy(() => import("./ConectoresSection"));
const CelularSection = lazy(() => import("./CelularSection"));
const UpdateSection = lazy(() => import("./UpdateSection"));
const ShortcutsSection = lazy(() => import("./ShortcutsSection"));
const PromptsSection = lazy(() => import("./PromptsSection"));
const ProfileSection = lazy(() => import("./ProfileSection"));
const ExecSection = lazy(() => import("./ExecSection"));
const ProjectSection = lazy(() => import("./ProjectSection"));
const AwaySection = lazy(() => import("./AwaySection"));
const NotifySection = lazy(() => import("./NotifySection"));
const HibernateSection = lazy(() => import("./HibernateSection"));

/** Abre um painel no workspace ativo (instalar CLI, login de conta), na missão selecionada. true = abriu. */
export async function spawnInActive(spec: Omit<PaneSpec, "workspaceId" | "cwd">): Promise<boolean> {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  if (!ws) {
    st.toast("Abra um workspace primeiro.", "erro");
    return false;
  }
  try {
    const row = await api.paneSpawn({ ...spec, missionId: st.missionSel[ws.id] ?? null, workspaceId: ws.id, cwd: ws.path });
    st.upsertPane(row);
    focusNewPane(row.id);
    if (spec.kind === "agent" && spec.cli) rememberPick(spec.cli, { model: spec.model ?? null, effort: spec.effort ?? null });
    st.patch({ settingsOpen: false });
    return true;
  } catch (e) {
    st.toastErr(e);
    return false;
  }
}

/** busca sem acento e sem caixa */
const norm = (s: string) => s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

const SECTIONS = [
  { id: "ias", label: "Suas IAs", keys: "ia cli claude codex gemini conta rota openrouter deepseek ollama instalar detectar login" },
  { id: "conectores", label: "Conectores MCP", keys: "mcp conector conectores servidor context7 conta contas compartilhar importar ferramenta" },
  { id: "catalogo", label: "Catálogo", keys: "agente skill marketplace instalar catálogo claude codex" },
  { id: "squads", label: "Squads", keys: "squad time maestro intake gate fases roster former receita" },
  { id: "receitas", label: "Receitas", keys: "receita atalho modelo pré-configurar missão" },
  { id: "agentica", label: "Missão agêntica", keys: "agêntica agentico piloto tarefa decisor modelo decisão automática escolher agente gasto" },
  { id: "ia", label: "IA auxiliar", keys: "ia auxiliar modelo anthropic openai claude chave sugestão commit briefing" },
  { id: "voz", label: "Voz", keys: "voz microfone ditado fala transcrição stt whisper groq tecla atalho idioma termos" },
  { id: "midia", label: "Mídia", keys: "mídia midia imagem imagens vídeo video áudio audio gemini gpt openai replicate elevenlabs gerar geração chave" },
  { id: "shot", label: "Captura de tela", keys: "shot captura tela print screenshot região gravar gravação atalho galeria imagem" },
  { id: "redline", label: "Limites de uso", keys: "limite limites uso consumo claude codex conta chip topo barra janela cota" },
  { id: "atualizacoes", label: "Atualizações", keys: "atualização atualizar update versão canal nova versão instalar reiniciar" },
  { id: "celular", label: "Celular", keys: "celular telefone pwa qr código pareamento controle remoto mobile rede tailscale aparelho" },
  { id: "agendamentos", label: "Agendamentos", keys: "agendamento cron agendar horario rodar diario semanal mensal" },
  { id: "memoria", label: "Memória", keys: "memoria memória memory lembrar fato decisão risco checkpoint" },
  { id: "precos", label: "Preços", keys: "preço custo tabela modelo token" },
  { id: "geral", label: "Aparência e geral", keys: "tema escuro claro fonte terminal shell concorrência onboarding sobre versão" },
  { id: "atalhos", label: "Atalhos", keys: "teclado atalho ctrl tecla" },
  { id: "prompts", label: "Prompts", keys: "prompt prompts snippet texto colar variável clipboard área de transferência revisar enter" },
  { id: "cofre", label: "Cofre", keys: "chave segredo api key senha" },
  { id: "uso", label: "Custos e orçamento", keys: "uso custos orçamento orcamento limite tokens gasto gráfico estatística custo" },
  { id: "notificacoes", label: "Notificações", keys: "notificação notificacao notificar aviso windows agente terminou pergunta gate" },
  { id: "hibernacao", label: "Hibernação", keys: "hibernação hibernar ocioso memória ram liberar painel automática" },
  { id: "logs", label: "Logs e diagnóstico", keys: "log diagnóstico erro suporte pasta problema" },
  // ▼ trilha A
  { id: "perfil", label: "Perfil de trabalho", keys: "perfil perfis modelo esforço economia econômico equilibrado precisão compactar contexto ajustes automáticos desfazer tokens" },
  { id: "execucoes", label: "Execuções e subagentes", keys: "execução execuções execucoes comando shell bash powershell painel promover servidor teste saída segundo plano tarefa subagente subagentes subtarefa subtarefas faixa atividade gaveta" },
  // ▲ trilha A
  // ▼ trilha B
  // ▲ trilha B
  // ▼ trilha C
  { id: "ausencia", label: "Enquanto você estava fora", keys: "digest ausência ausente foco janela voltar minutos resumo missões" },
  { id: "projeto", label: "Projeto", keys: "projeto projeto.json stack stacks comandos build testar teste relacionados glossário glossario perfil confiança confiar commit equipe cache" },
  // ▲ trilha C
] as const;
type SectionId = (typeof SECTIONS)[number]["id"];

/** Cabeçalhos do menu lateral. Ids que não existem em SECTIONS são pulados; seção sem grupo vai para "Outros". */
const GROUPS: [string, string[]][] = [
  ["IAs e agentes", ["ias", "conectores", "ia", "catalogo", "squads", "receitas", "agentica", "perfil", "projeto", "execucoes", "agendamentos", "memoria", "prompts"]],
  ["Voz e mídia", ["voz", "midia", "shot"]],
  ["Custos e limites", ["uso", "precos", "redline"]],
  ["Dispositivos e segurança", ["celular", "cofre", "atualizacoes"]],
  ["Aplicativo", ["geral", "notificacoes", "hibernacao", "ausencia", "atalhos", "logs"]],
];
const byId = (id: string) => SECTIONS.find((s) => s.id === id);
const grouped = new Set(GROUPS.flatMap(([, ids]) => ids));
const NAV: [string, (typeof SECTIONS)[number][]][] = [
  ...GROUPS.map(([name, ids]) => [name, ids.map(byId).filter((s): s is (typeof SECTIONS)[number] => !!s)] as [string, (typeof SECTIONS)[number][]]),
  ["Outros", SECTIONS.filter((s) => !grouped.has(s.id))],
];

/** A paleta de comandos abre cada seção pelo id. */
export { SECTIONS as SETTINGS_SECTIONS };

export default function Settings() {
  // a paleta pode abrir direto numa seção; sem isso, a primeira
  const [sec, setSec] = useState<SectionId>(() => (useStore.getState().settingsSection as SectionId | null) ?? "ias");
  const [q, setQ] = useState("");
  const close = () => useStore.getState().patch({ settingsOpen: false, settingsSection: null });
  const hit = (s: (typeof SECTIONS)[number], term: string) => norm(`${s.label} ${s.keys}`).includes(norm(term));
  const nav = NAV.map(([name, list]) => [name, list.filter((s) => !q || hit(s, q))] as const).filter(([, list]) => list.length > 0);
  return (
    <Modal label="Configurações" onClose={close} style={{ width: "auto", display: "flex", flexDirection: "column", overflow: "hidden" }}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <h2>Configurações</h2>
        <button className="icon-btn" aria-label="Fechar configurações" onClick={close}>✕</button>
      </div>
      <div className="settings">
        <nav className="settings-nav" aria-label="Seções">
          <input placeholder="Buscar" aria-label="Buscar nas configurações" value={q} onChange={(e) => { setQ(e.target.value); const first = nav.flatMap(([, list]) => list).find((s) => hit(s, e.target.value)); if (first) setSec(first.id); }} />
          {nav.map(([name, list]) => (
            <div key={name} role="group" aria-label={name} className="settings-group">
              <h4>{name}</h4>
              {list.map((s) => <button key={s.id} className={`rail-item ${sec === s.id ? "sel" : ""}`} aria-current={sec === s.id} onClick={() => setSec(s.id)}>{s.label}</button>)}
            </div>
          ))}
        </nav>
        <div className="settings-body">
          <Suspense fallback={<div className="empty">Carregando…</div>}>
          {sec === "ias" && <AisSection />}
          {sec === "conectores" && <ConectoresSection />}
          {sec === "catalogo" && <CatalogSection />}
          {sec === "squads" && <SquadsSection />}
          {sec === "agentica" && <AgenticSection />}
          {sec === "voz" && <VoiceSection />}
          {sec === "midia" && <MediaSection />}
          {sec === "shot" && <ShotSection />}
          {sec === "redline" && <RedlineSection />}
          {sec === "ia" && <IaAuxiliarSection />}
          {sec === "celular" && <CelularSection />}
          {sec === "atualizacoes" && <UpdateSection />}
          {sec === "agendamentos" && <CronSection />}
          {sec === "memoria" && <MemorySection />}
          {sec === "receitas" && <RecipesSection />}
          {sec === "precos" && <PricesSection />}
          {sec === "geral" && <GeneralSection />}
          {sec === "notificacoes" && <NotifySection />}
          {sec === "hibernacao" && <HibernateSection />}
          {sec === "atalhos" && <ShortcutsSection />}
          {sec === "prompts" && <PromptsSection />}
          {sec === "cofre" && <VaultSection />}
          {sec === "uso" && <UsageSection />}
          {sec === "logs" && <LogsSection />}
          {/* ▼ trilha A (render) */}
          {sec === "perfil" && <ProfileSection />}
          {sec === "execucoes" && <ExecSection />}
          {/* ▲ trilha A (render) */}
          {/* ▼ trilha B (render) */}
          {/* ▲ trilha B (render) */}
          {/* ▼ trilha C (render) */}
          {sec === "projeto" && <ProjectSection />}
          {sec === "ausencia" && <AwaySection />}
          {/* ▲ trilha C (render) */}
          </Suspense>
        </div>
      </div>
    </Modal>
  );
}
