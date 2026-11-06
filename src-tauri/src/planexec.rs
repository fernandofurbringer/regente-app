//! Missão "Plano → Execução" (X1a). O planejador estuda e entrega o plano (`plan_submit`); o usuário aprova no gate
//! "Aprovar plano"; executores rápidos fazem cada tarefa, na ordem das dependências, até `paraleloMax` abertos ao mesmo
//! tempo. Estado: `plano.json`/`plano.md` são artefatos da missão; o andamento de cada tarefa fica na tabela `tasks`
//! (id `<missão>-<tarefa>`).
use crate::artefatos;
use crate::db::{PaneRow, TaskItem};
use crate::error::{fail, AppResult};
use crate::mcp::{check_spawn_budget, not_archived, own_mission};
use crate::orchestrator::{bracketed, clean_text};
use crate::perfil;
use crate::providers::{PaneKind, PaneSpec, Permission};
use crate::runtime::AppState;
use crate::status::PaneStatus;
use crate::squads;
use crate::util::{lk, now_ms};
use crate::verificacao::{self, VerifyCfg};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// Título do gate que aprova o plano (o `on_gate` só age nele).
pub const GATE_PLANO: &str = "Aprovar plano";
/// Limite de `check_gate` para o resumo do gate.
const RESUMO_GATE_MAX: usize = 3900;
const MAX_TAREFAS: usize = 30;
/// Prompt de sistema do planejador (pt-BR).
pub const PLANNER_RULES: &str = "Você é o PLANEJADOR desta missão. Seu trabalho é estudar e decidir, não implementar.
- Explore o código o quanto for preciso e decida. Não deixe para o executor nada que ele precise descobrir.
- Divida em tarefas pequenas e independentes. Cada tarefa lista os arquivos exatos (caminhos relativos ao projeto).
- Cada tarefa tem critérios de aceite como comandos exatos ou nomes de testes, e as dependências (ids de tarefas anteriores).
- Não edite arquivos do projeto. Resultado grande vai para arquivo, citado pelo caminho.
- Entregue o plano SOMENTE com plan_submit (objetivo, decisoes, tarefas, glossario opcional).
- Quando um executor perguntar, responda com plan_reply. Não explore no lugar dele.
- Ao receber \"Ajuste pedido no plano\", revise e reenvie o plano inteiro com plan_submit.";

/// Uma tarefa do plano (o formato do `plan_submit`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Tarefa {
    /// Id curto e estável (letras, números, `-` ou `_`, até 12). É a referência nas dependências.
    pub id: String,
    pub titulo: String,
    /// Arquivos exatos que a tarefa toca (caminhos relativos ao projeto).
    #[serde(default)]
    pub arquivos: Vec<String>,
    /// O que mudar, em prosa objetiva.
    pub mudanca: String,
    /// Critérios de aceite: comandos exatos ou nomes de testes.
    pub aceite: Vec<String>,
    /// Ids das tarefas que precisam estar concluídas antes.
    #[serde(default)]
    pub depende: Vec<String>,
    /// P, M ou G.
    pub tamanho: String,
}

/// O plano inteiro (argumentos do `plan_submit`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Plano {
    pub objetivo: String,
    #[serde(default)]
    pub decisoes: Vec<String>,
    pub tarefas: Vec<Tarefa>,
    #[serde(default)]
    pub glossario: Option<String>,
}

/// `plano.json`: a versão atual do plano e o planejador que a entregou.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Guardado {
    pub versao: u64,
    pub planejador: String,
    pub plano: Plano,
}

fn id_valido(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Caminho relativo ao projeto, sem `..`, sem drive, sem absoluto (o arquivo pode ainda não existir).
fn caminho_valido(f: &str) -> AppResult<()> {
    let ruim = f.trim().is_empty() || f.len() > 260 || f.contains(['\0', ':']) || f.starts_with(['/', '\\']) || f.split(['/', '\\']).any(|c| c == "..");
    if ruim {
        return fail("bad_plano", format!("arquivo inválido: «{f}» (caminho relativo ao projeto, sem .. nem ':')"));
    }
    Ok(())
}

/// Confere o plano e devolve as tarefas em ordem topológica (estável: a primeira pronta na ordem original).
pub fn validate(mut p: Plano) -> AppResult<Plano> {
    let cabe = |s: &str, max: usize| s.chars().count() <= max;
    if p.objetivo.trim().is_empty() || !cabe(&p.objetivo, 1000) {
        return fail("bad_plano", "objetivo: de 1 a 1000 caracteres");
    }
    if p.decisoes.len() > 30 || p.decisoes.iter().any(|d| !cabe(d, 500)) {
        return fail("bad_plano", "decisões: até 30, cada uma com até 500 caracteres");
    }
    if p.glossario.as_deref().is_some_and(|g| !cabe(g, 4000)) {
        return fail("bad_plano", "glossário: até 4000 caracteres");
    }
    if p.tarefas.is_empty() || p.tarefas.len() > MAX_TAREFAS {
        return fail("bad_plano", "o plano tem de 1 a 30 tarefas");
    }
    let mut ids = HashSet::new();
    for t in &p.tarefas {
        if !id_valido(&t.id) {
            return fail("bad_plano", format!("id de tarefa inválido: «{}» (letras, números, - ou _, até 12)", t.id));
        }
        if !ids.insert(t.id.as_str()) {
            return fail("bad_plano", format!("a tarefa {} aparece duas vezes", t.id));
        }
        if t.titulo.trim().is_empty() || !cabe(&t.titulo, 120) || t.titulo.chars().any(char::is_control) {
            return fail("bad_plano", format!("tarefa {}: título de 1 a 120 caracteres, sem quebra de linha", t.id));
        }
        if t.mudanca.trim().is_empty() || !cabe(&t.mudanca, 2000) {
            return fail("bad_plano", format!("tarefa {}: a mudança é de 1 a 2000 caracteres", t.id));
        }
        if !matches!(t.tamanho.as_str(), "P" | "M" | "G") {
            return fail("bad_plano", format!("tarefa {}: tamanho é P, M ou G", t.id));
        }
        if t.aceite.is_empty() || t.aceite.len() > 20 {
            return fail("bad_plano", format!("tarefa {}: de 1 a 20 critérios de aceite", t.id));
        }
        if t.aceite.iter().any(|a| a.trim().is_empty() || !cabe(a, 300) || a.contains(['\0', '\n', '\r'])) {
            return fail("bad_plano", format!("tarefa {}: critério de aceite inválido (uma linha, até 300 caracteres)", t.id));
        }
        if let Some(a) = t.aceite.iter().find(|a| tem_metacaractere(a)) {
            return fail("bad_plano", format!("tarefa {}: o critério «{a}» tem caractere de shell (& | < > ^ % \" ; ( ) ! `); reescreva sem ele", t.id));
        }
        if t.arquivos.len() > 50 {
            return fail("bad_plano", format!("tarefa {}: até 50 arquivos", t.id));
        }
        t.arquivos.iter().try_for_each(|f| caminho_valido(f))?;
    }
    for t in &p.tarefas {
        for d in &t.depende {
            if d == &t.id {
                return fail("bad_plano", format!("a tarefa {} depende dela mesma", t.id));
            }
            if !ids.contains(d.as_str()) {
                return fail("bad_plano", format!("Tarefa {} depende de {d}, que não existe", t.id));
            }
        }
    }
    if secao_comandos(&p).chars().count() > RESUMO_GATE_MAX - RESERVA_RODAPE {
        return fail("bad_plano", "os comandos de aceite não cabem no resumo do gate (4000 caracteres): encurte os critérios ou divida o plano");
    }
    let mut ordem = Vec::with_capacity(p.tarefas.len());
    let mut feitas: HashSet<String> = HashSet::new();
    let mut restantes = std::mem::take(&mut p.tarefas);
    while !restantes.is_empty() {
        let Some(i) = restantes.iter().position(|t| t.depende.iter().all(|d| feitas.contains(d))) else {
            let presas: Vec<&str> = restantes.iter().map(|t| t.id.as_str()).collect();
            return fail("bad_plano", format!("dependências circulares entre: {}", presas.join(", ")));
        };
        let t = restantes.remove(i);
        feitas.insert(t.id.clone());
        ordem.push(t);
    }
    p.tarefas = ordem;
    Ok(p)
}

/// Caracteres de shell que um critério de aceite não pode ter: o comando vai para o `cmd.exe`.
const METACARACTERES: [char; 13] = ['&', '|', '<', '>', '^', '%', '"', ';', '(', ')', '!', '`', '\u{0}'];
/// Espaço do resumo do gate reservado ao rodapé (o caminho do artefato).
const RESERVA_RODAPE: usize = 150;

fn tem_metacaractere(a: &str) -> bool {
    a.chars().any(|c| METACARACTERES.contains(&c) || c == '\n' || c == '\r')
}

/// Seção do gate com todo critério de aceite, verbatim, com a tarefa. Os comandos nunca são cortados.
fn secao_comandos(p: &Plano) -> String {
    let mut s = String::from("## Comandos que o Regente vai rodar\n");
    for t in &p.tarefas {
        for a in &t.aceite {
            s += &format!("- {}: `{}`\n", t.id, a.trim());
        }
    }
    s
}

/// Resumo do gate: a seção de comandos primeiro, depois o começo do plano até o teto (o que sobrar), e o rodapé.
pub fn resumo_gate(p: &Plano, md: &str, missao: &str) -> String {
    let secao = secao_comandos(p);
    let rodape = format!("\n\nArtefato: .regente/artefatos/{missao}/plano.md");
    let resto = RESUMO_GATE_MAX.saturating_sub(secao.chars().count() + rodape.chars().count() + 1);
    format!("{secao}\n{}{rodape}", md.chars().take(resto).collect::<String>())
}

/// O plano em Markdown (o que o usuário lê no gate e no `plano.md`).
pub fn render_md(p: &Plano, versao: u64) -> String {
    let mut s = format!("# Plano (versão {versao})\n\n## Objetivo\n\n{}\n\n## Decisões\n\n", p.objetivo.trim());
    if p.decisoes.is_empty() {
        s += "- (nenhuma)\n";
    }
    for d in &p.decisoes {
        s += &format!("- {}\n", d.trim());
    }
    s += "\n## Tarefas\n\n| id | tarefa | tamanho | depende |\n|---|---|---|---|\n";
    for t in &p.tarefas {
        let deps = if t.depende.is_empty() { "—".to_string() } else { t.depende.join(", ") };
        s += &format!("| {} | {} | {} | {} |\n", t.id, t.titulo.replace('|', "/"), t.tamanho, deps);
    }
    for t in &p.tarefas {
        let arquivos = if t.arquivos.is_empty() { "—".to_string() } else { t.arquivos.join(", ") };
        s += &format!("\n### {} · {}\n\n**Arquivos:** {arquivos}\n\n**Mudança:** {}\n\n**Aceite:**\n", t.id, t.titulo, t.mudanca.trim());
        for a in &t.aceite {
            s += &format!("- `{}`\n", a.trim());
        }
    }
    if let Some(g) = p.glossario.as_deref().filter(|g| !g.trim().is_empty()) {
        s += &format!("\n## Glossário\n\n{}\n", g.trim());
    }
    s
}

/// O prompt do executor: só a fatia da tarefa, o glossário e as regras (quando `disciplina`). Nada mais do plano.
pub fn prompt_executor(plano: &Plano, t: &Tarefa, disciplina: bool) -> String {
    let mut p = format!("# Tarefa {} · {}\n\n## Mudança\n{}\n\n## Arquivos\n", t.id, t.titulo, t.mudanca.trim());
    if t.arquivos.is_empty() {
        p += "- (nenhum listado: crie só o que a tarefa pede)\n";
    }
    for f in &t.arquivos {
        p += &format!("- {f}\n");
    }
    p += "\n## Aceite\n";
    for a in &t.aceite {
        p += &format!("- `{}`\n", a.trim());
    }
    if let Some(g) = plano.glossario.as_deref().filter(|g| !g.trim().is_empty()) {
        p += &format!("\n## Glossário\n{}\n", g.trim());
    }
    if disciplina {
        p += "\n## Regras\n- Não explore além dos arquivos listados; leia só o trecho necessário.\n- Faça a menor mudança que cumpre a tarefa. Sem refatorar.\n- Rode só os testes desta tarefa (os critérios de aceite).\n- Se faltar contexto, pergunte com handoff_submit (data: {\"ok\": false}, summary: a pergunta) em vez de explorar.\n- Ao terminar, entregue com handoff_submit no esquema {changed[], tests, notes}.\n";
    }
    p
}

/// Avisa a tela da missão que as tarefas mudaram (o evento `tasks-changed`).
fn avisar(s: &AppState, missao: &str) {
    s.emit("tasks-changed", json!({"missionId": missao}));
}

/// Id da tarefa na tabela `tasks`.
fn tid(missao: &str, plano_id: &str) -> String {
    format!("{missao}-{plano_id}")
}

/// Id do plano (`T3`) a partir do id da tabela.
fn do_plano<'a>(missao: &str, id: &'a str) -> &'a str {
    id.strip_prefix(&format!("{missao}-")).unwrap_or(id)
}

