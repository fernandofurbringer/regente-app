//! Continuar um painel do Claude em outra conta (o limite da janela estourou). A sessão é copiada para a pasta da
//! outra conta (`<config>/projects/<projeto>/<sessão>.jsonl` e a pasta `<sessão>/` ao lado, com subagentes e
//! resultados de ferramentas) e o painel reinicia retomando-a (`--resume`). Se a conversa parou no limite, o painel
//! recebe "continue" assim que acorda. O limite é lido do JSONL: a última entrada é o erro `rate_limit` da API.
use crate::cost::claude::find_session_file;
use crate::db::PaneRow;
use crate::error::{fail, AppResult};
use crate::orchestrator::bracketed;
use crate::runtime::AppState;
use crate::status::PaneStatus;
use crate::util::lk;
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// O fim do JSONL que basta para achar a última entrada (o erro de limite é curto).
const CAUDA: u64 = 64 * 1024;

/// O que o painel recebe ao acordar na conta nova, quando a conversa parou no limite.
pub const CONTINUAR: &str = "continue";

/// A conta como o painel a guarda: `None` é a principal.
fn conta(a: &str) -> Option<&str> {
    (!a.is_empty() && a != "principal").then_some(a)
}

/// O limite da conta na última entrada da conversa: `(uuid, texto)`. Shape real (Claude Code 2.1.314):
/// `{"type":"assistant","error":"rate_limit","isApiErrorMessage":true,"apiErrorStatus":429,"uuid":"…",
/// "message":{"content":[{"type":"text","text":"You've hit your 5-hour limit · resets 5:10pm (America/Sao_Paulo)"}]}}`.
/// Só a última entrada de usuário/assistente conta: um pedido novo atendido depois dele significa que o limite passou.
pub fn limite_no_fim(cauda: &str) -> Option<(String, String)> {
    let v = cauda
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| matches!(v["type"].as_str(), Some("user" | "assistant")))?;
    if v["isApiErrorMessage"].as_bool() != Some(true) || v["error"].as_str() != Some("rate_limit") {
        return None;
    }
    let texto = v["message"]["content"][0]["text"].as_str().unwrap_or("limite de uso atingido").to_string();
    Some((v["uuid"].as_str().unwrap_or_default().to_string(), texto))
}

