//! Política de compactação por custo (RM-16 / H2, roadmap §4.4). Núcleo puro: `decide` diz se vale compactar,
//! recomeçar com memória, propor ao usuário ou não fazer nada. Quem chama junta os dados do painel, o preço do modelo e
//! o histórico, e age (escreve `/compact` ou `/clear` no PTY, registra no feed).
use crate::ajustes;
use crate::contexto::Medida;
use crate::cost::prices::PriceEntry;
use crate::db::PaneRow;
use crate::error::{fail, AppError, AppResult};
use crate::memory::clean;
use crate::orchestrator::bracketed;
use crate::perfil::{self, papel_longo, Contexto, Perfil};
use crate::planexec;
use crate::runtime::AppState;
use crate::status::PaneStatus;
use crate::util::{lk, now_ms};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Fração do contexto que vira resumo, até a calibração medir (PostCompact). [inferido]
pub const RAZAO_RESUMO: f64 = 0.03;
const S_MIN: f64 = 1500.0;
const S_MAX: f64 = 12000.0;
/// Tamanho típico do brief de retomada, em tokens (2500 caracteres). [inferido]
pub const BRIEF_TOKENS: f64 = 1000.0;
/// Turnos totais assumidos quando há poucos painéis parecidos para a mediana.
const TURNOS_PADRAO: f64 = 10.0;
const AMOSTRAS_MIN: usize = 5;
/// Tecla recente: o painel não recebe compactação nem recomeço por este tempo.
pub const TECLA_RECENTE: Duration = Duration::from_secs(30);
/// Piloto com ajudantes rodando só é mexido a partir deste % de contexto.
const PILOTO_COM_AJUDANTES_PCT: f64 = 75.0;

/// O painel, do jeito que a decisão precisa. Quem preenche é `compactacao_tick` (runtime).
#[derive(Debug, Clone)]
pub struct PaneCtx {
    /// Papel do agente; None = painel do usuário (sempre `Propor`).
    pub papel: Option<String>,
    pub tipo_tarefa: Option<String>,
    /// Porcentagem da janela (statusline).
    pub pct: f64,
    /// Tokens de contexto atuais (`ctx`).
    pub ctx_tokens: f64,
    /// Prefixo fixo do painel (`first_turn_tokens`); 0 = ainda não medido.
    pub prefixo: f64,
    pub turnos: i64,
    /// Turnos desde a última ação automática do painel (compactar ou recomeçar). Mínimo para agir de novo: 5.
    pub turnos_desde_acao: i64,
    /// Ação automática há menos de 10 min: o painel espera o cooldown.
    pub em_cooldown: bool,
    /// Status `ocioso` (depois de um Stop).
    pub ocioso: bool,
    /// Última entrada no painel (tecla ou escrita).
    pub ultima_entrada: Option<Instant>,
    /// O usuário está digitando sem Enter.
    pub digitando: bool,
    /// Última saída do CLI (para saber se o cache ainda está quente).
    pub ultima_atividade: Option<Instant>,
    /// TTL do cache do painel (5 min ou 1 h).
    pub cache_ttl: Duration,
    /// Há mensagens pendentes para entregar ao painel.
    pub fila: bool,
    /// Há gate aberto ou pergunta pendente.
    pub gate: bool,
    /// Fronteira de tarefa do papel (ver §4.4 do roadmap).
    pub fronteira: bool,
    /// Piloto com ajudantes trabalhando.
    pub ajudantes_rodando: bool,
    /// Planner: tarefas do plano que ainda faltam (somam no R).
    pub pendentes_plano: u64,
    /// Brief e arquivos modificados, para o texto de foco.
    pub brief: String,
    pub arquivos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decisao {
    Compactar { foco: String },
    Recomecar { brief: String },
    Propor { acao: &'static str, motivo: String },
    Pular { motivo: String },
}

/// Histórico que a decisão usa: turnos de painéis fechados do mesmo papel e tipo, e a razão de resumo calibrada.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hist {
    pub turnos_fechados: Vec<i64>,
    pub razao_resumo: Option<f64>,
    /// Delta de K aprendido pelo piloto automático (T1), somado ao `paybackK` do perfil; None = nenhum.
    pub k_aprendido: Option<f64>,
    /// Fator aprendido sobre os turnos restantes (T1); None = 1.
    pub fator_restante: Option<f64>,
}

impl Hist {
    /// Turnos restantes estimados: mediana dos fechados (ou 10 sem amostra suficiente) menos os atuais, sem ir abaixo de 0.
    pub fn restante(&self, turnos: i64) -> f64 {
        let total = if self.turnos_fechados.len() >= AMOSTRAS_MIN {
            let mut v = self.turnos_fechados.clone();
            v.sort_unstable();
            let m = v.len() / 2;
            if v.len() % 2 == 1 { v[m] as f64 } else { (v[m - 1] + v[m]) as f64 / 2.0 }
        } else {
            TURNOS_PADRAO
        };
        (total - turnos as f64).max(0.0) * self.fator()
    }

    fn fator(&self) -> f64 {
        self.fator_restante.unwrap_or(1.0)
    }