fn pasta(cwd: &Path, missao: &str) -> PathBuf {
    cwd.join(".regente").join("artefatos").join(missao)
}

fn ler(cwd: &Path, missao: &str) -> AppResult<Guardado> {
    let txt = std::fs::read_to_string(pasta(cwd, missao).join("plano.json"))?;
    Ok(serde_json::from_str(&txt)?)
}

/// Trava global do despacho: ler as tarefas, escolher e reivindicar acontecem dentro dela. Abrir ou fechar painel
/// fica de fora: a trava nunca segura uma chamada que pode reentrar no despacho.
// ponytail: trava global; por missão se o throughput importar
static DESPACHO: Mutex<()> = Mutex::new(());
/// Dono provisório de uma tarefa reivindicada, antes do executor existir.
const RESERVADA: &str = "pendente";

/// `aprovacao.json`: a versão aprovada, os gates do plano (id e a versão que mostram) e quando o plano fechou.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Aprovacao {
    #[serde(default)]
    pub aprovada: Option<u64>,
    /// Cópia do plano aprovado (versão, planejador, tarefas e aceite). Só ela roda aceite e abre executores;
    /// o plano em disco é espelho para leitura.
    #[serde(default)]
    pub aprovado: Option<Guardado>,
    #[serde(default)]
    pub gates: Vec<GatePlano>,
    #[serde(default)]
    pub concluida_em: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePlano {
    pub id: String,
    pub versao: u64,
    /// O plano exato que o gate mostrou. Aprovar copia ESTA cópia, nunca o `plano.json` do disco (editável à mão).
    #[serde(default)]
    pub plano: Option<Guardado>,
}

/// Texto colado no planejador quando o usuário pede ajuste no gate.
fn ajuste_texto(nota: Option<&str>) -> String {
    format!("Ajuste pedido no plano: {}. Reenvie com plan_submit.", nota.unwrap_or("sem detalhes"))
}

/// Pergunta de executor, colada no planejador quando o handoff não chegou a ele.
fn pergunta_texto(tarefa: &str, resumo: &str) -> String {
    format!("Executor {tarefa} pergunta: {resumo}. Responda com plan_reply task={tarefa}.")
}

/// Entrega de texto a um painel. Nos testes, guarda o que sairia (não há processo).
fn entregar(s: &AppState, pane: &str, data: &[u8]) -> AppResult<()> {
    #[cfg(test)]
    return {
        let _ = s;
        ENTREGUES.with(|e| e.borrow_mut().push((pane.to_string(), data.to_vec())));
        Ok(())
    };
    #[cfg(not(test))]
    s.write_input(pane, data)
}