/// Os últimos `CAUDA` bytes do arquivo (a primeira linha pode vir cortada: o parse a descarta).
fn ler_cauda(p: &Path) -> String {
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(len.saturating_sub(CAUDA))).is_ok() {
        let _ = f.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn modificado(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Copia a sessão `src` (o `.jsonl`) para a pasta de configuração `destino`, na mesma pasta de projeto, junto com a
/// pasta `<sessão>/` ao lado. A cópia fica com a data de modificação da origem: assim ela não parece mais nova que a
/// origem (ver `a_trazer`). Devolve o caminho novo do `.jsonl`.
pub fn copiar_sessao(src: &Path, destino: &Path) -> std::io::Result<PathBuf> {
    let fora = || std::io::Error::other("sessão fora de projects/<projeto>");
    let projeto = src.parent().and_then(Path::file_name).ok_or_else(fora)?;
    let nome = src.file_name().ok_or_else(fora)?;
    let pasta = destino.join("projects").join(projeto);
    std::fs::create_dir_all(&pasta)?;
    let novo = pasta.join(nome);
    let lado = src.with_extension("");
    if lado.is_dir() {
        copiar_pasta(&lado, &novo.with_extension(""))?;
    }
    // o `.jsonl` por último: com ele presente, a retomada acha a sessão inteira
    std::fs::copy(src, &novo)?;
    if let Some(m) = modificado(src) {
        std::fs::OpenOptions::new().write(true).open(&novo)?.set_modified(m)?;
    }
    Ok(novo)
}

/// Copia arquivos e subpastas (links simbólicos ficam de fora).
fn copiar_pasta(de: &Path, para: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(para)?;
    for e in std::fs::read_dir(de)? {
        let e = e?;
        let t = e.file_type()?;
        if t.is_dir() {
            copiar_pasta(&e.path(), &para.join(e.file_name()))?;
        } else if t.is_file() {
            std::fs::copy(e.path(), para.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Entre as cópias da sessão (pasta da conta, arquivo, modificação), a que deve ir para `alvo`: a mais nova, se não
/// estiver em `alvo` e se a de `alvo` (quando existe) for mais velha. A mais nova é a que a última conta usou.
pub fn a_trazer<'a>(alvo: &Path, copias: &'a [(PathBuf, PathBuf, SystemTime)]) -> Option<&'a (PathBuf, PathBuf, SystemTime)> {
    let nova = copias.iter().max_by_key(|c| c.2)?;
    if nova.0 == alvo || copias.iter().any(|c| c.0 == alvo && c.2 >= nova.2) {
        return None;
    }
    Some(nova)
}

impl AppState {
    /// As contas do Claude que o Regente conhece, com a pasta de cada uma: a principal e as extras.
    fn contas_claude(&self) -> Vec<(String, PathBuf)> {
        let extras = lk(&self.settings).accounts.get("claude").cloned().unwrap_or_default();
        std::iter::once("principal".to_string()).chain(extras).map(|a| {
            let dir = self.config_dir_for("claude", conta(&a));
            (a, dir)
        }).collect()
    }

    /// As cópias da sessão em todas as contas.
    fn copias_da_sessao(&self, sid: &str) -> Vec<(PathBuf, PathBuf, SystemTime)> {
        self.contas_claude()
            .into_iter()
            .filter_map(|(_, dir)| {
                let f = find_session_file(&dir, sid)?;
                let m = modificado(&f)?;
                Some((dir, f, m))
            })
            .collect()
    }

    /// Antes de retomar (`respawn`): se a cópia mais nova da sessão está em outra conta, traz para a conta do painel.
    /// O leitor de custo passa a ler a cópia de onde parou (o conteúdo é o mesmo byte a byte; nada conta duas vezes).
    pub(crate) fn trazer_sessao(&self, row: &PaneRow) {
        if row.cli.as_deref() != Some("claude") || crate::remote::is_remote(&row.cwd) {
            return;
        }
        let Some(sid) = row.session_id.as_deref() else { return };
        let alvo = self.config_dir_for("claude", row.account.as_deref());
        let copias = self.copias_da_sessao(sid);
        let Some((de, src, _)) = a_trazer(&alvo, &copias) else { return };
        let _serial = self.poll_guard();
        match copiar_sessao(src, &alvo) {
            Ok(_) => {
                if let Some(t) = self.rt().tailers.get_mut(&row.id) {
                    t.relocate_claude(de, &alvo);
                }
                tracing::info!("painel {}: sessão {sid} copiada para a conta {}", row.id, row.account.as_deref().unwrap_or("principal"));
            }
            Err(e) => tracing::warn!("painel {}: não deu para copiar a sessão {sid} para a outra conta: {e}", row.id),
        }
    }

    /// Passa o painel para `account` ("principal" ou uma conta extra do Claude). Rodando: para, troca a conta e acorda
    /// retomando a sessão (`frame` é a tela mostrada enquanto isso); se a conversa parou no limite, manda "continue".
    /// Hibernado ou encerrado: só troca a conta, que vale na próxima retomada.
    pub fn switch_account(&self, id: &str, account: &str, frame: &str) -> AppResult<PaneRow> {
        let row = self.db.get_pane(id)?;
        if row.kind != "agent" || row.cli.as_deref() != Some("claude") {
            return fail("bad_kind", "só painel do Claude troca de conta");
        }
        if crate::remote::is_remote(&row.cwd) {
            return fail("remote_unsupported", "painel remoto não troca de conta");
        }
        let alvo = conta(account);
        if let Some(a) = alvo {
            if !self.contas_claude().iter().any(|(c, _)| c == a) {
                return fail("unknown_account", format!("a conta {a} não está cadastrada (Configurações → IAs)"));
            }
        }
        if alvo == conta(row.account.as_deref().unwrap_or_default()) {
            return Ok(row);
        }
        let Some(sid) = row.session_id.clone() else {
            return fail("no_session", "este painel ainda não tem conversa para levar");
        };
        let copias = self.copias_da_sessao(&sid);
        // sem cópia gravada (nada foi mandado ainda): não há conversa a levar; o painel reinicia na outra conta e o
        // `respawn`, sem o arquivo da sessão, abre uma sessão nova lá
        let limitado = copias.iter().max_by_key(|c| c.2).is_some_and(|nova| limite_no_fim(&ler_cauda(&nova.1)).is_some());
        let rodando = self.panes.is_alive(id) && self.status_of(id) != Some(PaneStatus::Hibernated);
        if rodando {
            // para o processo; a cópia da sessão acontece na retomada, quando o processo antigo já saiu (`trazer_sessao`)
            self.hibernate(id, frame)?;
        }
        self.db.set_pane_account(id, alvo)?;
        self.rt().limit_warned.remove(id);
        if rodando {
            self.wake(id);
            if limitado {
                // acordando: o texto fica na fila e vai quando o CLI estiver pronto
                self.write_input(id, &bracketed(CONTINUAR))?;
            }
        }
        self.db.get_pane(id)
    }

    /// Painel do Claude parado no limite da conta: avisa a tela uma vez por erro (`account-limit`), com as outras contas.
    pub fn vigiar_limite(&self, id: &str) {
        let Ok(row) = self.db.get_pane(id) else { return };
        if row.kind != "agent" || row.cli.as_deref() != Some("claude") || crate::remote::is_remote(&row.cwd) {
            return;
        }
        let Some(sid) = row.session_id.as_deref() else { return };
        let Some(p) = find_session_file(&self.config_dir_for("claude", row.account.as_deref()), sid) else { return };
        let Some((uuid, texto)) = limite_no_fim(&ler_cauda(&p)) else { return };
        if self.rt().limit_warned.insert(id.to_string(), uuid.clone()).as_deref() == Some(uuid.as_str()) {
            return;
        }
        let atual = conta(row.account.as_deref().unwrap_or_default()).unwrap_or("principal").to_string();
        let outras: Vec<String> = self.contas_claude().into_iter().map(|(c, _)| c).filter(|c| *c != atual).collect();
        self.emit("account-limit", json!({"paneId": id, "paneName": row.name, "account": atual, "text": texto, "others": outras}));
    }

    /// A cada poucos segundos: os painéis ociosos do Claude (o limite deixa o painel ocioso, às vezes sem o `Stop`).
    pub fn tick_limites(&self) {
        let ociosos: Vec<String> = self.rt().statuses.iter().filter(|(_, s)| **s == PaneStatus::Idle).map(|(id, _)| id.clone()).collect();
        for id in ociosos {
            self.vigiar_limite(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // linhas reais do JSONL (Claude Code 2.1.314), encurtadas
    const LIMITE: &str = r#"{"type":"assistant","uuid":"u-lim","error":"rate_limit","isApiErrorMessage":true,"apiErrorStatus":429,"message":{"role":"assistant","content":[{"type":"text","text":"You've hit your 5-hour limit · resets 5:10pm (America/Sao_Paulo)"}]}}"#;
    const PEDIDO: &str = r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"faz o teste"}}"#;
    const RESPOSTA: &str = r#"{"type":"assistant","uuid":"u2","message":{"role":"assistant","content":[{"type":"text","text":"pronto"}]}}"#;

    #[test]
    fn o_limite_so_conta_quando_e_a_ultima_entrada() {
        let parou = format!("{PEDIDO}\n{LIMITE}\n{{\"type\":\"system\",\"subtype\":\"x\"}}\n");
        let (uuid, texto) = limite_no_fim(&parou).expect("limite");
        assert_eq!(uuid, "u-lim");
        assert!(texto.contains("5-hour limit"), "{texto}");
        // pedido novo depois do limite (outra conta, ou a janela virou): não está mais parado
        assert_eq!(limite_no_fim(&format!("{LIMITE}\n{PEDIDO}\n{RESPOSTA}\n")), None);
        assert_eq!(limite_no_fim(&format!("{LIMITE}\n{PEDIDO}\n")), None);
        // outro erro de API não é limite
        let outro = r#"{"type":"assistant","error":"server_error","isApiErrorMessage":true,"message":{"content":[{"type":"text","text":"API Error: 529 Overloaded"}]}}"#;
        assert_eq!(limite_no_fim(outro), None);
        // linha cortada no começo da cauda é descartada
        assert!(limite_no_fim(&format!("nal\":1}}\n{LIMITE}\n")).is_some());
        assert_eq!(limite_no_fim(""), None);
    }

    #[test]
    fn a_cauda_le_so_o_fim_do_arquivo() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.jsonl");
        let mut texto = format!("{PEDIDO}\n").repeat(2000); // > 64 KiB
        texto.push_str(LIMITE);
        texto.push('\n');
        std::fs::write(&p, &texto).unwrap();
        let cauda = ler_cauda(&p);
        assert!(cauda.len() as u64 <= CAUDA);
        assert!(limite_no_fim(&cauda).is_some());
        assert_eq!(ler_cauda(&d.path().join("nao-existe")), "");
    }

    #[test]
    fn copiar_leva_o_jsonl_e_a_pasta_ao_lado_com_a_mesma_data() {
        let d = tempfile::tempdir().unwrap();
        let de = d.path().join("contaA");
        let para = d.path().join("contaB");
        let proj = de.join("projects").join("D--w");
        std::fs::create_dir_all(proj.join("sid").join("subagents")).unwrap();
        std::fs::write(proj.join("sid.jsonl"), "linha\n").unwrap();
        std::fs::write(proj.join("sid").join("subagents").join("agent-1.jsonl"), "sub\n").unwrap();
        let antigo = SystemTime::now() - Duration::from_secs(3600);
        std::fs::OpenOptions::new().write(true).open(proj.join("sid.jsonl")).unwrap().set_modified(antigo).unwrap();

        let novo = copiar_sessao(&proj.join("sid.jsonl"), &para).unwrap();
        assert_eq!(novo, para.join("projects").join("D--w").join("sid.jsonl"));
        assert_eq!(std::fs::read_to_string(&novo).unwrap(), "linha\n");
        assert_eq!(std::fs::read_to_string(para.join("projects/D--w/sid/subagents/agent-1.jsonl")).unwrap(), "sub\n");
        assert_eq!(modificado(&novo), Some(antigo));
        // a retomada acha a sessão na conta nova
        assert_eq!(find_session_file(&para, "sid"), Some(novo));
    }

    #[test]
    fn traz_so_a_copia_mais_nova_de_outra_conta() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let c = |dir: &str, s: u64| (PathBuf::from(dir), PathBuf::from(format!("{dir}/x.jsonl")), t0 + Duration::from_secs(s));
        let a = Path::new("A");
        // só existe em B: traz
        let so_b = [c("B", 5)];
        assert_eq!(a_trazer(a, &so_b).map(|x| x.0.clone()), Some(PathBuf::from("B")));
        // A tem cópia velha (trocou A → B → A): traz a de B, que é mais nova
        let velha = [c("A", 1), c("B", 5)];
        assert_eq!(a_trazer(a, &velha).map(|x| x.0.clone()), Some(PathBuf::from("B")));
        // a de A é a mais nova, ou tem a mesma data (já copiada): nada a fazer
        assert!(a_trazer(a, &[c("A", 9), c("B", 5)]).is_none());
        assert!(a_trazer(a, &[c("A", 5), c("B", 5)]).is_none());
        assert!(a_trazer(a, &[]).is_none());
    }

    #[test]
    fn painel_parado_no_limite_avisa_uma_vez_e_troca_de_conta_levando_a_sessao() {
        use crate::util::new_id;
        use std::sync::{Arc, Mutex};
        let d = tempfile::tempdir().unwrap();
        let ev: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let e2 = ev.clone();
        let s = AppState::new(d.path().into(), PathBuf::from("C:/x/regente.exe"), Arc::new(move |n: &str, v: Value| lk(&e2).push((n.to_string(), v)))).unwrap();
        lk(&s.settings).accounts.insert("claude".into(), vec!["a".into(), "b".into()]);
        let sid = new_id(); // id único: a pasta principal (a real) não tem esta sessão
        let dir_a = s.config_dir_for("claude", Some("a"));
        let dir_b = s.config_dir_for("claude", Some("b"));
        let proj = dir_a.join("projects").join("C--w");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join(format!("{sid}.jsonl")), format!("{PEDIDO}
{LIMITE}
")).unwrap();
        s.db.insert_pane(&PaneRow { id: "p".into(), workspace_id: "w".into(), kind: "agent".into(), cli: Some("claude".into()), account: Some("a".into()), session_id: Some(sid.clone()), name: "p".into(), permission: "pend".into(), cwd: "C:/w".into(), status: "hibernado".into(), ..Default::default() }).unwrap();
        lk(&s.rt).statuses.insert("p".into(), PaneStatus::Hibernated);

        let avisos = || lk(&ev).iter().filter(|(n, _)| n == "account-limit").map(|(_, v)| v.clone()).collect::<Vec<_>>();
        s.vigiar_limite("p");
        s.vigiar_limite("p");
        let a = avisos();
        assert_eq!(a.len(), 1, "um aviso por erro");
        assert_eq!(a[0]["account"], "a");
        assert_eq!(a[0]["others"], json!(["principal", "b"]));
        assert!(a[0]["text"].as_str().unwrap().contains("limit"));

        assert_eq!(s.switch_account("p", "zz", "").unwrap_err().code(), "unknown_account");
        assert_eq!(s.switch_account("p", "a", "").unwrap().account.as_deref(), Some("a"), "mesma conta: nada muda");
        // hibernado: só troca a conta; a cópia vem na retomada
        let row = s.switch_account("p", "b", "").unwrap();
        assert_eq!(row.account.as_deref(), Some("b"));
        assert!(find_session_file(&dir_b, &sid).is_none());
        s.trazer_sessao(&row);
        let copia = find_session_file(&dir_b, &sid).expect("sessão na conta b");
        assert_eq!(copia, dir_b.join("projects").join("C--w").join(format!("{sid}.jsonl")));
        // de novo: a cópia tem a data da origem, então não copia outra vez
        s.trazer_sessao(&row);
        assert_eq!(copias_len(&s, &sid), 2);
        // principal também é aceita
        assert_eq!(s.switch_account("p", "principal", "").unwrap().account, None);
    }

    #[test]
    fn painel_sem_conversa_gravada_troca_de_conta_sem_pedir_mensagem() {
        use crate::util::new_id;
        use std::sync::Arc;
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), PathBuf::from("C:/x/regente.exe"), Arc::new(|_: &str, _: Value| {})).unwrap();
        lk(&s.settings).accounts.insert("claude".into(), vec!["b".into()]);
        // id único e nenhum .jsonl em conta nenhuma: o painel acabou de abrir e nada foi mandado
        s.db.insert_pane(&PaneRow { id: "p".into(), workspace_id: "w".into(), kind: "agent".into(), cli: Some("claude".into()), account: None, session_id: Some(new_id()), name: "p".into(), permission: "pend".into(), cwd: "C:/w".into(), status: "hibernado".into(), ..Default::default() }).unwrap();
        lk(&s.rt).statuses.insert("p".into(), PaneStatus::Hibernated);
        assert_eq!(s.switch_account("p", "b", "").unwrap().account.as_deref(), Some("b"));
    }

    fn copias_len(s: &AppState, sid: &str) -> usize {
        s.copias_da_sessao(sid).len()
    }
}