    fn razao(&self) -> f64 {
        self.razao_resumo.unwrap_or(RAZAO_RESUMO)
    }
}

/// A conta da compactação (US$ e turnos), para o feed.
#[derive(Debug, Clone, PartialEq)]
pub struct Conta {
    /// Custo de compactar agora.
    pub c: f64,
    /// Ganho por turno depois de compactar.
    pub g: f64,
    pub payback: f64,
    /// Turnos restantes estimados.
    pub r: f64,
    /// Tamanho estimado do resumo, em tokens.
    pub s: f64,
    /// K usado nesta conta: o do perfil mais o delta aprendido.
    pub k: f64,
}

/// Bloqueios de agir agora, em pt-BR. None = o painel está ocioso o bastante.
pub fn bloqueio(p: &PaneCtx, now: Instant) -> Option<String> {
    bloqueio_de(p.ocioso, p.digitando, p.ultima_entrada, p.fila, p.gate, now)
}

/// O mesmo que `bloqueio`, com os campos soltos. O recomeço pendente passa `ultima_entrada` = None: a própria escrita
/// do /clear é uma tecla nossa, então a regra dos 30 s não vale para ele.
fn bloqueio_de(ocioso: bool, digitando: bool, ultima_entrada: Option<Instant>, fila: bool, gate: bool, now: Instant) -> Option<String> {
    if !ocioso {
        return Some("painel não está ocioso".into());
    }
    if digitando {
        return Some("você está digitando neste painel".into());
    }
    if ultima_entrada.is_some_and(|t| now.duration_since(t) < TECLA_RECENTE) {
        return Some("você digitou há menos de 30 s".into());
    }
    if fila {
        return Some("há mensagens na fila deste painel".into());
    }
    if gate {
        return Some("há um gate ou pergunta pendente".into());
    }
    None
}

/// Conta do roadmap §4.4: C = ctx·cr + S·out + (P+S)·cw; G = (ctx−(P+S))·cr; payback = C/G; cache frio usa a entrada no lugar de cr.
/// Preços em US$ por milhão de tokens. None sem contexto ou sem prefixo medido.
pub fn conta(p: &PaneCtx, preco: &PriceEntry, hist: &Hist, k: u32, now: Instant) -> Option<Conta> {
    if p.ctx_tokens <= 0.0 || p.prefixo <= 0.0 {
        return None;
    }
    let frio = p.ultima_atividade.is_none_or(|t| now.duration_since(t) > p.cache_ttl);
    let cr = if frio { preco.input } else { preco.cache_read };
    let s = (hist.razao() * p.ctx_tokens).clamp(S_MIN, S_MAX);
    let c = (p.ctx_tokens * cr + s * preco.output + (p.prefixo + s) * preco.cache_write) / 1e6;
    let g = (p.ctx_tokens - (p.prefixo + s)) * cr / 1e6;
    let payback = if g > 0.0 { c / g } else { f64::INFINITY };
    let r = hist.restante(p.turnos) + p.pendentes_plano as f64;
    Some(Conta { c, g, payback, r, s, k: crate::autoajuste::k_efetivo(k, hist.k_aprendido) })
}

/// A decisão do painel. Ordem: papel curto; bloqueios de ociosidade; piloto com ajudantes; fronteira de tarefa
/// (recomeço ou proposta); senão a conta de payback. Painel do usuário nunca age sozinho.
pub fn decide(p: &PaneCtx, cfg: &Contexto, preco: &PriceEntry, hist: &Hist, now: Instant) -> Decisao {
    let papel = p.papel.as_deref();
    if !papel_longo(papel) {
        return Decisao::Pular { motivo: "papel de vida curta".into() };
    }
    if let Some(m) = bloqueio(p, now) {
        return Decisao::Pular { motivo: m };
    }
    if papel == Some("piloto") && p.ajudantes_rodando && p.pct < PILOTO_COM_AJUDANTES_PCT {
        return Decisao::Pular { motivo: format!("piloto espera ajudantes (ctx {}% < 75%)", p.pct.round() as i64) };
    }
    // ação automática (compactar ou recomeçar) só depois de 5 turnos desde a última e fora do cooldown de 10 min
    let ciclo_ok = p.turnos_desde_acao >= CICLO_TURNOS && !p.em_cooldown;
    let espera = format!("espera o ciclo: {} de {CICLO_TURNOS} turnos desde a última ação, cooldown de 10 min", p.turnos_desde_acao.max(0));
    // na fronteira o recomeço custa só o prefixo e o brief: vale quando o contexto atual já passa disso
    let recomeco_vale = p.ctx_tokens > p.prefixo + BRIEF_TOKENS;
    if p.fronteira && recomeco_vale && p.prefixo > 0.0 && p.ctx_tokens >= FRONTEIRA_CTX_MIN {
        let motivo = format!("fronteira de tarefa com contexto de {} tokens", p.ctx_tokens.round() as i64);
        if papel.is_none() {
            return Decisao::Propor { acao: "recomecar", motivo };
        }
        if cfg.recomecar == "auto" && matches!(papel, Some("piloto" | "planner" | "maestro")) {
            if !ciclo_ok {
                return Decisao::Pular { motivo: espera };
            }
            return Decisao::Recomecar { brief: brief_retomada_de(p) };
        }
        return Decisao::Propor { acao: "recomecar", motivo };
    }
    let Some(c) = conta(p, preco, hist, cfg.payback_k, now) else {
        return Decisao::Pular { motivo: "prefixo ainda não medido".into() };
    };
    if c.g <= 0.0 {
        return Decisao::Pular { motivo: "não compensa: sem ganho por turno".into() };
    }
    // K efetivo: o do perfil mais o delta aprendido (T1), o mesmo que vai para o feed
    let k = c.k;
    if c.payback > k {
        return Decisao::Pular { motivo: format!("não compensa: payback {} turnos > K={}", dec(c.payback), k_txt(k)) };
    }
    let minimo = (c.payback + 1.0).max(cfg.restante_min as f64);
    if c.r < minimo {
        return Decisao::Pular { motivo: format!("restante de {:.0} turnos é pouco para pagar {} turnos", c.r, dec(c.payback)) };
    }
    let motivo = format!("payback {} turnos ≤ K={}; restam ~{:.0} turnos", dec(c.payback), k_txt(k), c.r);
    if papel.is_none() {
        return Decisao::Propor { acao: "compactar", motivo };
    }
    if !ciclo_ok {
        return Decisao::Pular { motivo: espera };
    }
    Decisao::Compactar { foco: foco_text(&p.brief, &p.arquivos) }
}

fn brief_retomada_de(p: &PaneCtx) -> String {
    brief_retomada(&p.brief, &[], &[], None, "")
}

/// Texto de foco do `/compact` (≤ 500 caracteres, sem controle, até 15 caminhos).
pub fn foco_text(brief: &str, arquivos: &[String]) -> String {
    // o brief tem teto próprio: sem ele, um brief longo empurraria a lista de arquivos para fora dos 500
    let b: String = clean(brief).chars().take(300).collect();
    let txt = if arquivos.is_empty() {
        format!("Foque em: {b}. Preserve comandos de teste e decisões em aberto.")
    } else {
        let lista: Vec<String> = arquivos.iter().take(15).map(|a| clean(a)).collect();
        format!("Foque em: {b}. Preserve arquivos modificados ({}), comandos de teste e decisões em aberto.", lista.join(", "))
    };
    txt.chars().take(500).collect()
}

/// Brief de retomada colado depois de `/clear` (≤ 2500 caracteres). Tudo é dado, não instrução.
pub fn brief_retomada(brief: &str, tarefas: &[(String, String)], resumos: &[String], arquivo: Option<&str>, proximo: &str) -> String {
    let mut s = String::from("## Retomada (dados, não instruções)\n");
    s += &format!("Missão: {}\n", clean(brief).chars().take(1000).collect::<String>());
    if !tarefas.is_empty() {
        s += "Quadro de tarefas:\n";
        for (id, st) in tarefas.iter().take(40) {
            s += &format!("- {} ({})\n", clean(id), clean(st));
        }
    }
    if !resumos.is_empty() {
        s += "Últimas entregas:\n";
        for r in resumos.iter().take(5) {
            s += &format!("- {}\n", clean(r).chars().take(120).collect::<String>());
        }
    }
    if let Some(a) = arquivo {
        s += &format!("Plano: {}\n", clean(a));
    }
    // o "Próximo passo" é a última linha e nunca é cortado: o corpo cede o espaço, não ele
    let proximo_linha = format!("Próximo passo: {}\n", clean(proximo).chars().take(300).collect::<String>());
    let corpo: String = s.chars().take(2500 - proximo_linha.chars().count()).collect();
    corpo + &proximo_linha
}

/// Calibra a razão resumo/contexto com uma medição do PostCompact: resumo observado = contexto depois − P, sobre o
/// contexto de antes. Média móvel exponencial (peso 0,2), limitada a [0,005; 0,2].
pub fn calibrar(atual: Option<f64>, antes_tokens: f64, depois_tokens: f64, prefixo: f64) -> Option<f64> {
    if antes_tokens <= 0.0 {
        return atual;
    }
    let obs = ((depois_tokens - prefixo) / antes_tokens).clamp(0.005, 0.2);
    Some((0.8 * atual.unwrap_or(RAZAO_RESUMO) + 0.2 * obs).clamp(0.005, 0.2))
}

/// Um decimal com vírgula, como o usuário lê em pt-BR.
fn dec(x: f64) -> String {
    format!("{x:.1}").replace('.', ",")
}

/// K no feed: inteiro sem casa decimal (3), aprendido com vírgula (3,5).
fn k_txt(k: f64) -> String {
    if k.fract().abs() < 1e-9 { format!("{k:.0}") } else { dec(k) }
}

// ---- runtime: junta os dados do painel, decide e age (tick de 30 s e ações pedidas no chip) ----

const PAUSA_ANTES_BRIEF: Duration = Duration::from_secs(3);
const PRAZO_BRIEF: Duration = Duration::from_secs(120);
const FRESCOR_MEDIDA: Duration = Duration::from_secs(120);
/// Turnos depois de uma ação até medir o efeito (feed).
const EFEITO_TURNOS: i64 = 5;
/// "Pular" só é registrado com contexto a partir disto, e no máximo 1 por painel a cada 10 min.
const REGISTRA_PULAR_ACIMA: f64 = 150_000.0;
const INTERVALO_PULAR: Duration = Duration::from_secs(600);
/// Fronteira do painel do usuário: Stop com contexto a partir deste %.
const PROPOR_ACIMA_PCT: f64 = 50.0;
/// Acima disto o git da pasta é lido para o foco (lista de arquivos modificados).
/// Turnos mínimos entre duas ações automáticas do mesmo painel (e cooldown de 10 min, em `Estado`).
const CICLO_TURNOS: i64 = 5;
/// Contexto mínimo para a fronteira de tarefa valer como gatilho (abaixo disso, o recomeço não compensa).
const FRONTEIRA_CTX_MIN: f64 = 50_000.0;
const COOLDOWN_AUTO: Duration = Duration::from_secs(600);

/// Estado da política no processo (fica no `CtxRt`): calibração, propostas abertas, recomeços e efeitos a medir.
#[derive(Default)]
pub struct Estado {
    /// Razão resumo/contexto calibrada pelas medições do PostCompact (None = padrão).
    pub razao: Option<f64>,
    /// Propostas abertas por painel: a ação (`compactar` | `recomecar`) que o chip mostra.
    pub propostas: HashMap<String, String>,
    pular_em: HashMap<String, Instant>,
    /// Recomeço em curso: `/clear` mandado no instante; o brief entra quando o painel ficar ocioso.
    aguarda_limpeza: HashMap<String, (Instant, String)>,
    /// Ações à espera do efeito medido (5 turnos depois).
    efeitos: HashMap<String, Efeito>,
    /// Último marco do painel: a última ação (compactar ou recomeçar) ou a entrada do brief. Só o que acontece
    /// depois dele conta como fronteira; o cooldown e os turnos mínimos contam a partir dele.
    marco: HashMap<String, Marco>,
    /// Último feed de "proposto" por painel: no máximo um a cada 10 min (a não ser que a ação mude).
    proposto_em: HashMap<String, Instant>,
}

/// Marco do painel (`Estado::marco`): quando e em que turno a última ação aconteceu.
#[derive(Debug, Clone, Copy)]
struct Marco {
    em_ms: i64,
    turnos: i64,
    quando: Instant,
}

#[derive(Debug, Clone, PartialEq)]
struct Efeito {
    ajuste: String,
    turnos: i64,
    custo: f64,
    tokens: i64,
    ctx: f64,
    cr: f64,
}

impl Estado {
    pub fn esquecer(&mut self, pane: &str) {
        self.propostas.remove(pane);
        self.pular_em.remove(pane);
        self.aguarda_limpeza.remove(pane);
        self.efeitos.remove(pane);
        self.marco.remove(pane);
        self.proposto_em.remove(pane);
    }