#[cfg(test)]
thread_local! {
    static ENTREGUES: std::cell::RefCell<Vec<(String, Vec<u8>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Pasta de trabalho da missão de plano: a versão mais nova e o gate de cada versão ficam no banco
/// (`plan_json`, à prova de edição à mão). `aprovacao.json` é só espelho legível.
fn ler_aprovacao(s: &AppState, missao: &str) -> Aprovacao {
    s.db.get_mission(missao).ok().and_then(|m| m.plan_json).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Planner: se o plano da missão está aprovado, quantas tarefas ainda estão `pendente` (None = sem plano aprovado).
pub fn aprovado_pendentes(s: &AppState, missao: &str) -> Option<usize> {
    aprovado(s, missao)?;
    Some(s.db.tasks(missao).ok()?.iter().filter(|t| t.status == "pendente").count())
}

/// O plano aprovado da missão, da cópia do banco, só se a aprovação ainda aponta para ele.
fn aprovado(s: &AppState, missao: &str) -> Option<Guardado> {
    let a = ler_aprovacao(s, missao);
    let g = a.aprovado?;
    (a.aprovada == Some(g.versao)).then_some(g)
}

fn gravar_aprovacao(s: &AppState, cwd: &Path, missao: &str, a: &Aprovacao) -> AppResult<()> {
    let texto = serde_json::to_string_pretty(a)?;
    s.db.set_mission_plan(missao, &texto)?;
    artefatos::write(cwd, missao, "aprovacao", &texto, "json")?;
    Ok(())
}

/// Próxima versão: nunca recomeça em 1, mesmo com `plano.json` ausente ou corrompido.
fn proxima_versao(s: &AppState, cwd: &Path, missao: &str) -> u64 {
    let a = ler_aprovacao(s, missao);
    let do_arquivo = ler(cwd, missao).map_or(0, |g| g.versao);
    let gates = a.gates.iter().map(|g| g.versao).max().unwrap_or(0);
    do_arquivo.max(a.aprovada.unwrap_or(0)).max(gates) + 1
}

/// Missões sendo arquivadas: o despacho não abre nada para elas (checado antes de fechar os painéis).
static ARQUIVANDO: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Marca a missão como em arquivamento até o guarda sair de escopo.
pub struct Arquivando(String);

impl Arquivando {
    pub fn new(missao: &str) -> Self {
        lk(&ARQUIVANDO).push(missao.to_string());
        Self(missao.to_string())
    }
}

impl Drop for Arquivando {
    fn drop(&mut self) {
        let mut v = lk(&ARQUIVANDO);
        if let Some(i) = v.iter().position(|m| *m == self.0) {
            v.remove(i);
        }
    }
}

pub(crate) fn arquivando(missao: &str) -> bool {
    lk(&ARQUIVANDO).iter().any(|m| m == missao)
}

/// `plan_submit`: valida, grava `plano.md`/`plano.json`, põe as tarefas como `pendente` e abre o gate do plano.
/// Só o planejador de uma missão de plano entrega; uma nova versão troca as tarefas `pendente` que saíram do plano.
pub fn enviar(s: &Arc<AppState>, me: &PaneRow, plano: Plano) -> AppResult<Value> {
    if me.role.as_deref() != Some("planner") {
        return fail("not_planner", "só o planejador da missão entrega o plano");
    }
    not_archived(s, me)?;
    let missao = own_mission(me)?;
    if s.db.get_mission(&missao)?.style.as_deref() != Some("plano") {
        return fail("not_plan_mission", "a missão não é do estilo Plano → Execução");
    }
    let plano = validate(plano)?;
    let cwd = PathBuf::from(&me.cwd);
    let (versao, md, guardado) = {
        let _g = lk(&DESPACHO);
        let versao = proxima_versao(s, &cwd, &missao);
        let md = render_md(&plano, versao);
        let guardado = Guardado { versao, planejador: me.id.clone(), plano: plano.clone() };
        artefatos::write(&cwd, &missao, "plano", &md, "md")?;
        artefatos::write(&cwd, &missao, "plano", &serde_json::to_string_pretty(&guardado)?, "json")?;
        let agora = now_ms();
        let existentes: HashMap<String, TaskItem> = s.db.tasks(&missao)?.into_iter().map(|t| (t.id.clone(), t)).collect();
        let novos: HashSet<String> = plano.tarefas.iter().map(|t| tid(&missao, &t.id)).collect();
        for (id, antiga) in &existentes {
            if antiga.status == "pendente" && !novos.contains(id) {
                s.db.upsert_task(&TaskItem { status: "substituida".into(), updated_at: agora, ..antiga.clone() })?;
            }
        }
        for t in &plano.tarefas {
            let id = tid(&missao, &t.id);
            let atual = existentes.get(&id);
            // em execução ou concluída: o novo plano não mexe nela. Pendente ou substituída: (de volta) pendente.
            if atual.is_some_and(|a| !matches!(a.status.as_str(), "pendente" | "substituida")) {
                continue;
            }
            let item = TaskItem {
                id,
                mission_id: missao.clone(),
                title: t.titulo.clone(),
                status: "pendente".into(),
                owner_pane: None,
                created_at: atual.map_or(agora, |a| a.created_at),
                updated_at: agora,
            };
            s.db.upsert_task(&item)?;
        }
        (versao, md, guardado)
    };
    avisar(s, &missao);
    let resumo = resumo_gate(&plano, &md, &missao);
    // gate e registro na mesma seção: `on_gate` (que pega a mesma trava) nunca vê um gate sem registro
    let _g = lk(&DESPACHO);
    // o gate é só para o usuário: a decisão chega por `gate_answer` -> `on_gate`, não por um ticket esperado aqui
    let gate = squads::open_gate(s, me, GATE_PLANO.into(), resumo, vec!["Aprovar".into(), "Ajustar".into()])?;
    let gate_id = gate.id.clone();
    drop(gate);
    let mut a = ler_aprovacao(s, &missao);
    a.gates.push(GatePlano { id: gate_id, versao, plano: Some(guardado) });
    a.concluida_em = None;
    gravar_aprovacao(s, &cwd, &missao, &a)?;
    drop(_g);
    Ok(json!({"versao": versao, "tarefas": plano.tarefas.len(), "plano": format!(".regente/artefatos/{missao}/plano.md")}))
}

/// Decisão de um gate (chamado por `gate_answer`). Só age nos gates do plano (os que `enviar` abriu, listados em
/// `aprovacao.json`). Gate de versão antiga: avisa e para. "Aprovar" grava a versão e despacha; "Ajustar" manda a nota
/// de volta ao planejador, que reenvia com `plan_submit`.
pub fn on_gate(s: &Arc<AppState>, id: &str, decisao: &str, nota: Option<&str>) {
    let Ok(g) = s.db.get_gate(id) else { return };
    if g.title != GATE_PLANO {
        return;
    }
    let (Some(missao), Ok(planejador)) = (g.mission_id.clone(), s.db.get_pane(&g.pane_id)) else { return };
    if planejador.role.as_deref() != Some("planner") || planejador.mission_id.as_deref() != Some(missao.as_str()) {
        return;
    }
    let cwd = PathBuf::from(&planejador.cwd);
    // None: gate de plano sem a cópia mostrada (criado antes da correção); Some(substituido) nos demais casos
    let decidido = 'decide: {
        let _g = lk(&DESPACHO);
        let mut a = ler_aprovacao(s, &missao);
        let Some(gate) = a.gates.iter().find(|x| x.id == g.id).cloned() else { return };
        // o que o gate mostrou (cópia do banco), validado de novo; nada vem do disco
        let Some(mostrado) = gate.plano.clone() else { break 'decide None };
        let Ok(plano) = validate(mostrado.plano.clone()) else { return };
        let guardado = Guardado { plano, ..mostrado };
        // versão mais nova no banco: este gate foi substituído
        if a.gates.iter().any(|x| x.versao > gate.versao) {
            Some(true)
        } else {
            if decisao == "Aprovar" {
                a.aprovada = Some(gate.versao);
                a.aprovado = Some(guardado);
                if let Err(e) = gravar_aprovacao(s, &cwd, &missao, &a) {
                    tracing::warn!("aprovação do plano da missão {missao} não gravada: {e}");
                    return;
                }
            }
            Some(false)
        }
    };
    let Some(substituido) = decidido else {
        if decisao == "Aprovar" {
            s.emit("exec-aviso", json!({"text": "Este plano precisa ser reenviado pelo planejador.", "nivel": "info"}));
        }
        return;
    };
    if substituido {
        s.emit("exec-aviso", json!({"text": "Este plano foi substituído por uma versão mais nova; aprove a versão atual.", "nivel": "info"}));
        return;
    }
    if decisao == "Aprovar" {
        crate::fases::fechar(s, &missao, None, crate::fases::PLANEJAMENTO);
    }
    match decisao {
        "Aprovar" => {
            if let Err(e) = despachar(s, &missao, &planejador) {
                tracing::warn!("plano da missão {missao} não despachado: {e}");
            }
        }
        "Ajustar" => {
            if let Err(e) = entregar(s, &planejador.id, &bracketed(&ajuste_texto(nota))) {
                tracing::warn!("ajuste do plano não entregue ao planejador: {e}");
            }
        }
        _ => {}
    }
}

/// Tarefas a abrir agora: pendentes com as dependências concluídas, na ordem do plano, no máximo `vagas`.
pub fn escolher(ordem: &[(String, Vec<String>)], status: &HashMap<String, String>, vagas: usize) -> Vec<String> {
    let concluida = |id: &String| status.get(id).is_some_and(|s| s == "concluida");
    ordem
        .iter()
        .filter(|(id, deps)| status.get(id).is_some_and(|s| s == "pendente") && deps.iter().all(concluida))
        .take(vagas)
        .map(|(id, _)| id.clone())
        .collect()
}

/// Dentro da trava: confere a aprovação, reivindica as tarefas prontas (`em_execucao`, dono provisório) e marca o
/// fechamento do plano, se esta chamada é a que o vê. Sem plano aprovado nesta versão: `None`.
fn reivindicar(s: &AppState, missao: &str, cwd: &Path, paralelo_max: usize) -> AppResult<Option<(Plano, Vec<TaskItem>, bool)>> {
    let _g = lk(&DESPACHO);
    let Some(g) = aprovado(s, missao) else { return Ok(None) };
    let mut a = ler_aprovacao(s, missao);
    let tarefas = s.db.tasks(missao)?;
    let status: HashMap<String, String> = tarefas.iter().map(|t| (t.id.clone(), t.status.clone())).collect();
    let rodando = tarefas.iter().filter(|t| t.status == "em_execucao").count();
    let ordem: Vec<(String, Vec<String>)> =
        g.plano.tarefas.iter().map(|t| (tid(missao, &t.id), t.depende.iter().map(|d| tid(missao, d)).collect())).collect();
    let agora = now_ms();
    let mut reservadas: Vec<TaskItem> = vec![];
    for id in escolher(&ordem, &status, paralelo_max.saturating_sub(rodando)) {
        let Some(t) = tarefas.iter().find(|x| x.id == id) else { continue };
        let r = TaskItem { status: "em_execucao".into(), owner_pane: Some(RESERVADA.into()), updated_at: agora, ..t.clone() };
        if let Err(e) = s.db.upsert_task(&r) {
            reverter(s, &reservadas, agora);
            return Err(e);
        }
        reservadas.push(r);
    }
    let feitas = g.plano.tarefas.iter().all(|t| status.get(&tid(missao, &t.id)).is_some_and(|x| x == "concluida"));
    let fechou = feitas && a.concluida_em.is_none();
    if fechou {
        a.concluida_em = Some(agora);
        if let Err(e) = gravar_aprovacao(s, cwd, missao, &a) {
            reverter(s, &reservadas, agora);
            return Err(e);
        }
    }
    Ok(Some((g.plano, reservadas, fechou)))
}

/// Desfaz reservas feitas numa chamada que falhou no meio: a tarefa volta a `pendente`, sem dono.
/// Dentro da trava do despacho (quem chama segura `DESPACHO`).
fn reverter(s: &AppState, reservadas: &[TaskItem], agora: i64) {
    for r in reservadas {
        if let Err(e) = s.db.upsert_task(&TaskItem { status: "pendente".into(), owner_pane: None, updated_at: agora, ..r.clone() }) {
            tracing::warn!("reserva da tarefa {} não desfeita: {e}", r.id);
        }
    }
}

/// Acha a tarefa que este painel executor toca: pelo dono; se o dono ainda não foi gravado (handoff antes do
/// despacho gravar o dono), pela reserva cujo id casa com o nome do painel (`T3 · título`). Dentro da trava.
fn acha_tarefa(tarefas: &[TaskItem], missao: &str, painel: &PaneRow) -> Option<TaskItem> {
    let rodando = |t: &&TaskItem| matches!(t.status.as_str(), "em_execucao" | "verificando");
    if let Some(t) = tarefas.iter().filter(rodando).find(|t| t.owner_pane.as_deref() == Some(painel.id.as_str())) {
        return Some(t.clone());
    }
    tarefas
        .iter()
        .filter(rodando)
        .filter(|t| t.owner_pane.as_deref() == Some(RESERVADA))
        .find(|t| painel.name.starts_with(&format!("{} · ", do_plano(missao, &t.id))))
        .cloned()
}

/// Grava o dono de uma reserva, só se a tarefa ainda está reservada (um handoff rápido pode já tê-la concluído).
/// Tenta duas vezes; sem sucesso, a reserva fica com o dono provisório e o nome do painel a identifica.
fn fixar_dono(s: &AppState, missao: &str, tarefa: &str, painel: &str) {
    for _ in 0..2 {
        let _g = lk(&DESPACHO);
        let atual = s.db.tasks(missao).ok().and_then(|ts| ts.into_iter().find(|t| t.id == tarefa));
        match atual {
            Some(t) if t.status == "em_execucao" && t.owner_pane.as_deref() == Some(RESERVADA) => {
                if s.db.upsert_task(&TaskItem { owner_pane: Some(painel.into()), updated_at: now_ms(), ..t }).is_ok() {
                    return;
                }
            }
            _ => return,
        }
    }
    tracing::warn!("dono da tarefa {tarefa} não gravado; a reserva segue identificada pelo nome do painel");
}

/// Abre executores para as tarefas prontas, até `perfil.paraleloMax` em curso e dentro do limite de painéis.
/// Só age com o plano aprovado. Ao concluir o plano, avisa o usuário uma vez só. Devolve quantos executores abriu.
pub fn despachar(s: &AppState, missao: &str, planejador: &PaneRow) -> AppResult<usize> {
    // planejador fechando/fechado, missão arquivada ou em arquivamento: nenhum executor novo
    let planejador_saiu = planejador.closed_at.is_some() || s.status_of(&planejador.id) == Some(PaneStatus::Closing);
    let arquivada = s.db.get_mission(missao).map_or(true, |m| m.archived_at.is_some());
    if planejador_saiu || arquivada || arquivando(missao) || not_archived(s, planejador).is_err() {
        return Ok(0);
    }
    let (perfil, _) = perfil::efetivo_de(s, &planejador.workspace_id, planejador.profile.as_deref());
    let Some((plano, reservadas, fechou)) = reivindicar(s, missao, Path::new(&planejador.cwd), perfil.paralelo_max as usize)? else {
        return Ok(0);
    };
    let slot = perfil::slot_of(&perfil, "implementar");
    let mut abertos = 0;
    for (i, r) in reservadas.iter().enumerate() {
        let plano_id = do_plano(missao, &r.id);
        let Some(t) = plano.tarefas.iter().find(|t| t.id == plano_id) else { continue };
        let spec = PaneSpec {
            workspace_id: planejador.workspace_id.clone(),
            mission_id: Some(missao.into()),
            kind: PaneKind::Agent,
            name: Some(format!("{} · {}", t.id, t.titulo)),
            cli: Some("claude".into()),
            model: slot.map(|x| x.model.clone()),
            effort: slot.map(|x| x.effort.clone()),
            permission: Permission::Auto,
            cwd: planejador.cwd.clone(),
            prompt: Some(prompt_executor(&plano, t, perfil.disciplina_executor)),
            role: Some("executor".into()),
            parent_id: Some(planejador.id.clone()),
            mcp_profile: Some("minimal".into()),
            ..Default::default()
        };
        match check_spawn_budget(s, planejador, 1).and_then(|()| s.spawn_pane(spec)) {
            Ok(row) => {
                crate::fases::abrir(s, missao, Some(&row.id), Some(&r.id), crate::fases::EXECUCAO);
                fixar_dono(s, missao, &r.id, &row.id);
                abertos += 1;
            }
            Err(e) => {
                let motivo = e.to_string();
                {
                    let _g = lk(&DESPACHO);
                    let agora = now_ms();
                    // a que falhou fica bloqueada ("Retomar plano" a reabre); as reservadas depois dela voltam a pendente
                    s.db.upsert_task(&TaskItem { status: "bloqueada".into(), owner_pane: None, updated_at: agora, ..r.clone() })?;
                    for resto in &reservadas[i + 1..] {
                        s.db.upsert_task(&TaskItem { status: "pendente".into(), owner_pane: None, updated_at: agora, ..resto.clone() })?;
                    }
                }
                tracing::warn!("plano da missão {missao} parado: {motivo}");
                s.emit("exec-aviso", json!({"text": format!("Plano parado: {motivo}"), "nivel": "erro"}));
                s.notify_pane(&planejador.id, crate::notify::Kind::Asks, "Plano parado", &motivo);
                break;
            }
        }
    }
    avisar(s, missao);
    if fechou {
        let nome = s.db.get_mission(missao).map(|m| m.name).unwrap_or_default();
        s.notify_pane(&planejador.id, crate::notify::Kind::Finished, "Plano concluído", &nome);
    }
    Ok(abertos)
}

/// A tarefa que este painel executor tem em curso (dono = o painel), se houver.
fn em_curso(s: &AppState, missao: &str, painel: &PaneRow) -> Option<TaskItem> {
    let _g = lk(&DESPACHO);
    acha_tarefa(&s.db.tasks(missao).ok()?, missao, painel)
}

/// Entrega de um executor (`handoff_submit`). `ok: false` = pergunta: vai ao planejador e o executor fica aberto.
/// Entrega normal: a tarefa vira `concluida` e o plano segue. `entregue`: a quem o handoff foi entregue; se foi ao
/// planejador, a pergunta já chegou por ele e não se cola de novo.
pub fn on_handoff(s: &Arc<AppState>, exec: &PaneRow, dados: Option<&Value>, resumo: &str, entregue: Option<&str>) {
    if exec.role.as_deref() != Some("executor") {
        return;
    }
    let (Some(missao), Some(planner_id)) = (exec.mission_id.clone(), exec.parent_id.clone()) else { return };
    if s.db.get_pane(&planner_id).is_err() {
        return;
    }
    if dados.and_then(|d| d.get("ok")).and_then(Value::as_bool) == Some(false) {
        // o executor espera a resposta: não fecha sozinho
        s.rt().close_at.remove(&exec.id);
        let Some(t) = em_curso(s, &missao, exec) else { return };
        if entregue != Some(planner_id.as_str()) {
            if let Err(e) = entregar(s, &planner_id, &bracketed(&pergunta_texto(do_plano(&missao, &t.id), resumo))) {
                tracing::warn!("pergunta do executor não entregue ao planejador: {e}");
            }
        }
        return;
    }
    // a entrega vai para verificação (dono = este executor); a verificação decide: passou -> concluída; falhou ->
    // o mesmo executor corrige, sobe um degrau, ou o planejador decide. Roda fora da trava (leva minutos).
    let marcada = {
        let _g = lk(&DESPACHO);
        match s.db.tasks(&missao).ok().and_then(|ts| acha_tarefa(&ts, &missao, exec)) {
            // já em verificação: a entrega repetida não abre a segunda thread
            Some(t) if t.status == "verificando" => None,
            Some(t) => {
                let id = t.id.clone();
                match s.db.upsert_task(&TaskItem { status: "verificando".into(), owner_pane: Some(exec.id.clone()), updated_at: now_ms(), ..t }) {
                    Ok(()) => {
                        lk(&VERIFICANDO).insert(id.clone());
                        Some(id)
                    }
                    Err(e) => {
                        tracing::warn!("entrega não marcada para verificação: {e}");
                        None
                    }
                }
            }
            None => None,
        }
    };
    let Some(tarefa) = marcada else { return };
    crate::fases::fechar(s, &missao, Some(&tarefa), crate::fases::EXECUCAO);
    crate::fases::fechar(s, &missao, Some(&tarefa), crate::fases::CORRECOES);
    avisar(s, &missao);
    let (alvo, exec_id) = (s.clone(), exec.id.clone());
    let tarefa_thread = tarefa.clone();
    let spawned = std::thread::Builder::new().name("regente-verificacao-entrega".into()).spawn(move || {
        // o guarda tira a tarefa de VERIFICANDO também se a thread entrar em pânico
        let _vg = Verificando(tarefa_thread.clone());
        if let Err(e) = verificar_entrega(&alvo, &missao, &tarefa_thread, &exec_id, &planner_id) {
            tracing::warn!("verificação da entrega da missão {missao} não concluiu: {e}");
            // sem isto a tarefa ficaria `verificando` para sempre: conta como falha da verificação
            if let Err(e2) = falhou(&alvo, &missao, &tarefa_thread, &exec_id, &planner_id, &format!("a verificação não rodou: {e}")) {
                tracing::warn!("falha da verificação não registrada: {e2}");
            }
        }
    });
    if let Err(e) = spawned {
        lk(&VERIFICANDO).remove(&tarefa);
        tracing::warn!("thread de verificação não criada: {e}");
    }
}

/// Tarefas com uma thread de verificação viva (a entrega ainda está sendo checada). `retomar` não mexe nelas.
static VERIFICANDO: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Tira a tarefa de `VERIFICANDO` ao sair de escopo (fim normal ou pânico da thread de verificação).
struct Verificando(String);

impl Drop for Verificando {
    fn drop(&mut self) {
        lk(&VERIFICANDO).remove(&self.0);
    }
}

/// Falhas de verificação seguidas por tarefa (na memória: zera quando a tarefa passa).
static FALHAS: LazyLock<Mutex<HashMap<String, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Passo depois de falhas da mesma tarefa: o executor corrige, sobe um degrau ou o planejador decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Passo {
    Mesmo,
    Escalar,
    Planejador,
}

/// Escada de falhas (X1b). `falhas` conta a atual: 1 → o mesmo executor corrige; 2 → sobe um degrau (se `escalar`),
/// senão o mesmo; 3 ou mais → o planejador. Não há laço além do planejador.
pub fn proximo_passo(falhas: u32, escalar: bool) -> Passo {
    match falhas {
        0 | 1 => Passo::Mesmo,
        2 if escalar => Passo::Escalar,
        2 => Passo::Mesmo,
        _ => Passo::Planejador,
    }
}

/// Critério de aceite que a verificação roda como comando: começa por uma ferramenta conhecida, numa linha só.
/// Critério em prosa fica de fora (o executor já o cumpriu).
fn eh_comando(aceite: &str) -> bool {
    // ponytail: lista fechada de programas E subcomandos de teste/build. Sem node/python/make/bun/yarn e sem
    // `npm exec`, `pnpm dlx`, `cargo run/install`, `go run`, `dotnet run`: quem roda script ou código arbitrário fica de fora.
    const NPX_OK: [&str; 5] = ["vitest", "jest", "tsc", "eslint", "playwright"];
    let t = aceite.trim();
    if tem_metacaractere(t) || verificacao::check_cmd(t).is_err() {
        return false;
    }
    let p: Vec<&str> = t.split_whitespace().collect();
    let Some(&prog) = p.first() else { return false };
    // flags globais que trocam a config do build (`--config`, `-Z`-style) nunca passam
    if p.iter().any(|x| *x == "--config" || x.starts_with("--config=")) {
        return false;
    }
    // flags que trocam o programa de build ou carregam código (plugin, shell, exec), em qualquer posição
    const FLAGS_PERIGOSAS: [&str; 7] = ["-exec", "-toolexec", "-vettool", "--script-shell", "--shell", "--plugin", "--exec"];
    if p.iter().any(|x| FLAGS_PERIGOSAS.contains(&x.split('=').next().unwrap_or(""))) {
        return false;
    }
    let sub = |i: usize| p.get(i).copied().unwrap_or("");
    // script de teste do gerenciador: `test`, `run test`, `run test:*` (nunca `exec`/`dlx`/`run <outro>`)
    let script = |i: usize| {
        let s = sub(i);
        s == "test" || s.starts_with("test:") || (s == "run" && (sub(i + 1) == "test" || sub(i + 1).starts_with("test:")))
    };
    match prog {
        "npx" => {
            let ferramenta_ok = NPX_OK.contains(&sub(1));
            ferramenta_ok && !p.iter().any(|x| matches!(*x, "--yes" | "-y" | "-p") || x.starts_with("--package") || x.starts_with("-p="))
        }
        "cargo" => matches!(sub(1), "test" | "check" | "clippy" | "build" | "nextest"),
        "npm" | "pnpm" => script(1),
        "go" => matches!(sub(1), "test" | "vet"),
        "dotnet" => matches!(sub(1), "test" | "build"),
        "pytest" | "vitest" | "jest" | "tsc" | "eslint" => true,
        _ => false,
    }
}

/// Verificação de uma entrega (a tarefa está em `verificando`): testes relacionados à mudança e os comandos de
/// aceite. Passou: conclui. Falhou: passo da escada. Roda numa thread própria, fora da trava do despacho.
fn verificar_entrega(s: &AppState, missao: &str, tarefa: &str, exec: &str, planejador: &str) -> AppResult<()> {
    let comandos = comandos_da_entrega(s, missao, tarefa)?;
    if comandos.is_empty() {
        return concluir(s, missao, tarefa, exec, planejador);
    }
    let limite = VerifyCfg::default().timeout_min;
    match verificacao::rodar_esperando(s, missao, &comandos, limite, "entrega") {
        Ok(r) if r.ok => concluir(s, missao, tarefa, exec, planejador),
        // cancelada (pelo usuário ou pelo arquivamento): não é falha, não escala; a tarefa volta a em_execucao
        Ok(r) if r.cancelado => devolver_cancelada(s, missao, tarefa, exec),
        Ok(_) if arquivando(missao) => Ok(()),
        Ok(r) => falhou(s, missao, tarefa, exec, planejador, &r.digest),
        Err(e) if e.code() == "arquivando" => Ok(()),
        Err(e) => falhou(s, missao, tarefa, exec, planejador, &format!("a verificação não rodou: {e}")),
    }
}

/// Comandos que verificam uma entrega: os relacionados à mudança e os aceites que são comandos, do plano APROVADO
/// (a cópia do banco). Sem plano aprovado nesta versão, erro: nenhum aceite roda.
fn comandos_da_entrega(s: &AppState, missao: &str, tarefa: &str) -> AppResult<Vec<String>> {
    let Some(plano) = aprovado(s, missao).map(|g| g.plano) else {
        return fail("sem_plano_aprovado", "o plano desta missão não está aprovado na versão atual; nenhum aceite roda");
    };
    let Some(t) = plano.tarefas.iter().find(|t| tid(missao, &t.id) == tarefa) else { return Ok(vec![]) };
    let cwd = cwd_da_missao(s, missao)?;
    let (_, mudados) = verificacao::mudancas(&cwd);
    let mut comandos = verificacao::comandos(&cwd, &VerifyCfg::default(), None, &mudados, false, true).map(|r| r.comandos).unwrap_or_default();
    for a in t.aceite.iter().filter(|a| eh_comando(a)) {
        if !comandos.contains(&a.trim().to_string()) {
            comandos.push(a.trim().to_string());
        }
    }
    Ok(comandos)
}

/// Passou: a tarefa vira `concluida` e o executor fecha (fora da fila, pois o dono já não é ele). Com o plano inteiro
/// pronto, a suíte completa roda uma vez (`testes.completoNoFim`).
fn concluir(s: &AppState, missao: &str, tarefa: &str, exec: &str, planejador: &str) -> AppResult<()> {
    let feita = {
        let _g = lk(&DESPACHO);
        match s.db.tasks(missao)?.into_iter().find(|t| t.id == tarefa && t.status == "verificando" && t.owner_pane.as_deref() == Some(exec)) {
            Some(t) => {
                s.db.upsert_task(&TaskItem { status: "concluida".into(), updated_at: now_ms(), ..t })?;
                true
            }
            None => false,
        }
    };
    if !feita {
        return Ok(());
    }
    lk(&FALHAS).remove(tarefa);
    avisar(s, missao);
    let _ = s.close_pane(exec);
    let planejador_row = s.db.get_pane(planejador)?;
    despachar(s, missao, &planejador_row)?;
    let pendentes = s.db.tasks(missao)?.iter().filter(|t| !matches!(t.status.as_str(), "concluida" | "substituida")).count();
    let (perfil, _) = perfil::efetivo_de(s, &planejador_row.workspace_id, planejador_row.profile.as_deref());
    if pendentes == 0 && perfil.testes.completo_no_fim {
        let cfg = VerifyCfg::parse(s.db.get_mission(missao)?.verify_json.as_deref());
        let cwd = cwd_da_missao(s, missao)?;
        let ws = s.db.get_workspace(&planejador_row.workspace_id)?;
        let root = Path::new(&ws.path);
        let projeto = if verificacao::confiado(s, &ws.id, root) { crate::projeto::load(root).ok().flatten() } else { None };
        if let Some(r) = verificacao::comandos(&cwd, &cfg, projeto.as_ref(), &[], true, false) {
            let _ = verificacao::rodar_esperando(s, missao, &r.comandos, cfg.timeout_min, "completa");
        }
    }
    Ok(())
}

/// Verificação cancelada: a tarefa volta a `em_execucao` (mesmo dono) para a entrega ser refeita. Missão arquivando:
/// nada muda. Não conta falha.
fn devolver_cancelada(s: &AppState, missao: &str, tarefa: &str, exec: &str) -> AppResult<()> {
    if arquivando(missao) {
        return Ok(());
    }
    let _g = lk(&DESPACHO);
    if let Some(t) = s.db.tasks(missao)?.into_iter().find(|t| t.id == tarefa && t.status == "verificando" && t.owner_pane.as_deref() == Some(exec)) {
        s.db.upsert_task(&TaskItem { status: "em_execucao".into(), updated_at: now_ms(), ..t })?;
    }
    drop(_g);
    avisar(s, missao);
    Ok(())
}

/// Falhou: o passo da escada. Só age se a tarefa ainda está em verificação com este executor. Missão arquivando: não age.
fn falhou(s: &AppState, missao: &str, tarefa: &str, exec: &str, planejador: &str, digest: &str) -> AppResult<()> {
    if arquivando(missao) {
        return Ok(());
    }
    let atual = {
        let _g = lk(&DESPACHO);
        s.db.tasks(missao)?.into_iter().find(|t| t.id == tarefa && t.status == "verificando" && t.owner_pane.as_deref() == Some(exec))
    };
    let Some(t) = atual else { return Ok(()) };
    let falhas = {
        let mut f = lk(&FALHAS);
        let n = f.entry(tarefa.to_string()).or_insert(0);
        *n += 1;
        *n
    };
    let planejador_row = s.db.get_pane(planejador)?;
    let (perfil, _) = perfil::efetivo_de(s, &planejador_row.workspace_id, planejador_row.profile.as_deref());
    let mut passo = proximo_passo(falhas, perfil.escalar_ao_falhar);
    if passo == Passo::Mesmo {
        let texto = format!("A verificação falhou: {digest}. Corrija só isso e entregue de novo.");
        {
            let _g = lk(&DESPACHO);
            s.db.upsert_task(&TaskItem { status: "em_execucao".into(), updated_at: now_ms(), ..t.clone() })?;
        }
        match entregar(s, exec, &bracketed(&clean_text(&texto))) {
            Ok(()) => {
                crate::fases::abrir(s, missao, Some(exec), Some(tarefa), crate::fases::CORRECOES);
                avisar(s, missao);
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("falha da verificação não entregue a {exec}: {e}; sobe um degrau");
                passo = Passo::Escalar;
            }
        }
    }
    if passo == Passo::Escalar && escalar(s, missao, &planejador_row, &t, exec, digest, &perfil)? {
        return Ok(());
    }
    // o planejador decide: a tarefa para como bloqueada e o executor que falhou fecha
    {
        let _g = lk(&DESPACHO);
        s.db.upsert_task(&TaskItem { status: "bloqueada".into(), owner_pane: None, updated_at: now_ms(), ..t.clone() })?;
    }
    crate::fases::fechar_tarefa(s, missao, tarefa);
    // bloqueada: a contagem de falhas recomeça quando o planejador desbloquear (`retomar`)
    lk(&FALHAS).remove(tarefa);
    let _ = s.close_pane(exec);
    let texto = format!("{} falhou {falhas} vezes: {digest}. Ajuste a tarefa com plan_submit ou responda com plan_reply.", do_plano(missao, &t.id));
    if let Err(e) = entregar(s, planejador, &bracketed(&clean_text(&texto))) {
        tracing::warn!("falha da tarefa {} não entregue ao planejador: {e}", t.id);
    }
    s.notify_pane(planejador, crate::notify::Kind::Asks, "Tarefa bloqueada na verificação", &t.id);
    avisar(s, missao);
    Ok(())
}

/// Um degrau acima da escada: executor novo com a mesma fatia e a última falha. O dono da tarefa passa a ser o novo
/// antes de o antigo fechar (senão o fechamento devolveria a tarefa à fila). Falso: sem degrau, sem modelo registrado
/// ou sem vaga; quem chama então escala ao planejador.
fn escalar(s: &AppState, missao: &str, planejador: &PaneRow, t: &TaskItem, exec: &str, digest: &str, perfil: &perfil::Perfil) -> AppResult<bool> {
    let antigo = s.db.get_pane(exec)?;
    let (Some(modelo), Some(esforco)) = (antigo.model.clone(), antigo.effort.clone()) else { return Ok(false) };
    let Some((modelo2, esforco2)) = perfil::escalate(perfil, &modelo, &esforco) else { return Ok(false) };
    let Some(plano) = aprovado(s, missao).map(|g| g.plano) else { return Ok(false) };
    let Some(tarefa) = plano.tarefas.iter().find(|x| x.id == do_plano(missao, &t.id)) else { return Ok(false) };
    let prompt = format!("{}\n## Última falha de verificação\n{digest}\n", prompt_executor(&plano, tarefa, perfil.disciplina_executor));
    let spec = PaneSpec {
        workspace_id: planejador.workspace_id.clone(),
        mission_id: Some(missao.into()),
        kind: PaneKind::Agent,
        name: Some(format!("{} · {}", tarefa.id, tarefa.titulo)),
        cli: Some("claude".into()),
        model: Some(modelo2.clone()),
        effort: Some(esforco2.clone()),
        permission: Permission::Auto,
        cwd: planejador.cwd.clone(),
        prompt: Some(prompt),
        role: Some("executor".into()),
        parent_id: Some(planejador.id.clone()),
        mcp_profile: Some("minimal".into()),
        ..Default::default()
    };
    let novo = match check_spawn_budget(s, planejador, 1).and_then(|()| s.spawn_pane(spec)) {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!("escalonamento da tarefa {} sem executor: {e}", t.id);
            return Ok(false);
        }
    };
    {
        let _g = lk(&DESPACHO);
        if let Err(e) = s.db.upsert_task(&TaskItem { status: "em_execucao".into(), owner_pane: Some(novo.id.clone()), updated_at: now_ms(), ..t.clone() }) {
            tracing::warn!("dono escalonado da tarefa {} não gravado: {e}", t.id);
            let _ = s.close_pane(&novo.id);
            return Ok(false);
        }
    }
    let _ = s.db.metric_bump(&novo.id, "escalations");
    let motivo = format!("{} falhou na verificação: escalonado de {modelo}:{esforco} para {modelo2}:{esforco2}", tarefa.id);
    let novo_ajuste = crate::ajustes::Novo {
        workspace_id: Some(planejador.workspace_id.clone()),
        mission_id: Some(missao.into()),
        pane_id: Some(novo.id.clone()),
        kind: "escalonamento",
        acao: "aplicado",
        motivo,
        antes: Some(json!({"modelo": modelo, "esforco": esforco})),
        depois: Some(json!({"modelo": modelo2, "esforco": esforco2})),
        estimativa: None,
        desfazivel: false,
    };
    if let Err(e) = crate::ajustes::registrar(s, novo_ajuste) {
        tracing::warn!("escalonamento não registrado no feed: {e}");
    }
    let _ = s.close_pane(exec);
    avisar(s, missao);
    Ok(true)
}

/// Painel fechou: se era executor de plano com tarefa em curso, a tarefa volta a `pendente` e o plano segue.
/// Para os demais painéis não faz nada. Planejador fechado: a tarefa volta, mas nenhum executor novo abre.
pub fn executor_fechou(s: &AppState, painel: &str) {
    let Ok(row) = s.db.get_pane(painel) else { return };
    let (Some(missao), Some(planner_id)) = (row.mission_id.clone(), row.parent_id.clone()) else { return };
    let voltou = {
        let _g = lk(&DESPACHO);
        let Ok(tarefas) = s.db.tasks(&missao) else { return };
        match acha_tarefa(&tarefas, &missao, &row) {
            Some(t) => {
                let tarefa = t.id.clone();
                if let Err(e) = s.db.upsert_task(&TaskItem { status: "pendente".into(), owner_pane: None, updated_at: now_ms(), ..t }) {
                    tracing::warn!("tarefa do executor {painel} não devolvida à fila: {e}");
                    return;
                }
                // o executor fechou: as fases abertas da tarefa não ficam em curso
                crate::fases::fechar_tarefa(s, &missao, &tarefa);
                true
            }
            None => false,
        }
    };
    if !voltou {
        return;
    }
    avisar(s, &missao);
    let Ok(planejador) = s.db.get_pane(&planner_id) else { return };
    if planejador.closed_at.is_some() {
        return;
    }
    if let Err(e) = despachar(s, &missao, &planejador) {
        tracing::warn!("plano da missão {missao} não avançou: {e}");
    }
}

/// "Retomar plano" (palette e aba de tarefas): tarefas bloqueadas, e as de executor que já não roda, voltam a pendente;
/// o despacho segue (só se o plano estiver aprovado). Devolve quantos executores abriu.
pub fn retomar(s: &AppState, missao: &str) -> AppResult<usize> {
    // o planejador vem da cópia aprovada no banco, nunca do plano em disco
    let Some(planejador_id) = aprovado(s, missao).map(|g| g.planejador) else {
        return fail("sem_plano_aprovado", "o plano desta missão não está aprovado na versão atual");
    };
    let planejador = s.db.get_pane(&planejador_id)?;
    if planejador.closed_at.is_some() {
        return fail("planner_closed", "o planejador da missão está fechado");
    }
    {
        let _g = lk(&DESPACHO);
        let agora = now_ms();
        for t in s.db.tasks(missao)? {
            // reserva em andamento (dono provisório) não é parada: o despacho que a fez ainda abre o executor
            // reserva de mais de 60 s sem executor: o despacho que a fez já morreu
            let reserva_presa = t.owner_pane.as_deref() == Some(RESERVADA) && now_ms() - t.updated_at > 60_000;
            // verificando sem thread viva: a verificação morreu com o processo; a entrega volta à fila
            let verif_presa = t.status == "verificando" && !lk(&VERIFICANDO).contains(&t.id);
            let parada = t.status == "bloqueada"
                || verif_presa
                || (t.status == "em_execucao" && (reserva_presa || (t.owner_pane.as_deref() != Some(RESERVADA) && !t.owner_pane.as_deref().is_some_and(|p| s.panes.is_alive(p)))));
            if parada {
                lk(&FALHAS).remove(&t.id);
                s.db.upsert_task(&TaskItem { status: "pendente".into(), owner_pane: None, updated_at: agora, ..t })?;
            }
        }
    }
    avisar(s, missao);
    despachar(s, missao, &planejador)
}

/// `plan_reply`: a resposta do planejador vai para o executor da tarefa.
pub fn responder(s: &AppState, me: &PaneRow, tarefa: &str, texto: &str) -> AppResult<()> {
    if me.role.as_deref() != Some("planner") {
        return fail("not_planner", "só o planejador da missão responde executores");
    }
    let missao = own_mission(me)?;
    let id = tid(&missao, tarefa);
    let Some(t) = s.db.tasks(&missao)?.into_iter().find(|t| t.id == id) else {
        return fail("bad_task", format!("tarefa {tarefa} não existe nesta missão"));
    };
    let Some(pane) = t.owner_pane.filter(|_| t.status == "em_execucao") else {
        return fail("no_executor", format!("a tarefa {tarefa} não tem executor ativo"));
    };
    crate::mcp::check_texts(&[texto])?;
    entregar(s, &pane, &bracketed(&clean_text(texto)))
}

/// Pasta de trabalho da missão (a do worktree, se tiver; senão a do workspace).
pub(crate) fn cwd_da_missao(s: &AppState, missao: &str) -> AppResult<PathBuf> {
    let m = s.db.get_mission(missao)?;
    let ws = s.db.get_workspace(&m.workspace_id)?;
    Ok(match m.worktree_path.as_deref() {
        Some(wt) => crate::missions::mission_cwd(Path::new(&ws.path), Path::new(wt)),
        None => PathBuf::from(&ws.path),
    })
}

/// `plan_get`: o plano atual (`plano.json`) da missão, para a aba de tarefas. Sem plano: `plano` é null.
pub fn plan_get(s: &AppState, missao: &str) -> AppResult<Value> {
    let cwd = cwd_da_missao(s, missao)?;
    let plano = ler(&cwd, missao).ok();
    let arquivo = plano.as_ref().map(|_| pasta(&cwd, missao).join("plano.md").to_string_lossy().to_string());
    Ok(json!({"plano": plano, "arquivo": arquivo, "tarefas": s.db.tasks(missao)?}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tarefa(id: &str, deps: &[&str]) -> Tarefa {
        Tarefa {
            id: id.into(),
            titulo: format!("tarefa {id}"),
            arquivos: vec![format!("src/{id}.rs")],
            mudanca: "mudar algo".into(),
            aceite: vec!["cargo test x".into()],
            depende: deps.iter().map(|d| d.to_string()).collect(),
            tamanho: "P".into(),
        }
    }

    fn plano(tarefas: Vec<Tarefa>) -> Plano {
        Plano { objetivo: "fazer".into(), decisoes: vec![], tarefas, glossario: None }
    }

    fn erro(p: Plano) -> String {
        validate(p).unwrap_err().to_string()
    }

    #[test]
    fn topological_order_is_stable_and_deps_come_first() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let p = validate(plano(vec![tarefa("C", &["A"]), tarefa("A", &[]), tarefa("B", &[]), tarefa("D", &["C", "B"])])).unwrap();
        let ids: Vec<&str> = p.tarefas.iter().map(|t| t.id.as_str()).collect();
        // A é a primeira pronta; C fica pronta com A e vem antes de B (ordem original)
        assert_eq!(ids, ["A", "C", "B", "D"]);
    }

    #[test]
    fn cycles_duplicates_and_missing_deps_are_refused_in_pt_br() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(erro(plano(vec![tarefa("A", &["B"]), tarefa("B", &["A"])])).contains("circulares"));
        assert!(erro(plano(vec![tarefa("A", &[]), tarefa("A", &[])])).contains("aparece duas vezes"));
        assert!(erro(plano(vec![tarefa("T3", &["T9"])])).contains("Tarefa T3 depende de T9, que não existe"));
        assert!(erro(plano(vec![tarefa("A", &["A"])])).contains("depende dela mesma"));
    }

    #[test]
    fn bad_ids_paths_and_acceptance_are_refused() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(erro(plano(vec![tarefa("a b", &[])])).contains("id de tarefa inválido"));
        assert!(erro(plano(vec![tarefa("abcdefghijklm", &[])])).contains("id de tarefa inválido"));
        for ruim in ["../x.rs", "/etc/x", "C:\\x", "src/../../x", ""] {
            let mut t = tarefa("A", &[]);
            t.arquivos = vec![ruim.into()];
            assert!(erro(plano(vec![t])).contains("arquivo inválido"), "{ruim}");
        }
        let mut t = tarefa("A", &[]);
        t.aceite = vec![];
        assert!(erro(plano(vec![t])).contains("critérios de aceite"));
        let mut t = tarefa("A", &[]);
        t.aceite = vec!["linha\nduas".into()];
        assert!(erro(plano(vec![t])).contains("critério de aceite inválido"));
        let mut t = tarefa("A", &[]);
        t.tamanho = "XL".into();
        assert!(erro(plano(vec![t])).contains("tamanho"));
    }

    #[test]
    fn caps_on_tasks_and_text() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(erro(plano(vec![])).contains("1 a 30 tarefas"));
        let muitas: Vec<Tarefa> = (0..31).map(|i| tarefa(&format!("T{i}"), &[])).collect();
        assert!(erro(plano(muitas)).contains("1 a 30 tarefas"));
        let mut t = tarefa("A", &[]);
        t.mudanca = "x".repeat(2001);
        assert!(erro(plano(vec![t])).contains("mudança"));
    }

    #[test]
    fn executor_prompt_holds_only_the_slice_glossary_and_rules() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = plano(vec![tarefa("A", &[]), tarefa("B", &["A"])]);
        p.glossario = Some("Pedido = linha de pedido".into());
        let texto = prompt_executor(&p, &p.tarefas[1], true);
        let esperado = "# Tarefa B · tarefa B\n\n## Mudança\nmudar algo\n\n## Arquivos\n- src/B.rs\n\n## Aceite\n- `cargo test x`\n\n## Glossário\nPedido = linha de pedido\n\n## Regras\n- Não explore além dos arquivos listados; leia só o trecho necessário.\n- Faça a menor mudança que cumpre a tarefa. Sem refatorar.\n- Rode só os testes desta tarefa (os critérios de aceite).\n- Se faltar contexto, pergunte com handoff_submit (data: {\"ok\": false}, summary: a pergunta) em vez de explorar.\n- Ao terminar, entregue com handoff_submit no esquema {changed[], tests, notes}.\n";
        assert_eq!(texto, esperado);
        assert!(!texto.contains("tarefa A"), "outras tarefas ficam de fora");
        let sem_regras = prompt_executor(&p, &p.tarefas[1], false);
        assert!(!sem_regras.contains("## Regras"));
    }

    #[test]
    fn dispatch_respects_deps_and_free_slots() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let ordem = vec![("m-A".to_string(), vec![]), ("m-B".to_string(), vec![]), ("m-C".to_string(), vec!["m-A".to_string()])];
        let mut status: HashMap<String, String> = [("m-A", "pendente"), ("m-B", "pendente"), ("m-C", "pendente")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        // C espera A; A e B abrem juntos com 2 vagas
        assert_eq!(escolher(&ordem, &status, 2), ["m-A", "m-B"]);
        // com 1 vaga, só A (na ordem do plano)
        assert_eq!(escolher(&ordem, &status, 1), ["m-A"]);
        status.insert("m-A".into(), "concluida".into());
        status.insert("m-B".into(), "em_execucao".into());
        assert_eq!(escolher(&ordem, &status, 2), ["m-C"], "C liberada por A; B já está em execução");
        assert!(escolher(&ordem, &status, 0).is_empty(), "sem vaga, nada abre");
    }

    #[test]
    fn plano_md_has_table_and_sections() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let md = render_md(&plano(vec![tarefa("A", &[])]), 2);
        assert!(md.starts_with("# Plano (versão 2)"));
        assert!(md.contains("| A | tarefa A | P | — |"));
        assert!(md.contains("### A · tarefa A"));
        assert!(md.contains("- `cargo test x`"));
    }

    #[test]
    fn ler_e_json_do_plano_roundtrip() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let p = validate(plano(vec![tarefa("A", &[])])).unwrap();
        let g = Guardado { versao: 1, planejador: "P1".into(), plano: p.clone() };
        artefatos::write(d.path(), "m1", "plano", &serde_json::to_string(&g).unwrap(), "json").unwrap();
        let lido = ler(d.path(), "m1").unwrap();
        assert_eq!(lido.plano, p);
        assert_eq!(lido.versao, 1);
    }

    use crate::db::Mission;

    fn estado(dir: &Path) -> Arc<AppState> {
        AppState::new(dir.into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap()
    }

    fn painel(id: &str, role: &str, cwd: &Path) -> PaneRow {
        PaneRow { id: id.into(), workspace_id: "w".into(), mission_id: Some("m".into()), kind: "agent".into(), name: id.into(), permission: "auto".into(), cwd: cwd.to_string_lossy().into(), status: "ocioso".into(), role: Some(role.into()), ..Default::default() }
    }

    #[test]
    fn only_the_mission_planner_submits_and_a_new_version_replaces_pending_tasks() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let s = estado(d.path());
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        s.db.insert_mission(&Mission { id: "m".into(), workspace_id: ws.id.clone(), name: "Missão".into(), mode: "agentico".into(), status: "ativa".into(), style: Some("plano".into()), created_at: 1, ..Default::default() }).unwrap();
        let cwd = d.path();
        s.db.insert_pane(&PaneRow { workspace_id: ws.id.clone(), ..painel("p", "planner", cwd) }).unwrap();
        s.db.insert_pane(&PaneRow { workspace_id: ws.id.clone(), ..painel("e", "executor", cwd) }).unwrap();
        let exec = s.db.get_pane("e").unwrap();
        assert_eq!(enviar(&s, &exec, plano(vec![tarefa("A", &[])])).unwrap_err().code(), "not_planner");

        let planner = s.db.get_pane("p").unwrap();
        let v1 = enviar(&s, &planner, plano(vec![tarefa("A", &[]), tarefa("B", &["A"])])).unwrap();
        assert_eq!(v1["versao"], 1);
        let tarefas = s.db.tasks("m").unwrap();
        assert_eq!(tarefas.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["m-A", "m-B"]);
        assert!(tarefas.iter().all(|t| t.status == "pendente"));
        let gates = s.db.open_gates(Some(&ws.id)).unwrap();
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].title, GATE_PLANO);
        assert!(d.path().join(".regente/artefatos/m/plano.md").exists());

        // nova versão sem B: B (ainda pendente) sai; A continua
        let v2 = enviar(&s, &planner, plano(vec![tarefa("A", &[])])).unwrap();
        assert_eq!(v2["versao"], 2);
        let por_id: HashMap<String, String> = s.db.tasks("m").unwrap().into_iter().map(|t| (t.id, t.status)).collect();
        assert_eq!(por_id["m-B"], "substituida");
        assert_eq!(por_id["m-A"], "pendente");
        assert_eq!(ler(d.path(), "m").unwrap().versao, 2);
    }

    fn estado_com_eventos(dir: &Path) -> (Arc<AppState>, Arc<Mutex<Vec<(String, Value)>>>) {
        let ev = Arc::new(Mutex::new(Vec::new()));
        let e2 = ev.clone();
        let emit: crate::runtime::Emit = Arc::new(move |n: &str, v: Value| lk(&e2).push((n.to_string(), v)));
        (AppState::new(dir.into(), PathBuf::from("C:/x/regente.exe"), emit).unwrap(), ev)
    }

    /// Missão de plano (`m`) com planejador `p` e o plano v1 entregue; `aprovar` grava a aprovação da v1 sem gate.
    fn missao_de_plano(dir: &Path, tarefas: Vec<Tarefa>, aprovar: bool) -> (Arc<AppState>, Arc<Mutex<Vec<(String, Value)>>>) {
        let (s, ev) = estado_com_eventos(dir);
        let ws = s.db.upsert_workspace(&dir.to_string_lossy()).unwrap();
        s.db.insert_mission(&Mission { id: "m".into(), workspace_id: ws.id.clone(), name: "Missão".into(), mode: "agentico".into(), status: "ativa".into(), style: Some("plano".into()), created_at: 1, ..Default::default() }).unwrap();
        s.db.insert_pane(&PaneRow { workspace_id: ws.id.clone(), ..painel("p", "planner", dir) }).unwrap();
        enviar(&s, &s.db.get_pane("p").unwrap(), plano(tarefas)).unwrap();
        if aprovar {
            let g = ler(dir, "m").unwrap();
            gravar_aprovacao(&s, dir, "m", &Aprovacao { aprovada: Some(1), aprovado: Some(g), ..Default::default() }).unwrap();
        }
        (s, ev)
    }

    fn tarefa_em(s: &AppState, id: &str) -> TaskItem {
        s.db.tasks("m").unwrap().into_iter().find(|t| t.id == id).unwrap()
    }

    fn pendente(s: &AppState, id: &str, status: &str) {
        s.db.upsert_task(&TaskItem { id: id.into(), mission_id: "m".into(), title: "x".into(), status: status.into(), owner_pane: None, created_at: 1, updated_at: 1 }).unwrap();
    }

    #[test]
    fn plan_submit_needs_a_plan_mission() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let s = estado(d.path());
        let ws = s.db.upsert_workspace(&d.path().to_string_lossy()).unwrap();
        s.db.insert_mission(&Mission { id: "m".into(), workspace_id: ws.id.clone(), name: "Missão".into(), mode: "agentico".into(), status: "ativa".into(), style: Some("chain".into()), created_at: 1, ..Default::default() }).unwrap();
        s.db.insert_pane(&PaneRow { workspace_id: ws.id.clone(), ..painel("p", "planner", d.path()) }).unwrap();
        let p = s.db.get_pane("p").unwrap();
        assert_eq!(enviar(&s, &p, plano(vec![tarefa("A", &[])])).unwrap_err().code(), "not_plan_mission");
    }

    #[test]
    fn titulo_refuses_control_characters_and_newlines() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = tarefa("A", &[]);
        t.titulo = "linha\nduas".into();
        assert!(erro(plano(vec![t])).contains("sem quebra de linha"));
        let mut t = tarefa("A", &[]);
        t.titulo = "x".repeat(121);
        assert!(erro(plano(vec![t])).contains("título"));
    }

    #[test]
    fn stale_gate_and_unapproved_versions_never_dispatch() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        // A: o gate da v1 aprovado depois de a v2 existir não aprova nada e avisa
        let d = tempfile::tempdir().unwrap();
        let (s, ev) = missao_de_plano(d.path(), vec![tarefa("A", &[])], false);
        let gate_v1 = s.db.open_gates(None).unwrap()[0].id.clone();
        enviar(&s, &s.db.get_pane("p").unwrap(), plano(vec![tarefa("A", &[]), tarefa("B", &[])])).unwrap();
        on_gate(&s, &gate_v1, "Aprovar", None);
        assert_eq!(ler_aprovacao(&s, "m").aprovada, None);
        assert!(lk(&ev).iter().any(|(n, v)| n == "exec-aviso" && v["text"].as_str().is_some_and(|t| t.contains("substituído"))));
        // B: a v2 sem aprovação não reivindica nada (nem as tarefas novas, nem as antigas)
        let p = s.db.get_pane("p").unwrap();
        assert!(reivindicar(&s, "m", d.path(), 5).unwrap().is_none());
        assert!(s.db.tasks("m").unwrap().iter().all(|t| t.status == "pendente"));
        assert_eq!(despachar(&s, "m", &p).unwrap(), 0);
    }

    #[test]
    fn aprovar_dispatches_and_a_full_workspace_blocks_the_task_and_warns() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, ev) = missao_de_plano(d.path(), vec![tarefa("A", &[])], false);
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        // espaço cheio: o executor não abre; a tarefa vira bloqueada e o usuário é avisado
        for i in 0..crate::mcp::MCP_PANE_MAX {
            s.db.insert_pane(&PaneRow { id: format!("x{i}"), workspace_id: ws.clone(), kind: "agent".into(), name: "x".into(), permission: "auto".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), ..Default::default() }).unwrap();
        }
        let gate = s.db.open_gates(None).unwrap()[0].id.clone();
        on_gate(&s, &gate, "Aprovar", None);
        assert_eq!(ler_aprovacao(&s, "m").aprovada, Some(1));
        assert_eq!(tarefa_em(&s, "m-A").status, "bloqueada");
        assert!(lk(&ev).iter().any(|(n, v)| n == "exec-aviso" && v["text"].as_str().is_some_and(|t| t.starts_with("Plano parado: "))));
    }

    #[test]
    fn concurrent_claims_never_exceed_the_cap_or_take_a_task_twice() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let tarefas: Vec<Tarefa> = (0..8).map(|i| tarefa(&format!("T{i}"), &[])).collect();
        let (s, _) = missao_de_plano(d.path(), tarefas, true);
        let cwd = d.path();
        let reservas: Vec<Vec<TaskItem>> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..4)
                .map(|_| sc.spawn(|| reivindicar(&s, "m", cwd, 3).unwrap().map(|(_, r, _)| r).unwrap_or_default()))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut ids: Vec<&str> = reservas.iter().flatten().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "o teto é 3 em curso, somado entre as threads");
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3, "nenhuma tarefa reivindicada duas vezes");
        assert_eq!(s.db.tasks("m").unwrap().iter().filter(|t| t.status == "em_execucao").count(), 3);
    }

    #[test]
    fn a_stale_plan_cannot_revert_a_running_task_and_substituted_ones_come_back() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[]), tarefa("B", &[])], true);
        assert_eq!(reivindicar(&s, "m", d.path(), 1).unwrap().unwrap().1.len(), 1);
        assert_eq!(tarefa_em(&s, "m-A").status, "em_execucao");
        // v2 sem A: A está em curso e não pode voltar a substituída; B (pendente) sai
        enviar(&s, &s.db.get_pane("p").unwrap(), plano(vec![tarefa("B", &[])])).unwrap();
        assert_eq!(tarefa_em(&s, "m-A").status, "em_execucao");
        assert_eq!(tarefa_em(&s, "m-A").owner_pane.as_deref(), Some("pendente"));
        // v3 {A,B}: B volta de substituída ou pendente; v4 {A} sai, v5 {A,B} devolve B
        enviar(&s, &s.db.get_pane("p").unwrap(), plano(vec![tarefa("A", &[])])).unwrap();
        assert_eq!(tarefa_em(&s, "m-B").status, "substituida");
        enviar(&s, &s.db.get_pane("p").unwrap(), plano(vec![tarefa("A", &[]), tarefa("B", &[])])).unwrap();
        assert_eq!(tarefa_em(&s, "m-B").status, "pendente", "substituída que volta ao plano fica pendente");
        assert_eq!(tarefa_em(&s, "m-A").status, "em_execucao");
    }

    #[test]
    fn the_plan_closes_and_is_marked_once() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[]), tarefa("B", &[])], true);
        pendente(&s, "m-A", "concluida");
        assert!(!reivindicar(&s, "m", d.path(), 5).unwrap().unwrap().2, "B ainda falta");
        pendente(&s, "m-B", "concluida");
        assert!(reivindicar(&s, "m", d.path(), 5).unwrap().unwrap().2, "a última entrega fecha o plano");
        assert!(!reivindicar(&s, "m", d.path(), 5).unwrap().unwrap().2, "a seguinte não avisa de novo");
        assert!(ler_aprovacao(&s, "m").concluida_em.is_some());
    }

    #[test]
    fn executor_that_closes_without_handoff_is_requeued_and_nothing_spawns_for_a_closed_planner() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        pendente(&s, "m-A", "em_execucao");
        s.db.upsert_task(&TaskItem { owner_pane: Some("e".into()), ..tarefa_em(&s, "m-A") }).unwrap();
        s.db.insert_pane(&PaneRow { workspace_id: s.db.get_mission("m").unwrap().workspace_id, parent_id: Some("p".into()), role: Some("executor".into()), ..painel("e", "executor", d.path()) }).unwrap();
        s.db.close_pane("p").unwrap(); // planejador fechado: a tarefa volta, mas nenhum executor abre
        executor_fechou(&s, "e");
        let t = tarefa_em(&s, "m-A");
        assert_eq!((t.status.as_str(), t.owner_pane), ("pendente", None));
        // painel que não é executor de plano: nada muda
        executor_fechou(&s, "p");
    }

    #[test]
    fn boot_requeues_only_running_tasks() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let db = crate::db::Db::open_in_memory().unwrap();
        for (id, st) in [("m-A", "em_execucao"), ("m-B", "bloqueada"), ("m-C", "concluida")] {
            db.upsert_task(&TaskItem { id: id.into(), mission_id: "m".into(), title: "x".into(), status: st.into(), owner_pane: Some("e".into()), created_at: 1, updated_at: 1 }).unwrap();
        }
        assert_eq!(db.requeue_running_tasks(5).unwrap(), 1);
        let por_id: HashMap<String, String> = db.tasks("m").unwrap().into_iter().map(|t| (t.id, t.status)).collect();
        assert_eq!(por_id["m-A"], "pendente");
        assert_eq!(por_id["m-B"], "bloqueada");
        assert_eq!(por_id["m-C"], "concluida");
    }

    #[test]
    fn plan_reply_is_for_the_planner_and_only_its_own_mission_tasks() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        s.db.insert_pane(&PaneRow { workspace_id: ws.clone(), parent_id: Some("p".into()), ..painel("e", "executor", d.path()) }).unwrap();
        let exec = s.db.get_pane("e").unwrap();
        assert_eq!(responder(&s, &exec, "A", "oi").unwrap_err().code(), "not_planner");
        // tarefa de outra missão não existe nesta
        s.db.upsert_task(&TaskItem { id: "outra-Z".into(), mission_id: "outra".into(), title: "z".into(), status: "em_execucao".into(), owner_pane: Some("e".into()), created_at: 1, updated_at: 1 }).unwrap();
        let planner = s.db.get_pane("p").unwrap();
        assert_eq!(responder(&s, &planner, "Z", "oi").unwrap_err().code(), "bad_task");
    }

    #[test]
    fn question_and_adjust_texts_are_cleaned_and_bracketed() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let b = bracketed(&pergunta_texto("T3", "qual\x1b[31m arquivo?\n"));
        assert!(b.starts_with(b"\x1b[200~") && b.ends_with(b"\x1b[201~\r"));
        let corpo = String::from_utf8_lossy(&b[6..b.len() - 7]).to_string();
        assert_eq!(corpo, "Executor T3 pergunta: qual[31m arquivo?\n. Responda com plan_reply task=T3.");
        let aj = bracketed(&ajuste_texto(Some("sem\x07 a Y")));
        assert_eq!(String::from_utf8_lossy(&aj[6..aj.len() - 7]), "Ajuste pedido no plano: sem a Y. Reenvie com plan_submit.");
    }

    /// Textos entregues aos painéis desde a última leitura (`(painel, texto com os colchetes de colagem)`).
    /// `FALHAS` é global (chave = tarefa `m-A`) e o despacho mexe nela: os testes do módulo rodam um de cada vez.
    static SERIAL_FALHAS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn entregues() -> Vec<(String, String)> {
        ENTREGUES.with(|e| e.borrow_mut().drain(..).map(|(p, d)| (p, String::from_utf8_lossy(&d).to_string())).collect())
    }

    #[test]
    fn ajustar_pastes_cleaned_and_bracketed_text_to_the_planner() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], false);
        entregues();
        let gate = s.db.open_gates(None).unwrap()[0].id.clone();
        on_gate(&s, &gate, "Ajustar", Some("sem\x07 o B"));
        assert_eq!(entregues(), vec![("p".to_string(), "\x1b[200~Ajuste pedido no plano: sem o B. Reenvie com plan_submit.\x1b[201~\r".to_string())]);
    }

    #[test]
    fn question_is_forwarded_once_and_only_when_the_planner_did_not_get_it() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        s.db.upsert_task(&TaskItem { id: "m-A".into(), mission_id: "m".into(), title: "x".into(), status: "em_execucao".into(), owner_pane: Some("e".into()), created_at: 1, updated_at: 1 }).unwrap();
        s.db.insert_pane(&PaneRow { workspace_id: ws, parent_id: Some("p".into()), ..painel("e", "executor", d.path()) }).unwrap();
        let exec = s.db.get_pane("e").unwrap();
        entregues();
        on_handoff(&s, &exec, Some(&json!({"ok": false})), "qual arquivo?", None);
        let e = entregues();
        assert_eq!(e.len(), 1, "uma pergunta, uma colagem");
        assert!(e[0].1.contains("Executor A pergunta: qual arquivo?. Responda com plan_reply task=A."));
        // já entregue ao planejador pelo handoff: não cola de novo
        on_handoff(&s, &exec, Some(&json!({"ok": false})), "qual arquivo?", Some("p"));
        assert!(entregues().is_empty());
        assert_eq!(tarefa_em(&s, "m-A").status, "em_execucao", "a pergunta não fecha a tarefa");
    }

    #[test]
    fn early_handoff_before_the_owner_is_saved_keeps_the_task_done() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        assert_eq!(reivindicar(&s, "m", d.path(), 1).unwrap().unwrap().1.len(), 1);
        assert_eq!(tarefa_em(&s, "m-A").owner_pane.as_deref(), Some(RESERVADA));
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        // o executor já existe com o nome da tarefa; a tarefa é achada pelo nome enquanto o dono é a reserva
        s.db.insert_pane(&PaneRow { workspace_id: ws, parent_id: Some("p".into()), name: "A · tarefa A".into(), ..painel("e", "executor", d.path()) }).unwrap();
        let exec = s.db.get_pane("e").unwrap();
        let ts = s.db.tasks("m").unwrap();
        assert_eq!(acha_tarefa(&ts, "m", &exec).map(|t| t.id), Some("m-A".to_string()), "achada pelo nome do painel");
        // a entrega já passou da verificação (concluída) quando o despacho grava o dono: não reabre
        s.db.upsert_task(&TaskItem { status: "concluida".into(), ..tarefa_em(&s, "m-A") }).unwrap();
        fixar_dono(&s, "m", "m-A", "e");
        assert_eq!(tarefa_em(&s, "m-A").status, "concluida", "o dono que chega tarde não reabre a tarefa");
    }

    #[test]
    fn nothing_spawns_for_an_archived_or_closing_mission() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        let p = s.db.get_pane("p").unwrap();
        {
            let _arquivando = Arquivando::new("m");
            assert_eq!(despachar(&s, "m", &p).unwrap(), 0, "em arquivamento");
        }
        s.db.update_mission(&Mission { archived_at: Some(1), ..s.db.get_mission("m").unwrap() }).unwrap();
        assert_eq!(despachar(&s, "m", &p).unwrap(), 0, "missão arquivada");
        assert!(s.db.tasks("m").unwrap().iter().all(|t| t.status == "pendente"), "nenhuma tarefa reservada");
    }

    #[test]
    fn version_never_restarts_when_the_plan_file_is_gone() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        std::fs::remove_file(pasta(d.path(), "m").join("plano.json")).unwrap();
        let v = enviar(&s, &s.db.get_pane("p").unwrap(), plano(vec![tarefa("A", &[])])).unwrap();
        assert_eq!(v["versao"], 2, "a aprovada é a 1: a próxima é a 2");
    }

    /// Executor `e` da tarefa `m-A` em verificação (o estado depois da entrega).
    fn em_verificacao(s: &AppState, d: &Path) {
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        s.db.insert_pane(&PaneRow { workspace_id: ws, parent_id: Some("p".into()), name: "A · tarefa A".into(), ..painel("e", "executor", d) }).unwrap();
        s.db.upsert_task(&TaskItem { id: "m-A".into(), mission_id: "m".into(), title: "x".into(), status: "verificando".into(), owner_pane: Some("e".into()), created_at: 1, updated_at: 1 }).unwrap();
    }

    #[test]
    fn escalation_ladder_stops_at_the_planner() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(proximo_passo(1, true), Passo::Mesmo);
        assert_eq!(proximo_passo(2, true), Passo::Escalar);
        assert_eq!(proximo_passo(2, false), Passo::Mesmo);
        assert_eq!(proximo_passo(3, true), Passo::Planejador);
        assert_eq!(proximo_passo(4, true), Passo::Planejador, "sem laço além do planejador");
    }

    #[test]
    fn only_real_commands_from_acceptance_run_as_checks() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(eh_comando("cargo test x"));
        assert!(eh_comando("  npx vitest run"));
        assert!(!eh_comando("o arquivo existe e tem a função"));
        assert!(!eh_comando("npm test\nrm -rf x"));
    }

    #[test]
    fn a_failed_check_goes_back_to_the_executor_then_to_the_planner() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        lk(&FALHAS).clear();
        em_verificacao(&s, d.path());
        entregues();
        // 1ª falha: o mesmo executor recebe o digest e a tarefa volta a em curso
        falhou(&s, "m", "m-A", "e", "p", "erro X").unwrap();
        let e1 = entregues();
        assert_eq!(e1.len(), 1);
        assert_eq!(e1[0].0, "e");
        assert!(e1[0].1.contains("A verificação falhou: erro X. Corrija só isso e entregue de novo."), "{e1:?}");
        assert_eq!(tarefa_em(&s, "m-A").status, "em_execucao");
        // o executor entrega de novo e falha outra vez: sem modelo registrado não há degrau, então o planejador decide
        s.db.upsert_task(&TaskItem { status: "verificando".into(), ..tarefa_em(&s, "m-A") }).unwrap();
        falhou(&s, "m", "m-A", "e", "p", "erro Y").unwrap();
        let e2 = entregues();
        assert!(e2.iter().any(|(p, t)| p == "p" && t.contains("A falhou 2 vezes: erro Y. Ajuste a tarefa com plan_submit ou responda com plan_reply.")), "{e2:?}");
        assert_eq!(tarefa_em(&s, "m-A").status, "bloqueada");
        assert_eq!(tarefa_em(&s, "m-A").owner_pane, None);
    }

    #[test]
    fn a_passing_check_concludes_the_task_once() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        lk(&FALHAS).clear();
        em_verificacao(&s, d.path());
        // a vaga da missão está cheia: o despacho do que sobrar não abre painel nesta prova
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        for i in 0..crate::mcp::MCP_PANE_MAX {
            s.db.insert_pane(&PaneRow { id: format!("x{i}"), workspace_id: ws.clone(), kind: "agent".into(), name: "x".into(), permission: "auto".into(), cwd: d.path().to_string_lossy().into(), status: "ocioso".into(), ..Default::default() }).unwrap();
        }
        concluir(&s, "m", "m-A", "e", "p").unwrap();
        assert_eq!(tarefa_em(&s, "m-A").status, "concluida");
        // segunda chamada (a entrega já foi tratada): nada muda
        concluir(&s, "m", "m-A", "e", "p").unwrap();
        assert_eq!(tarefa_em(&s, "m-A").status, "concluida");
    }

    fn aceite_com(c: char) -> AppResult<Plano> {
        let mut t = tarefa("A", &[]);
        t.aceite = vec![format!("cargo test a{c}b")];
        validate(plano(vec![t]))
    }

    macro_rules! recusa_metacaractere {
        ($($nome:ident => $c:expr),* $(,)?) => {
            $(
                #[test]
                fn $nome() {
                    assert!(erro_de(aceite_com($c)).contains("caractere de shell"), "caractere {}", $c);
                }
            )*
        };
    }

    fn erro_de(r: AppResult<Plano>) -> String {
        r.unwrap_err().to_string()
    }

    recusa_metacaractere! {
        recusa_e_comercial => '&',
        recusa_barra_vertical => '|',
        recusa_menor => '<',
        recusa_maior => '>',
        recusa_circunflexo => '^',
        recusa_percento => '%',
        recusa_aspas => '"',
        recusa_ponto_e_virgula => ';',
        recusa_parentese_abre => '(',
        recusa_parentese_fecha => ')',
        recusa_exclamacao => '!',
        recusa_crase => '`',
    }

    #[test]
    fn aceite_limpo_passa_na_validacao() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(validate(plano(vec![tarefa("A", &[])])).is_ok());
    }

    #[test]
    fn so_comando_de_ferramenta_conhecida_roda_como_aceite() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(eh_comando("cargo test x"));
        assert!(eh_comando("npx vitest run"));
        assert!(eh_comando("npx tsc --noEmit"));
        assert!(!eh_comando("npx create-foo"), "npx só para as ferramentas da lista");
        assert!(!eh_comando("npx vitest run --yes"));
        assert!(!eh_comando("npx -y vitest run"));
        assert!(!eh_comando("npx vitest -p evil"));
        assert!(!eh_comando("npx vitest --package=evil"));
        for proibido in ["node x.js", "python t.py", "make test", "bun test", "yarn test", "python3 t.py"] {
            assert!(!eh_comando(proibido), "{proibido}");
        }
        assert!(!eh_comando("cargo test a&b"));
    }

    #[test]
    fn gate_mostra_todos_os_comandos_e_recusa_secao_grande() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = tarefa("A", &[]);
        t.aceite = vec!["cargo test um".into(), "npx vitest run".into()];
        let p = plano(vec![t, tarefa("B", &[])]);
        let p = validate(p).unwrap();
        let md = render_md(&p, 1).repeat(40);
        let r = resumo_gate(&p, &md, "m");
        assert!(r.starts_with("## Comandos que o Regente vai rodar\n"));
        assert!(r.contains("- A: `cargo test um`") && r.contains("- A: `npx vitest run`"));
        assert!(r.chars().count() <= RESUMO_GATE_MAX + 200);
        let mut grande = tarefa("G", &[]);
        grande.aceite = (0..20).map(|i| format!("cargo test {}", "x".repeat(250) + &i.to_string())).collect();
        let mut tarefas = vec![grande];
        for i in 0..5 {
            let mut t = tarefa(&format!("T{i}"), &[]);
            t.aceite = (0..20).map(|j| format!("cargo test {}", "y".repeat(250) + &j.to_string())).collect();
            tarefas.push(t);
        }
        assert!(erro(plano(tarefas)).contains("não cabem no resumo do gate"));
    }

    #[test]
    fn verificacao_roda_o_aceite_aprovado_nao_o_do_disco() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let mut a = tarefa("A", &[]);
        a.aceite = vec!["cargo test v1".into()];
        let (s, _) = missao_de_plano(d.path(), vec![a], true);
        // o planejador entrega a v2 com outro aceite: o disco muda, a aprovação não
        let mut a2 = tarefa("A", &[]);
        a2.aceite = vec!["cargo test v2".into()];
        enviar(&s, &s.db.get_pane("p").unwrap(), plano(vec![a2])).unwrap();
        let cmds = comandos_da_entrega(&s, "m", "m-A").unwrap();
        assert!(cmds.contains(&"cargo test v1".to_string()), "{cmds:?}");
        assert!(!cmds.iter().any(|c| c.contains("v2")), "{cmds:?}");
    }

    #[test]
    fn sem_plano_aprovado_nenhum_aceite_roda() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], false);
        assert!(comandos_da_entrega(&s, "m", "m-A").is_err());
    }

    #[test]
    fn entrega_repetida_nao_abre_segunda_verificacao() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        em_verificacao(&s, d.path());
        let e = s.db.get_pane("e").unwrap();
        on_handoff(&s, &e, None, "de novo", None);
        assert_eq!(tarefa_em(&s, "m-A").status, "verificando");
        assert!(!lk(&VERIFICANDO).contains("m-A"), "nenhuma thread nova");
    }

    #[test]
    fn retomar_devolve_verificacao_presa_a_fila() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        em_verificacao(&s, d.path());
        lk(&VERIFICANDO).clear();
        retomar(&s, "m").unwrap();
        let t = tarefa_em(&s, "m-A");
        assert_ne!(t.status, "verificando", "sem thread viva, a entrega sai da verificação");
    }

    #[test]
    fn aprovar_copia_o_plano_que_o_gate_mostrou_nao_o_do_disco() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let mut a = tarefa("A", &[]);
        a.aceite = vec!["cargo test v1".into()];
        let (s, _) = missao_de_plano(d.path(), vec![a], false);
        let ws = s.db.get_mission("m").unwrap().workspace_id;
        let gate = s.db.open_gates(Some(&ws)).unwrap().into_iter().find(|g| g.title == GATE_PLANO).unwrap();
        // o disco é editado entre o gate e o Aprovar
        let mut mexido = ler(d.path(), "m").unwrap();
        mexido.plano.tarefas[0].aceite = vec!["cargo test FORJADO".into()];
        std::fs::write(pasta(d.path(), "m").join("plano.json"), serde_json::to_string(&mexido).unwrap()).unwrap();
        on_gate(&s, &gate.id, "Aprovar", None);
        let aprovado = ler_aprovacao(&s, "m").aprovado.expect("aprovado");
        assert_eq!(aprovado.plano.tarefas[0].aceite, vec!["cargo test v1".to_string()]);
    }

    #[test]
    fn eh_comando_so_aceita_subcomandos_de_teste_e_build() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        for ok in ["cargo test x", "cargo nextest run", "cargo clippy", "npm test", "npm run test:unit", "pnpm run test", "go vet ./...", "dotnet build", "pytest -q", "npx tsc --noEmit"] {
            assert!(eh_comando(ok), "{ok}");
        }
        for no in [
            "npm exec evil", "npm run build", "npm install x", "pnpm dlx evil", "pnpm exec x", "cargo run", "cargo install x",
            "cargo test --config x", "cargo --config x test", "go run main.go", "dotnet run", "go generate", "yarn test",
            "go test -toolexec evil ./...", "go test -exec=evil ./...", "go vet -vettool=evil ./...", "cargo test -- --shell x", "pytest --plugin evil",
        ] {
            assert!(!eh_comando(no), "{no}");
        }
    }

    #[test]
    fn retomar_sem_plano_aprovado_nao_le_o_disco() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], false);
        assert_eq!(retomar(&s, "m").unwrap_err().code(), "sem_plano_aprovado");
    }

    #[test]
    fn verificacao_cancelada_volta_a_execucao_sem_contar_falha() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        em_verificacao(&s, d.path());
        devolver_cancelada(&s, "m", "m-A", "e").unwrap();
        assert_eq!(tarefa_em(&s, "m-A").status, "em_execucao");
        assert!(!lk(&FALHAS).contains_key("m-A"), "cancelar não conta falha");
    }

    #[test]
    fn missao_arquivando_nao_conta_falha_nem_escala() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let (s, _) = missao_de_plano(d.path(), vec![tarefa("A", &[])], true);
        em_verificacao(&s, d.path());
        let _arq = Arquivando::new("m");
        falhou(&s, "m", "m-A", "e", "p", "digest").unwrap();
        assert_eq!(tarefa_em(&s, "m-A").status, "verificando", "nada muda com a missão arquivando");
        assert!(!lk(&FALHAS).contains_key("m-A"));
    }

    #[test]
    fn guarda_tira_a_tarefa_de_verificando_mesmo_com_panico() {
        let _serial = SERIAL_FALHAS.lock().unwrap_or_else(|e| e.into_inner());
        lk(&VERIFICANDO).insert("m-P".into());
        let r = std::panic::catch_unwind(|| {
            let _g = Verificando("m-P".into());
            panic!("falha simulada");
        });
        assert!(r.is_err());
        assert!(!lk(&VERIFICANDO).contains("m-P"));
    }
}