    /// Marca a ação (compactar ou recomeçar): nova fronteira, novo turno de referência e início do cooldown.
    fn marcar(&mut self, pane: &str, em_ms: i64, turnos: i64, now: Instant) {
        self.marco.insert(pane.into(), Marco { em_ms, turnos, quando: now });
    }

    /// Feed de "proposto": no máximo um por painel a cada 10 min. True = pode registrar agora.
    fn proposta_feed_pode(&mut self, pane: &str, now: Instant) -> bool {
        if self.proposto_em.get(pane).is_some_and(|t| now.duration_since(*t) < INTERVALO_PULAR) {
            return false;
        }
        self.proposto_em.insert(pane.into(), now);
        true
    }

    /// Guarda a proposta. True = mudou (a interface precisa saber).
    fn propor(&mut self, pane: &str, acao: &str) -> bool {
        if self.propostas.get(pane).map(String::as_str) == Some(acao) {
            return false;
        }
        self.propostas.insert(pane.into(), acao.into());
        true
    }

    /// Tira a proposta. True = havia uma.
    fn sem_proposta(&mut self, pane: &str) -> bool {
        self.propostas.remove(pane).is_some()
    }

    /// Libera um "Pular" para o feed: no máximo um por painel a cada 10 min.
    fn pular_pode(&mut self, pane: &str, now: Instant) -> bool {
        if self.pular_em.get(pane).is_some_and(|t| now.duration_since(*t) < INTERVALO_PULAR) {
            return false;
        }
        self.pular_em.insert(pane.into(), now);
        true
    }
}

/// Junta o que a decisão precisa de um painel. `manual`: ação pedida pelo usuário, então o git é lido sempre.
fn montar(s: &AppState, row: &PaneRow, medida: &Medida, manual: bool) -> (PaneCtx, Perfil) {
    let now = Instant::now();
    let (perfil, _) = perfil::efetivo_de(s, &row.workspace_id, row.profile.as_deref());
    let m = s.db.pane_metrics(&row.id).ok().flatten().unwrap_or_default();
    let janela = medida.janela.unwrap_or(0) as f64;
    let pct = medida.pct.unwrap_or(0.0);
    let ctx_tokens = medida.tokens.map_or(pct / 100.0 * janela, |t| t as f64);
    let filhos = s.db.panes_by_parent(&row.id).unwrap_or_default();
    let ajudantes = filhos.iter().any(|f| matches!(s.status_of(&f.id), Some(PaneStatus::Working | PaneStatus::Starting)));
    let (marco, turnos_marco, em_cooldown) = match s.rt().ctx.compact.marco.get(&row.id).copied() {
        Some(mc) => (mc.em_ms, mc.turnos, now.duration_since(mc.quando) < COOLDOWN_AUTO),
        None => (row.created_at, 0, false),
    };
    let turnos_desde_acao = m.turns - turnos_marco;
    // todo papel exige um evento de fronteira depois do marco (e 5 turnos desde a última ação, para o de usuário)
    let (fronteira, pendentes) = match row.role.as_deref() {
        None => (pct >= PROPOR_ACIMA_PCT && turnos_desde_acao >= CICLO_TURNOS, 0),
        Some("piloto") => (!ajudantes && s.db.filhos_fechados_desde(&row.id, marco).unwrap_or(0) > 0, 0),
        // [inferido] fronteira do planejador: plano aprovado, nenhuma tarefa pendente, e uma tarefa concluída depois do marco
        Some("planner") => match row.mission_id.as_deref().and_then(|m| planexec::aprovado_pendentes(s, m).map(|p| (p, m))) {
            Some((p, missao)) => {
                let tarefa_depois = s.db.tasks(missao).is_ok_and(|ts| ts.iter().any(|t| t.updated_at > marco));
                (p == 0 && tarefa_depois, p as u64)
            }
            None => (false, 0),
        },
        // [inferido] fronteira de fase: algum gate do maestro decidido desde o último marco
        Some("maestro") => (s.db.gates_decididos_desde(&row.id, marco).unwrap_or(0) > 0, 0),
        _ => (false, 0),
    };
    let mission = row.mission_id.as_deref().and_then(|m| s.db.get_mission(m).ok());
    let brief = mission.map_or_else(|| row.name.clone(), |mi| mi.brief.unwrap_or(mi.name));
    // os arquivos alterados (um git status) só entram quando a decisão é compactar: ver `decidir_e_agir`
    let arquivos = if manual { crate::verificacao::arquivos_alterados(Path::new(&row.cwd)) } else { vec![] };
    let cache_ttl = if perfil.cache_ttl_longos == "5m" { Duration::from_secs(300) } else { Duration::from_secs(3600) };
    let ctx = PaneCtx {
        papel: row.role.clone(),
        tipo_tarefa: row.task_type.clone(),
        pct,
        ctx_tokens,
        prefixo: m.first_turn_tokens.unwrap_or(0) as f64,
        turnos: m.turns,
        ocioso: s.status_of(&row.id) == Some(PaneStatus::Idle),
        ultima_entrada: s.panes.last_input(&row.id),
        digitando: s.panes.typed_since_enter(&row.id),
        ultima_atividade: s.panes.activity(&row.id).and_then(|(_, o)| o),
        cache_ttl,
        fila: s.rt().pending_input.get(&row.id).is_some_and(|b| !b.is_empty()),
        gate: s.db.open_gates_of_pane(&row.id).is_ok_and(|g| !g.is_empty()),
        fronteira,
        ajudantes_rodando: ajudantes,
        pendentes_plano: pendentes,
        brief,
        arquivos,
        turnos_desde_acao,
        em_cooldown,
    };
    (ctx, perfil)
}

fn hist_de(s: &AppState, row: &PaneRow) -> Hist {
    let (desligado, aprendido) = {
        let cur = lk(&s.settings);
        (cur.auto_ajuste.modo() == "desligado", cur.auto_ajuste.aprendido.clone())
    };
    // em `desligado` o que foi aprendido não entra nas decisões; a calibração da sessão (medida agora) entra
    let (k, fator, r) = if desligado { (None, None, None) } else { (aprendido.k, aprendido.fator_restante, aprendido.r_resumo) };
    Hist {
        turnos_fechados: s.db.turnos_fechados(row.role.as_deref(), row.task_type.as_deref()).unwrap_or_default(),
        // a calibração da sessão vale primeiro; sem ela, a razão persistida
        razao_resumo: s.rt().ctx.compact.razao.or(r),
        k_aprendido: k,
        fator_restante: fator,
    }
}

/// Brief de retomada do painel: missão, quadro de tarefas, últimas entregas, plano e próximo passo (dados, não instruções).
fn brief_de(s: &AppState, row: &PaneRow) -> String {
    let Some(missao) = row.mission_id.as_deref() else { return brief_retomada(&row.name, &[], &[], None, "continuar o trabalho") };
    let Ok(m) = s.db.get_mission(missao) else { return brief_retomada(&row.name, &[], &[], None, "continuar o trabalho") };
    let tarefas = s.db.tasks(missao).unwrap_or_default();
    let quadro: Vec<(String, String)> = tarefas.iter().map(|t| (t.id.clone(), t.status.clone())).collect();
    let proximo = tarefas.iter().find(|t| matches!(t.status.as_str(), "em_execucao" | "pendente")).map_or_else(|| "continuar o trabalho".to_string(), |t| t.title.clone());
    let resumos: Vec<String> = s.db.handoffs(Some(missao), None, 5).unwrap_or_default().into_iter().map(|h| h.summary).collect();
    let arquivo = planexec::plan_get(s, missao).ok().and_then(|v| v["arquivo"].as_str().map(String::from));
    brief_retomada(&m.brief.unwrap_or(m.name), &quadro, &resumos, arquivo.as_deref(), &proximo)
}

fn registrar(s: &AppState, row: &PaneRow, kind: &'static str, acao: &'static str, motivo: String, est: Option<Value>) -> Option<String> {
    let novo = ajustes::Novo {
        workspace_id: Some(row.workspace_id.clone()),
        mission_id: row.mission_id.clone(),
        pane_id: Some(row.id.clone()),
        kind,
        acao,
        motivo,
        antes: None,
        depois: None,
        estimativa: est,
        desfazivel: false,
    };
    match ajustes::registrar(s, novo) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("ajuste de compactação de {} não registrado: {e}", row.id);
            None
        }
    }
}

/// Avisa a interface que a proposta do painel mudou (`acao` nulo = sem proposta).
fn avisar_proposta(s: &AppState, row: &PaneRow, acao: Option<&str>, motivo: &str) {
    s.emit("compact-proposta", json!({"workspaceId": row.workspace_id, "paneId": row.id, "acao": acao, "motivo": motivo}));
}

fn limpar_proposta(s: &AppState, row: &PaneRow) {
    if s.rt().ctx.compact.sem_proposta(&row.id) {
        avisar_proposta(s, row, None, "");
    }
}

fn executar_compactar(s: &AppState, row: &PaneRow, ctx: &PaneCtx, motivo: String, est: Option<Value>, cr: f64) -> AppResult<()> {
    let foco = foco_text(&ctx.brief, &ctx.arquivos);
    s.write_input(&row.id, &bracketed(&format!("/compact {foco}")))?;
    limpar_proposta(s, row);
    // o marco vale também para a compactação: a próxima fronteira precisa de um evento depois dele
    s.rt().ctx.compact.marcar(&row.id, now_ms(), ctx.turnos, Instant::now());
    if let Some(ajuste) = registrar(s, row, "compactacao", "aplicado", motivo, est) {
        let tokens = row.tok_in + row.tok_cache_r + row.tok_cache_w;
        s.rt().ctx.compact.efeitos.insert(row.id.clone(), Efeito { ajuste, turnos: ctx.turnos, custo: row.cost_usd, tokens, ctx: ctx.ctx_tokens, cr });
    }
    Ok(())
}

fn executar_recomeco(s: &AppState, row: &PaneRow, turnos: i64, motivo: String, est: Option<Value>) -> AppResult<()> {
    let brief = brief_de(s, row);
    s.write_input(&row.id, &bracketed("/clear"))?;
    limpar_proposta(s, row);
    let agora = Instant::now();
    // marco já no /clear: o cooldown e os 5 turnos contam daqui, não só depois do brief colado
    s.rt().ctx.compact.marcar(&row.id, now_ms(), turnos, agora);
    s.rt().ctx.compact.aguarda_limpeza.insert(row.id.clone(), (agora, brief));
    registrar(s, row, "recomeco", "aplicado", motivo, est);
    Ok(())
}

/// O que a decisão faz: "Pular" some (com throttle), "Propor" vai ao chip, compactar e recomeçar escrevem no PTY.
fn aplicar(s: &AppState, row: &PaneRow, ctx: &PaneCtx, d: &Decisao, motivo_acao: String, est: Option<Value>, cr: f64, now: Instant) {
    let r = match d {
        Decisao::Pular { motivo } => {
            limpar_proposta(s, row);
            if ctx.ctx_tokens >= REGISTRA_PULAR_ACIMA && s.rt().ctx.compact.pular_pode(&row.id, now) {
                registrar(s, row, "compactacao", "pulado", motivo.clone(), est);
            }
            Ok(())
        }
        Decisao::Propor { acao, motivo } => {
            let kind = if *acao == "recomecar" { "recomeco" } else { "compactacao" };
            // o chip sempre acompanha a mudança; o feed registra no máximo uma proposta por painel a cada 10 min
            // (uma troca de ação conta como mudança e entra sempre)
            let mudou = s.rt().ctx.compact.propor(&row.id, acao);
            if mudou {
                avisar_proposta(s, row, Some(acao), motivo);
            }
            let pode = s.rt().ctx.compact.proposta_feed_pode(&row.id, now);
            if mudou || pode {
                registrar(s, row, kind, "proposto", motivo.clone(), est);
            }
            Ok(())
        }
        Decisao::Compactar { .. } => executar_compactar(s, row, ctx, motivo_acao, est, cr),
        Decisao::Recomecar { .. } => executar_recomeco(s, row, ctx.turnos, motivo_acao, est),
    };
    if let Err(e) = r {
        tracing::warn!("ação de compactação em {} não enviada: {e}", row.id);
    }
}

/// Arquivos alterados com prazo de 2 s, numa thread curta: um `git status` lento não segura o tick. Estourou: a
/// compactação segue sem a lista de arquivos (o foco fica genérico).
fn arquivos_com_prazo(cwd: &Path) -> Vec<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let cwd = cwd.to_path_buf();
    let _ = std::thread::Builder::new().name("regente-arquivos".into()).spawn(move || {
        let _ = tx.send(crate::verificacao::arquivos_alterados(&cwd));
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default()
}

/// Preço do modelo do painel; no "padrão do CLI" (sem modelo próprio), o do modelo que a statusline informou.
fn preco_do_painel(s: &AppState, row: &PaneRow, medida: &Medida) -> AppResult<PriceEntry> {
    let modelo = row.model.as_deref().or(medida.modelo.as_deref());
    modelo.and_then(|m| lk(&s.prices).find(m).cloned()).ok_or_else(|| AppError::new("sem_preco", "o modelo do painel não tem preço cadastrado"))
}

/// Um painel: monta, decide e age.
fn decidir_e_agir(s: &AppState, row: &PaneRow, medida: &Medida, now: Instant) -> AppResult<()> {
    let (mut ctx, perfil) = montar(s, row, medida, false);
    let preco = preco_do_painel(s, row, medida)?;
    let hist = hist_de(s, row);
    let mut d = decide(&ctx, &perfil.contexto, &preco, &hist, now);
    // só a compactação usa os arquivos alterados (no foco): um `git status` por decisão de compactar, nunca por painel
    if matches!(d, Decisao::Compactar { .. }) {
        ctx.arquivos = arquivos_com_prazo(Path::new(&row.cwd));
        d = Decisao::Compactar { foco: foco_text(&ctx.brief, &ctx.arquivos) };
    }
    let c = conta(&ctx, &preco, &hist, perfil.contexto.payback_k, now);
    let est = c.as_ref().map(|c| json!({"C": c.c, "G": c.g, "payback": c.payback, "R": c.r, "S": c.s, "K": c.k, "turnos": ctx.turnos}));
    let motivo_acao = c.as_ref().map_or_else(String::new, |c| format!("payback {} turnos; restam ~{:.0} turnos", dec(c.payback), c.r));
    aplicar(s, row, &ctx, &d, motivo_acao, est, preco.cache_read, now);
    Ok(())
}

/// Tick (a cada 30 s, no ticker): efeitos, recomeços pendentes e a decisão de cada painel Claude com medida recente.
pub fn tick(s: &Arc<AppState>) {
    let now = Instant::now();
    efeitos_tick(s);
    pastas_tick(s, now);
    for id in s.panes.ids() {
        let Ok(row) = s.db.get_pane(&id) else { continue };
        if row.kind != "agent" || row.cli.as_deref() != Some("claude") || row.closed_at.is_some() {
            continue;
        }
        let Some(medida) = s.rt().ctx.atual.get(&id).cloned() else { continue };
        if medida.quando.is_none_or(|t| now.duration_since(t) > FRESCOR_MEDIDA) {
            continue;
        }
        match decidir_e_agir(s, &row, &medida, now) {
            // modelo sem preço é estado, não falha: repetir o aviso a cada 30 s só enche o log
            Err(e) if e.code() == "sem_preco" => tracing::debug!("decisão de compactação de {id} não tomada: {e}"),
            Err(e) => tracing::warn!("decisão de compactação de {id} não tomada: {e}"),
            Ok(()) => {}
        }
    }
}

/// Recomeço pendente: o brief entra quando o painel fica ocioso (sem digitação) e depois de uma pausa.
fn pastas_tick(s: &AppState, now: Instant) {
    let pendentes: Vec<(String, Instant, String)> = s.rt().ctx.compact.aguarda_limpeza.iter().map(|(k, (t, b))| (k.clone(), *t, b.clone())).collect();
    for (id, desde, brief) in pendentes {
        let fechado = s.db.get_pane(&id).map_or(true, |r| r.closed_at.is_some());
        if fechado || now.duration_since(desde) > PRAZO_BRIEF {
            s.rt().ctx.compact.aguarda_limpeza.remove(&id);
            continue;
        }
        if now.duration_since(desde) < PAUSA_ANTES_BRIEF {
            continue;
        }
        // os mesmos bloqueios do tick (ocioso, digitação, fila, gate), menos a regra dos 30 s: o /clear é nosso
        let bloqueado = bloqueio_de(
            s.status_of(&id) == Some(PaneStatus::Idle),
            s.panes.typed_since_enter(&id),
            None,
            s.rt().pending_input.get(&id).is_some_and(|b| !b.is_empty()),
            s.db.open_gates_of_pane(&id).is_ok_and(|g| !g.is_empty()),
            now,
        );
        if bloqueado.is_some() {
            continue;
        }
        if s.write_input(&id, &bracketed(&brief)).is_ok() {
            let turnos = s.db.pane_metrics(&id).ok().flatten().map_or(0, |m| m.turns);
            s.rt().ctx.compact.aguarda_limpeza.remove(&id);
            s.rt().ctx.compact.marcar(&id, now_ms(), turnos, now);
            let _ = s.db.metric_bump(&id, "fresh_starts");
        }
    }
}

/// Mede o efeito das ações: depois de 5 turnos, custo e tokens reais contra o que a conta previa (N·ctx·cr).
fn efeitos_tick(s: &AppState) {
    let ids: Vec<String> = s.rt().ctx.compact.efeitos.keys().cloned().collect();
    for id in ids {
        let Some(e) = s.rt().ctx.compact.efeitos.get(&id).cloned() else { continue };
        let Ok(row) = s.db.get_pane(&id) else {
            s.rt().ctx.compact.efeitos.remove(&id);
            continue;
        };
        let turnos = s.db.pane_metrics(&id).ok().flatten().map_or(0, |m| m.turns);
        let tokens = row.tok_in + row.tok_cache_r + row.tok_cache_w;
        let Some(json) = fecha_efeito(&e, turnos, row.cost_usd, tokens, now_ms()) else { continue };
        s.rt().ctx.compact.efeitos.remove(&id);
        if let Err(err) = s.db.set_ajuste_efeito(&e.ajuste, &json.to_string()) {
            tracing::warn!("efeito da ação em {id} não gravado: {err}");
        }
    }
}

/// O efeito medido, quando já passaram `EFEITO_TURNOS` turnos desde a ação. Puro, para teste.
fn fecha_efeito(e: &Efeito, turnos_agora: i64, custo_agora: f64, tokens_agora: i64, agora_ms: i64) -> Option<Value> {
    if turnos_agora < e.turnos + EFEITO_TURNOS {
        return None;
    }
    let n = EFEITO_TURNOS as f64;
    Some(json!({
        "turnos": EFEITO_TURNOS,
        "custoReal": (custo_agora - e.custo).max(0.0),
        "custoSemAcao": n * e.ctx * e.cr / 1e6,
        "tokensReais": (tokens_agora - e.tokens).max(0),
        "tokensSemAcao": (n * e.ctx) as i64,
        "medidoEm": agora_ms,
    }))
}

/// Ação pedida pelo usuário no chip ("Compactar" ou "Recomeçar com memória"). As mesmas travas do tick: ocioso, sem
/// tecla nos últimos 30 s, fila vazia, sem gate.
pub fn acao_manual(s: &AppState, id: &str, recomeco: bool) -> AppResult<()> {
    let row = s.db.get_pane(id)?;
    let medida = s.rt().ctx.atual.get(id).cloned();
    let Some(medida) = medida else { return fail("sem_medida", "não há medida de contexto deste painel") };
    let now = Instant::now();
    let (ctx, perfil) = montar(s, &row, &medida, true);
    if let Some(m) = bloqueio(&ctx, now) {
        return fail("bloqueado", m);
    }
    let preco = preco_do_painel(s, &row, &medida)?;
    let hist = hist_de(s, &row);
    let c = conta(&ctx, &preco, &hist, perfil.contexto.payback_k, now);
    let est = c.as_ref().map(|c| json!({"C": c.c, "G": c.g, "payback": c.payback, "R": c.r, "S": c.s, "K": c.k, "turnos": ctx.turnos}));
    let motivo = "pedido pelo usuário".to_string();
    if recomeco {
        executar_recomeco(s, &row, ctx.turnos, motivo, est)
    } else {
        executar_compactar(s, &row, &ctx, motivo, est, preco.cache_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opus() -> PriceEntry {
        PriceEntry { pattern: "opus".into(), input: 4.0, output: 20.0, cache_read: 0.2, cache_write: 8.0 }
    }

    #[test]
    fn painel_no_padrao_do_cli_usa_o_modelo_da_statusline() {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        *lk(&s.prices) = crate::cost::prices::Prices { models: vec![opus()] };
        let row = PaneRow::default();
        let mut m = Medida::default();
        assert_eq!(preco_do_painel(&s, &row, &m).unwrap_err().code(), "sem_preco");
        m.modelo = Some("claude-opus-5-5".into());
        assert_eq!(preco_do_painel(&s, &row, &m).unwrap(), opus());
        // o modelo próprio do painel vence o da statusline
        let row = PaneRow { model: Some("claude-haiku-5-5".into()), ..Default::default() };
        assert_eq!(preco_do_painel(&s, &row, &m).unwrap_err().code(), "sem_preco");
    }

    /// Instante de referência: o painel está ocioso há horas, cache quente, sem tecla recente.
    fn agora() -> Instant {
        Instant::now() + Duration::from_secs(10_000)
    }

    fn painel(now: Instant) -> PaneCtx {
        PaneCtx {
            papel: Some("piloto".into()),
            tipo_tarefa: Some("backend".into()),
            pct: 60.0,
            ctx_tokens: 300_000.0,
            prefixo: 25_000.0,
            turnos: 0,
            ocioso: true,
            ultima_entrada: Some(now - Duration::from_secs(3600)),
            digitando: false,
            ultima_atividade: Some(now - Duration::from_secs(1)),
            cache_ttl: Duration::from_secs(3600),
            fila: false,
            gate: false,
            fronteira: false,
            ajudantes_rodando: false,
            pendentes_plano: 0,
            brief: "Implementar o medidor".into(),
            arquivos: vec![],
            turnos_desde_acao: 10,
            em_cooldown: false,
        }
    }

    #[test]
    fn recomeco_nao_se_repete_sem_novo_evento_nem_cooldown() {
        // planner com plano concluído e fronteira verdadeira: o primeiro recomeço sai
        let now = agora();
        let mut p = painel(now);
        p.papel = Some("planner".into());
        p.fronteira = true;
        p.turnos_desde_acao = 10;
        assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Recomecar { .. }));
        // depois do recomeço: marco novo (0 turnos desde a ação, em cooldown). A fronteira ainda vale, mas não age
        p.turnos_desde_acao = 0;
        p.em_cooldown = true;
        for _ in 0..3 {
            assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Pular { .. }));
        }
        // cooldown passou mas ainda sem 5 turnos: continua parado
        p.em_cooldown = false;
        p.turnos_desde_acao = 4;
        assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Pular { .. }));
        // cinco turnos depois, a fronteira volta a agir
        p.turnos_desde_acao = 5;
        assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Recomecar { .. }));
    }

    #[test]
    fn fronteira_so_vale_com_prefixo_medido_e_contexto_de_50k() {
        let now = agora();
        let mut p = painel(now);
        p.papel = Some("planner".into());
        p.fronteira = true;
        p.prefixo = 0.0;
        assert!(!matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Recomecar { .. }));
        p.prefixo = 25_000.0;
        p.ctx_tokens = 40_000.0;
        assert!(!matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Recomecar { .. }));
    }

    #[test]
    fn brief_nunca_corta_o_proximo_passo() {
        let longo = "x".repeat(5000);
        let tarefas: Vec<(String, String)> = (0..40).map(|i| (format!("T{i} {longo}"), "pendente".into())).collect();
        let b = brief_retomada(&longo, &tarefas, &[longo.clone()], Some("plano.md"), "PASSO FINAL");
        assert!(b.chars().count() <= 2500);
        assert!(b.ends_with("Próximo passo: PASSO FINAL\n"), "{b}");
    }

    #[test]
    fn feed_de_proposta_uma_vez_por_10_min() {
        let mut e = Estado::default();
        let t0 = agora();
        assert!(e.proposta_feed_pode("p", t0));
        assert!(!e.proposta_feed_pode("p", t0 + Duration::from_secs(60)));
        assert!(e.proposta_feed_pode("p", t0 + Duration::from_secs(601)));
    }

    fn cfg() -> Contexto {
        Contexto::default()
    }

    fn motivo(d: &Decisao) -> String {
        match d {
            Decisao::Pular { motivo } | Decisao::Propor { motivo, .. } => motivo.clone(),
            other => panic!("sem motivo em {other:?}"),
        }
    }

    #[test]
    fn papel_curto_nunca_compacta() {
        let now = agora();
        let mut p = painel(now);
        p.papel = Some("executor".into());
        p.fronteira = true;
        assert_eq!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Pular { motivo: "papel de vida curta".into() });
    }

    #[test]
    fn bloqueios_de_ociosidade() {
        let now = agora();
        let casos: Vec<(Box<dyn Fn(&mut PaneCtx)>, &str)> = vec![
            (Box::new(|p| p.ocioso = false), "não está ocioso"),
            (Box::new(|p| p.digitando = true), "digitando"),
            (Box::new(move |p| p.ultima_entrada = Some(now - Duration::from_secs(5))), "menos de 30 s"),
            (Box::new(|p| p.fila = true), "fila"),
            (Box::new(|p| p.gate = true), "gate"),
        ];
        for (ajusta, esperado) in casos {
            let mut p = painel(now);
            p.fronteira = true;
            ajusta(&mut p);
            let d = decide(&p, &cfg(), &opus(), &Hist::default(), now);
            assert!(motivo(&d).contains(esperado), "{d:?} deveria citar {esperado}");
        }
    }

    #[test]
    fn piloto_na_fronteira_recomeca_no_auto_e_propoe_no_propor() {
        let now = agora();
        let mut p = painel(now);
        p.fronteira = true;
        assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Recomecar { .. }));
        let mut propor = cfg();
        propor.recomecar = "propor".into();
        assert!(matches!(decide(&p, &propor, &opus(), &Hist::default(), now), Decisao::Propor { acao: "recomecar", .. }));
    }

    #[test]
    fn planner_e_maestro_na_fronteira_recomecam_no_auto() {
        let now = agora();
        for papel in ["planner", "maestro"] {
            let mut p = painel(now);
            p.papel = Some(papel.into());
            p.fronteira = true;
            assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Recomecar { .. }), "{papel}");
        }
    }

    #[test]
    fn painel_do_usuario_so_propoe() {
        let now = agora();
        let mut p = painel(now);
        p.papel = None;
        p.fronteira = true;
        assert!(matches!(decide(&p, &cfg(), &opus(), &Hist::default(), now), Decisao::Propor { acao: "recomecar", .. }));
        // sem fronteira, mas a conta fecha: também só propõe
        let mut conta_ok = painel(now);
        conta_ok.papel = None;
        let mut k10 = cfg();
        k10.payback_k = 10;
        let hist = Hist { turnos_fechados: vec![30; 5], ..Default::default() };
        assert!(matches!(decide(&conta_ok, &k10, &opus(), &hist, now), Decisao::Propor { acao: "compactar", .. }));
    }

    #[test]
    fn k_e_fator_aprendidos_valem_sobre_o_perfil() {
        let now = agora();
        let p = painel(now);
        // K aprendido alto deixa passar o payback de 9,6 que o K 3 do perfil recusa
        // o delta +5 leva o K de 3 até o teto de 8 (o payback de 9,6 ainda recusa, e a mensagem mostra o K usado)
        let aprendido = Hist { k_aprendido: Some(5.0), ..Default::default() };
        assert!(motivo(&decide(&p, &cfg(), &opus(), &aprendido, now)).contains("> K=8"));
        assert!(motivo(&decide(&p, &cfg(), &opus(), &Hist::default(), now)).contains("> K=3"));
        // fator 2 dobra os turnos restantes estimados
        let c1 = conta(&p, &opus(), &Hist::default(), 3, now).unwrap();
        let c2 = conta(&p, &opus(), &Hist { fator_restante: Some(2.0), ..Default::default() }, 3, now).unwrap();
        assert!((c2.r - 2.0 * c1.r).abs() < 1e-9, "{} vs {}", c2.r, c1.r);
    }

    #[test]
    fn exemplo_do_roadmap_pula_com_payback_de_9_6() {
        let now = agora();
        let p = painel(now);
        let c = conta(&p, &opus(), &Hist::default(), 3, now).unwrap();
        assert!((c.c - 0.512).abs() < 1e-6, "C={}", c.c);
        assert!((c.g - 0.0532).abs() < 1e-6, "G={}", c.g);
        assert!((c.payback - 9.6).abs() < 0.05, "payback={}", c.payback);
        assert_eq!(c.s, 9000.0);
        let d = decide(&p, &cfg(), &opus(), &Hist::default(), now);
        assert!(motivo(&d).contains("payback 9,6 turnos > K=3"), "{d:?}");
    }

    #[test]
    fn compacta_quando_a_conta_fecha_e_ha_trabalho_pela_frente() {
        let now = agora();
        let p = painel(now);
        let mut k10 = cfg();
        k10.payback_k = 10;
        let hist = Hist { turnos_fechados: vec![30; 5], ..Default::default() };
        match decide(&p, &k10, &opus(), &hist, now) {
            Decisao::Compactar { foco } => assert!(foco.starts_with("Foque em: Implementar o medidor.")),
            outro => panic!("esperava Compactar, veio {outro:?}"),
        }
    }

    #[test]
    fn payback_no_limite_mas_restante_pequeno_pula() {
        let now = agora();
        let p = painel(now);
        let mut k10 = cfg();
        k10.payback_k = 10;
        // R = 10 (padrão) < payback + 1 = 10,6
        let d = decide(&p, &k10, &opus(), &Hist::default(), now);
        assert!(motivo(&d).contains("restante de 10 turnos"), "{d:?}");
    }

    #[test]
    fn cache_frio_usa_preco_de_entrada() {
        let now = agora();
        let mut p = painel(now);
        p.ultima_atividade = Some(now - Duration::from_secs(7200));
        let c = conta(&p, &opus(), &Hist::default(), 3, now).unwrap();
        // C = 300k·4 + 9k·20 + 34k·8, em US$/M
        assert!((c.c - 1.652).abs() < 1e-6, "C={}", c.c);
        assert!((c.g - 1.064).abs() < 1e-6, "G={}", c.g);
    }

    #[test]
    fn planner_pendentes_do_plano_contam_no_restante() {
        let now = agora();
        let mut p = painel(now);
        p.papel = Some("planner".into());
        p.pendentes_plano = 7;
        let c = conta(&p, &opus(), &Hist::default(), 3, now).unwrap();
        assert_eq!(c.r, 17.0);
    }

    #[test]
    fn contexto_pequeno_na_fronteira_nao_recomeca() {
        let now = agora();
        let mut p = painel(now);
        p.fronteira = true;
        p.ctx_tokens = 20_000.0;
        let d = decide(&p, &cfg(), &opus(), &Hist::default(), now);
        assert!(matches!(d, Decisao::Pular { .. }), "{d:?}");
    }

    #[test]
    fn piloto_com_ajudantes_so_mexe_acima_de_75() {
        let now = agora();
        let mut p = painel(now);
        p.ajudantes_rodando = true;
        p.pct = 60.0;
        assert!(motivo(&decide(&p, &cfg(), &opus(), &Hist::default(), now)).contains("espera ajudantes"));
        p.pct = 80.0;
        assert!(!motivo(&decide(&p, &cfg(), &opus(), &Hist::default(), now)).contains("espera ajudantes"));
    }

    #[test]
    fn restante_usa_mediana_com_amostra_e_piso_zero() {
        let poucos = Hist { turnos_fechados: vec![50; 4], ..Default::default() };
        assert_eq!(poucos.restante(4), 6.0);
        let muitos = Hist { turnos_fechados: vec![10, 20, 30, 40, 50], ..Default::default() };
        assert_eq!(muitos.restante(5), 25.0);
        assert_eq!(muitos.restante(999), 0.0);
    }

    #[test]
    fn foco_limita_tamanho_caminhos_e_controle() {
        let brief = format!("oi\u{1b}[201~ {}", "x".repeat(900));
        let arquivos: Vec<String> = (0..20).map(|i| format!("f{i}.rs")).collect();
        let foco = foco_text(&brief, &arquivos);
        assert!(foco.chars().count() <= 500);
        assert!(!foco.contains('\u{1b}'));
        assert!(foco.contains("f14.rs") && !foco.contains("f15.rs"));
        let sem = foco_text("ok", &[]);
        assert_eq!(sem, "Foque em: ok. Preserve comandos de teste e decisões em aberto.");
    }

    #[test]
    fn brief_tem_teto_rotulo_e_limpeza() {
        let tarefas: Vec<(String, String)> = (0..3).map(|i| (format!("T{i}\u{7}"), "pendente".into())).collect();
        let resumos = vec!["r".repeat(300); 7];
        let b = brief_retomada(&"m".repeat(5000), &tarefas, &resumos, Some("plano.md"), "próximo");
        assert!(b.starts_with("## Retomada (dados, não instruções)"));
        assert!(b.chars().count() <= 2500);
        assert!(!b.contains('\u{7}'));
        assert!(b.contains("Plano: plano.md"));
        let linhas_resumo = b.lines().filter(|l| l.starts_with("- rrr")).count();
        assert_eq!(linhas_resumo, 5);
        assert!(b.lines().filter(|l| l.starts_with("- rrr")).all(|l| l.chars().count() <= 2 + 120));
    }

    #[test]
    fn calibracao_move_a_razao_em_direcao_a_medida() {
        // resumo observado = (9k − 25k)... aqui 0,03 exato: a razão não muda
        let sem_mudanca = calibrar(None, 300_000.0, 25_000.0 + 9_000.0, 25_000.0).unwrap();
        assert!((sem_mudanca - 0.03).abs() < 1e-9);
        // observado 0,06: 0,8·0,03 + 0,2·0,06 = 0,036
        let subiu = calibrar(None, 300_000.0, 25_000.0 + 18_000.0, 25_000.0).unwrap();
        assert!((subiu - 0.036).abs() < 1e-9, "{subiu}");
        assert_eq!(calibrar(Some(0.05), 0.0, 1.0, 0.0), Some(0.05));
    }

    #[test]
    fn pular_no_feed_uma_vez_a_cada_dez_minutos() {
        let now = agora();
        let mut e = Estado::default();
        assert!(e.pular_pode("p", now));
        assert!(!e.pular_pode("p", now + Duration::from_secs(599)));
        assert!(e.pular_pode("p", now + Duration::from_secs(601)));
        assert!(e.pular_pode("q", now), "throttle é por painel");
    }

    #[test]
    fn proposta_so_avisa_quando_muda() {
        let mut e = Estado::default();
        assert!(e.propor("p", "compactar"));
        assert!(!e.propor("p", "compactar"), "a mesma proposta não se repete");
        assert!(e.propor("p", "recomecar"));
        assert!(e.sem_proposta("p"));
        assert!(!e.sem_proposta("p"));
    }

    #[test]
    fn efeito_so_fecha_depois_de_cinco_turnos_e_compara_com_a_conta() {
        let e = Efeito { ajuste: "a1".into(), turnos: 10, custo: 2.0, tokens: 1_000, ctx: 300_000.0, cr: 0.2 };
        assert!(fecha_efeito(&e, 14, 2.5, 9_000, 0).is_none(), "4 turnos ainda não");
        let v = fecha_efeito(&e, 15, 2.5, 9_000, 42).expect("5 turnos fecham");
        assert_eq!(v["turnos"], 5);
        assert!((v["custoReal"].as_f64().unwrap() - 0.5).abs() < 1e-9);
        // sem ação: 5 turnos relendo 300k a 0,2 US$/M
        assert!((v["custoSemAcao"].as_f64().unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(v["tokensReais"], 8_000);
        assert_eq!(v["tokensSemAcao"], 1_500_000);
        assert_eq!(v["medidoEm"], 42);
    }

    #[test]
    fn compactar_escreve_exatamente_um_comando_entre_colchetes() {
        // o que o tick escreve no PTY: /compact com o foco, num paste com o Enter no fim
        let foco = foco_text("Implementar o medidor", &["src/a.rs".to_string()]);
        let bytes = crate::orchestrator::bracketed(&format!("/compact {foco}"));
        let esperado = format!("\u{1b}[200~/compact {foco}\u{1b}[201~\r");
        assert_eq!(bytes, esperado.into_bytes());
    }

    #[test]
    fn recomeco_escreve_clear_e_depois_o_brief() {
        assert_eq!(crate::orchestrator::bracketed("/clear"), b"\x1b[200~/clear\x1b[201~\r".to_vec());
        let brief = brief_retomada("missão", &[], &[], None, "próximo");
        assert!(crate::orchestrator::bracketed(&brief).ends_with(b"\x1b[201~\r"));
    }
}
