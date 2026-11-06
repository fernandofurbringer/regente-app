//! Worktrees rápidos (X1.2): cache de build compartilhado por workspace, `node_modules` por hardlink do checkout
//! principal (junção opcional como plano B), venv reaproveitado (opt-in) e pré-aquecimento do build. Liga-se em
//! `projeto.json` (`cacheBuild`). Nada aqui pode falhar a criação da worktree: o ligamento roda numa thread curta
//! depois de criada.
use crate::db::{Mission, Workspace};
use crate::error::{fail, AppResult};
use crate::projeto;
use crate::providers;
use crate::runtime::AppState;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Acima disso, `node_modules` não é ligado: a cópia parcial é apagada e o worktree instala do zero.
pub const LIMITE_ARQUIVOS: usize = 200_000;

/// Os interruptores de `cacheBuild`. Chave ausente = o padrão de cada um (`venvCompartilhado` e `nodeModulesJuncao`
/// começam desligadas; as outras, ligadas).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheBuild {
    pub cargo_target: bool,
    pub node_hardlink: bool,
    pub node_junction: bool,
    pub venv: bool,
    pub preaquecer: bool,
}

impl CacheBuild {
    /// Lê `cacheBuild` do projeto (raiz do workspace). Arquivo ausente ou inválido: os padrões.
    pub fn de(projeto: Option<&Value>) -> Self {
        let flag = |k: &str, padrao: bool| projeto.and_then(|p| p.pointer(&format!("/cacheBuild/{k}"))).and_then(Value::as_bool).unwrap_or(padrao);
        Self {
            cargo_target: flag("cargoTargetCompartilhado", true),
            node_hardlink: flag("nodeModulesHardlink", true),
            node_junction: flag("nodeModulesJuncao", false),
            venv: flag("venvCompartilhado", false),
            preaquecer: flag("preaquecer", true),
        }
    }
}

/// Resultado do ligamento de `node_modules`. `Nada` = não havia o que ligar (ou falhou sem plano B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Via {
    #[default]
    Nada,
    Hardlink,
    Junction,
    /// Projeto com pnpm: não copia; o toast recomenda `pnpm install --offline`.
    Pnpm,
    /// Passou de `LIMITE_ARQUIVOS`: a cópia parcial foi apagada; o worktree instala do zero.
    Limite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Saved {
    pub via: Via,
    pub files: usize,
    pub ms: u128,
    /// Links dentro do `node_modules` do checkout principal que não foram recriados (sem seguir).
    pub puladas: usize,
}

/// Variáveis de ambiente das panes de uma worktree de missão. Rust: `CARGO_TARGET_DIR` compartilhado (e
/// `RUSTC_WRAPPER` com o caminho absoluto do sccache, se estiver no PATH). Python: `VIRTUAL_ENV` + PATH do `.venv`
/// do workspace, só com os arquivos de lock/requisitos iguais (bytes) e sem instalação editável. Vazio para as outras stacks.
pub fn cache_env(ws: &Path, wt: &Path, stacks: &[String], cache: &CacheBuild) -> Vec<(String, String)> {
    let tem = |s: &str| stacks.iter().any(|x| x == s);
    let mut env = vec![];
    if cache.cargo_target && tem("rust") {
        env.push(("CARGO_TARGET_DIR".into(), ws.join(".regente").join("cache").join("target").to_string_lossy().into_owned()));
        if let Some(sccache) = providers::which("sccache") {
            env.push(("RUSTC_WRAPPER".into(), sccache));
        }
    }
    let venv = ws.join(".venv");
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    if cache.venv && tem("python") && venv.join(bin).is_dir() && locks_iguais(ws, wt) && !tem_editavel(&venv, ws) {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let atual = std::env::var("PATH").unwrap_or_default();
        env.push(("VIRTUAL_ENV".into(), venv.to_string_lossy().into_owned()));
        env.push(("PATH".into(), format!("{}{sep}{atual}", venv.join(bin).to_string_lossy())));
    }
    env
}

/// O venv tem instalação editável (`pip install -e`, poetry, uv): aponta para o código do checkout principal, então
/// não serve à worktree. Procura `*.egg-link`, `__editable__*` e `direct_url.json` com `dir_info.editable`.
fn tem_editavel(venv: &Path, ws: &Path) -> bool {
    let mut sites = vec![venv.join("Lib").join("site-packages")];
    if let Ok(rd) = std::fs::read_dir(venv.join("lib")) {
        sites.extend(rd.flatten().map(|e| e.path().join("site-packages")));
    }
    sites.iter().any(|dir| {
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        rd.flatten().any(|e| {
            let nome = e.file_name().to_string_lossy().into_owned();
            if nome.ends_with(".egg-link") || nome.starts_with("__editable__") {
                return true;
            }
            // .pth com caminho absoluto dentro do workspace é instalação editável (ex.: `pip install -e .`)
            if nome.ends_with(".pth") && std::fs::read_to_string(e.path()).is_ok_and(|t| t.lines().any(|l| aponta_para_dentro(l.trim(), ws))) {
                return true;
            }
            nome.ends_with(".dist-info")
                && std::fs::read_to_string(e.path().join("direct_url.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.pointer("/dir_info/editable").and_then(Value::as_bool)) == Some(true)
        })
    })
}

/// A linha de um `.pth` é um caminho absoluto que fica dentro de `ws`. Compara sem caixa e sem o prefixo `\\?\`.
fn aponta_para_dentro(linha: &str, ws: &Path) -> bool {
    let norm = |t: &str| t.trim_start_matches(r"\\?\").replace('/', "\\").to_lowercase();
    let p = norm(linha);
    let w = norm(&ws.to_string_lossy());
    let p = Path::new(&p);
    p.is_absolute() && p.starts_with(Path::new(&w))
}

/// Arquivos de lock e de requisitos que decidem se o venv do workspace serve à worktree.
const LOCKS: [&str; 6] = ["requirements.txt", "requirements-dev.txt", "uv.lock", "poetry.lock", "Pipfile.lock", "pyproject.toml"];

/// Todos os arquivos de lock presentes no workspace existem, com os mesmos bytes, na worktree (e pelo menos um existe).
fn locks_iguais(ws: &Path, wt: &Path) -> bool {
    let mut achou = false;
    for f in LOCKS {
        match (std::fs::read(ws.join(f)), std::fs::read(wt.join(f))) {
            (Ok(a), Ok(b)) if a == b => achou = true,
            (Err(_), Err(_)) => {}
            _ => return false,
        }
    }
    achou
}

/// Estado de uma cópia com hardlinks: contadores e os caminhos que ela criou. Só esses podem ser apagados depois.
#[derive(Default)]
struct Copia {
    files: usize,
    puladas: usize,
    criados: Vec<PathBuf>,
}

/// Apaga só o que a cópia criou, do mais fundo para o mais raso. Diretório com conteúdo alheio (outra instalação
/// que começou na worktree) não é removido: o `remove_dir` falha e fica.
fn desfazer_copia(criados: &[PathBuf]) {
    for p in criados.iter().rev() {
        let _ = std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p));
    }
}

/// Copia a árvore de `src` para `dst` com hardlinks. Links e junções dentro de `src` não são seguidos: são pulados
/// (contados). `Ok(false)` = passou de `limite` arquivos; quem chama desfaz só o que `c.criados` lista.
/// Caches de ferramenta (`.cache`, `.vite`) na raiz de `node_modules` não são compartilhados (são da máquina).
fn copia(src: &Path, dst: &Path, c: &mut Copia, limite: usize, raiz: bool) -> std::io::Result<bool> {
    std::fs::create_dir(dst)?;
    c.criados.push(dst.to_path_buf());
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let nome = e.file_name();
        let (from, to) = (e.path(), dst.join(&nome));
        let m = std::fs::symlink_metadata(&from)?;
        if projeto::e_link(&m) || m.file_type().is_symlink() {
            c.puladas += 1;
        } else if m.is_dir() {
            if raiz && (nome == ".cache" || nome == ".vite") {
                c.puladas += 1;
                continue;
            }
            if !copia(&from, &to, c, limite, false)? {
                return Ok(false);
            }
        } else if m.is_file() {
            if c.files >= limite {
                return Ok(false);
            }
            std::fs::hard_link(&from, &to)?;
            c.criados.push(to);
            c.files += 1;
        }
    }
    Ok(true)
}

/// Caracteres que o `cmd.exe` interpreta mesmo entre aspas: caminho com algum deles não entra num `mklink`.
const CARACTERES_DO_CMD: &[char] = &['&', '%', '^', '|', '<', '>', '!', '"', '(', ')'];

/// Junção de diretório (`cmd /d /c mklink /J <link> <alvo>`). A linha vai com `raw_arg` e aspas explícitas, sem
/// concatenar nada num shell. Caminho com metacaractere do cmd ou controle é recusado (`caminho_invalido`).
pub(crate) fn junction(alvo: &Path, link: &Path) -> AppResult<()> {
    for p in [alvo, link] {
        let t = p.to_string_lossy();
        if t.chars().any(|c| c.is_control() || CARACTERES_DO_CMD.contains(&c)) {
            return fail("caminho_invalido", "caminho com caractere que o cmd interpreta (& % ^ | < > ! \" ( )) não é ligado");
        }
    }
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: sem janela de console piscando
        Command::new("cmd.exe")
            .raw_arg(format!("/d /c mklink /J \"{}\" \"{}\"", link.display(), alvo.display()))
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
    };
    #[cfg(not(windows))]
    let status = {
        std::os::unix::fs::symlink(alvo, link)?;
        return Ok(());
    };
    if status.success() { Ok(()) } else { fail("junction_falhou", "mklink /J não criou a junção") }
}

/// Liga o `node_modules` do checkout principal à worktree: hardlink da árvore (padrão). Com `juncao`, a junção entra
/// como plano B quando o hardlink falha. Acima de `LIMITE_ARQUIVOS` arquivos, apaga a cópia (`Via::Limite`).
/// `pnpm-lock.yaml` no projeto: não copia, só avisa. Não sobrescreve nada: se a worktree já tem `node_modules`, não faz nada.
pub fn link_node_modules(base: &Path, wt: &Path, juncao: bool) -> AppResult<Saved> {
    ligar(base, wt, juncao, LIMITE_ARQUIVOS)
}

fn ligar(base: &Path, wt: &Path, juncao: bool, limite: usize) -> AppResult<Saved> {
    let nada = Saved::default();
    let src = base.join("node_modules");
    let dst = wt.join("node_modules");
    let Ok(m) = std::fs::symlink_metadata(&src) else { return Ok(nada) };
    if projeto::e_link(&m) || !m.is_dir() || std::fs::symlink_metadata(&dst).is_ok() {
        return Ok(nada);
    }
    if base.join("pnpm-lock.yaml").is_file() {
        return Ok(Saved { via: Via::Pnpm, ..nada });
    }
    let inicio = Instant::now();
    let mut c = Copia::default();
    match copia(&src, &dst, &mut c, limite, true) {
        Ok(true) => Ok(Saved { via: Via::Hardlink, files: c.files, ms: inicio.elapsed().as_millis(), puladas: c.puladas }),
        Ok(false) => {
            desfazer_copia(&c.criados);
            tracing::warn!("node_modules passou de {limite} arquivos; a worktree vai instalar do zero");
            Ok(Saved { via: Via::Limite, files: c.files, ms: inicio.elapsed().as_millis(), puladas: c.puladas })
        }
        // alguém (um `npm install` já iniciado) criou node_modules na worktree no meio da cópia: não mexe e não liga
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            desfazer_copia(&c.criados);
            tracing::warn!("node_modules da worktree já está sendo preenchido; ligamento abortado sem apagar nada");
            Ok(nada)
        }
        Err(e) => {
            tracing::warn!("hardlink de node_modules falhou ({e})");
            desfazer_copia(&c.criados);
            if !juncao {
                return Ok(nada);
            }
            match junction(&src, &dst) {
                Ok(()) => Ok(Saved { via: Via::Junction, files: 0, ms: inicio.elapsed().as_millis(), puladas: 0 }),
                Err(e2) => {
                    tracing::warn!("junção de node_modules também falhou ({}); sem ligamento", e2.code());
                    Ok(nada)
                }
            }
        }
    }
}

/// Texto do aviso (pt-BR) sobre o ligamento; None quando não há o que dizer.
pub fn aviso(s: &Saved) -> Option<String> {
    const COMPARTILHADO: &str = "Os arquivos são compartilhados com o checkout principal; não edite arquivos dentro de node_modules no worktree.";
    match s.via {
        Via::Hardlink => Some(format!("node_modules ligado do checkout principal em {} s. {COMPARTILHADO}", format!("{:.1}", s.ms as f64 / 1000.0).replace('.', ","))),
        Via::Junction => Some(format!("node_modules ligado por junção do checkout principal. {COMPARTILHADO}")),
        Via::Pnpm => Some("Este projeto usa pnpm: rode `pnpm install --offline` na worktree em vez de copiar node_modules.".into()),
        Via::Limite => Some("node_modules grande demais para ligar (mais de 200 mil arquivos): instale as dependências no worktree.".into()),
        Via::Nada => None,
    }
}

/// Comandos do pré-aquecimento: o `build` do projeto (confiado), ou `cargo build --tests` em projeto Rust.
pub fn comandos_aquecer(stacks: &[String], projeto: Option<&Value>) -> Vec<String> {
    if let Some(b) = projeto.and_then(|p| p.pointer("/comandos/build")).and_then(Value::as_str) {
        return vec![b.to_string()];
    }
    if stacks.iter().any(|s| s == "rust") { vec!["cargo build --tests".into()] } else { vec![] }
}

/// Comandos do pré-aquecimento de uma missão, ou None quando não se aquece: sem worktree, pré-aquecimento desligado,
/// projeto não confiado ou nada a rodar.
pub fn comandos_da_missao(m: &Mission, stacks: &[String], proj: Option<&Value>, confiado: bool) -> Option<Vec<String>> {
    if m.worktree_path.is_none() || !CacheBuild::de(proj).preaquecer || !confiado {
        return None;
    }
    let c = comandos_aquecer(stacks, proj);
    (!c.is_empty()).then_some(c)
}

/// Stacks do workspace: as do `projeto.json` quando existem, senão as detectadas.
fn stacks_de(ws: &Path, projeto: Option<&Value>) -> Vec<String> {
    let lidas: Option<Vec<String>> = projeto.and_then(|p| p.get("stacks")).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
    lidas.unwrap_or_else(|| projeto::detect_stacks(ws))
}

/// Ambiente de cache para as panes de uma missão com worktree (`runtime::plan_for`). Vazio se a missão não tem worktree.
pub fn ambiente_da_missao(m: &Mission, ws: &Workspace) -> Vec<(String, String)> {
    let Some(wt) = m.worktree_path.as_deref() else { return vec![] };
    let root = Path::new(&ws.path);
    let proj = projeto::load(root).ok().flatten();
    cache_env(root, Path::new(wt), &stacks_de(root, proj.as_ref()), &CacheBuild::de(proj.as_ref()))
}

/// Depois de criada a missão: sem worktree, não faz nada. Com worktree, numa thread curta, liga o `node_modules`
/// (com o teto de arquivos), avisa (toast) e começa o pré-aquecimento, se o projeto é confiado. Não falha: erro vira log.
pub fn acelerar(s: &AppState, ws: &Workspace, m: &Mission) {
    acelerar_com(s, ws, m, true);
}

/// Como `acelerar`, escolhendo se pré-aquece: a comparação de abordagens liga só o `node_modules`, para não rodar
/// N builds ao mesmo tempo (um por variante).
pub fn acelerar_com(s: &AppState, ws: &Workspace, m: &Mission, aquecer: bool) {
    if m.worktree_path.is_none() {
        return;
    }
    let Some(alvo) = s.me.upgrade() else { return };
    let (ws, m) = (ws.clone(), m.clone());
    let spawned = std::thread::Builder::new().name("regente-worktree".into()).spawn(move || {
        let root = Path::new(&ws.path);
        let Some(wt) = m.worktree_path.as_deref().map(Path::new) else { return };
        let proj = projeto::load(root).ok().flatten();
        let cache = CacheBuild::de(proj.as_ref());
        let ligado = if cache.node_hardlink {
            link_node_modules(root, wt, cache.node_junction).unwrap_or_else(|e| {
                tracing::warn!("node_modules não ligado na worktree {}: {}", m.id, e.code());
                Saved::default()
            })
        } else {
            Saved::default()
        };
        if let Some(texto) = aviso(&ligado) {
            alvo.emit("worktree-acelerado", json!({"workspaceId": ws.id, "missionId": m.id, "texto": texto}));
        }
        if !aquecer {
            return;
        }
        let confiado = crate::verificacao::confiado(&alvo, &ws.id, root);
        let Some(comandos) = comandos_da_missao(&m, &stacks_de(root, proj.as_ref()), proj.as_ref(), confiado) else { return };
        let env = ambiente_da_missao(&m, &ws);
        if let Ok(cwd) = crate::planexec::cwd_da_missao(&alvo, &m.id) {
            crate::verificacao::aquecer(alvo.clone(), &m.id, cwd, comandos, env);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("thread de worktree não criada: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pth_inside_the_workspace_ignores_case_and_extended_prefix() {
        let ws = Path::new(r"C:\Users\dev\proj");
        assert!(aponta_para_dentro(r"c:\users\dev\proj\src", ws));
        assert!(aponta_para_dentro(r"\\?\C:\Users\dev\PROJ\src", ws));
        assert!(aponta_para_dentro("C:/Users/dev/proj/src", ws));
        assert!(!aponta_para_dentro(r"C:\Users\dev\projeto-x", ws), "prefixo de nome não é pasta");
        assert!(!aponta_para_dentro(r"relativo\src", ws));
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn stacks(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cache_build_defaults_to_on_and_respects_flags() {
        let padrao = CacheBuild::de(None);
        assert!(padrao.cargo_target && padrao.node_hardlink && padrao.preaquecer);
        assert!(!padrao.venv && !padrao.node_junction, "venv compartilhado e junção são opt-in");
        let p = serde_json::json!({"cacheBuild": {"nodeModulesHardlink": false, "preaquecer": false}});
        let c = CacheBuild::de(Some(&p));
        assert!(!c.node_hardlink && !c.preaquecer && c.cargo_target);
    }

    #[test]
    fn rust_gets_shared_target_and_no_python_env() {
        let d = tmp();
        let env = cache_env(d.path(), d.path(), &stacks(&["rust"]), &CacheBuild::de(None));
        let target = env.iter().find(|(k, _)| k == "CARGO_TARGET_DIR").expect("CARGO_TARGET_DIR");
        assert!(target.1.ends_with(&format!("{}cache{}target", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR)), "{}", target.1);
        assert!(!env.iter().any(|(k, _)| k == "VIRTUAL_ENV"));
        // flag desligada: nada
        let off = CacheBuild { cargo_target: false, ..CacheBuild::de(None) };
        assert!(cache_env(d.path(), d.path(), &stacks(&["rust"]), &off).is_empty());
    }

    #[test]
    fn python_venv_only_with_identical_locks() {
        let base = tmp();
        let wt = tmp();
        std::fs::create_dir_all(base.path().join(".venv").join(if cfg!(windows) { "Scripts" } else { "bin" })).unwrap();
        std::fs::write(base.path().join("requirements.txt"), "flask==3\n").unwrap();
        std::fs::write(wt.path().join("requirements.txt"), "flask==3\n").unwrap();
        let env = cache_env(base.path(), wt.path(), &stacks(&["python"]), &CacheBuild { venv: true, ..CacheBuild::de(None) });
        assert!(env.iter().any(|(k, _)| k == "VIRTUAL_ENV"), "{env:?}");
        let bin = if cfg!(windows) { "Scripts" } else { "bin" };
        assert!(env.iter().any(|(k, v)| k == "PATH" && v.contains(bin)));
        // lock diferente na worktree: sem venv compartilhado
        std::fs::write(wt.path().join("requirements.txt"), "flask==2\n").unwrap();
        assert!(!cache_env(base.path(), wt.path(), &stacks(&["python"]), &CacheBuild { venv: true, ..CacheBuild::de(None) }).iter().any(|(k, _)| k == "VIRTUAL_ENV"));
    }

    #[test]
    fn hardlink_copy_matches_content_and_counts_files() {
        let base = tmp();
        let wt = tmp();
        let nm = base.path().join("node_modules");
        std::fs::create_dir_all(nm.join("a").join("b")).unwrap();
        std::fs::write(nm.join("x.js"), "1").unwrap();
        std::fs::write(nm.join("a").join("b").join("y.js"), "22").unwrap();
        let r = link_node_modules(base.path(), wt.path(), false).unwrap();
        assert_eq!(r.via, Via::Hardlink);
        assert_eq!(r.files, 2);
        assert_eq!(std::fs::read_to_string(wt.path().join("node_modules").join("a").join("b").join("y.js")).unwrap(), "22");
        // a worktree já tem node_modules: nada muda
        assert_eq!(link_node_modules(base.path(), wt.path(), false).unwrap().via, Via::Nada);
    }

    /// Remove um link criado no teste (junção ou symlink) sem seguir o alvo.
    fn tira_link(link: &Path) {
        #[cfg(windows)]
        std::fs::remove_dir(link).unwrap();
        #[cfg(not(windows))]
        std::fs::remove_file(link).unwrap();
    }

    #[test]
    fn junction_fallback_links_the_directory() {
        let alvo = tmp();
        let d = tmp();
        std::fs::write(alvo.path().join("pacote.js"), "x").unwrap();
        let link = d.path().join("node_modules");
        junction(alvo.path(), &link).unwrap();
        assert_eq!(std::fs::read_to_string(link.join("pacote.js")).unwrap(), "x");
        tira_link(&link);
    }

    #[test]
    fn links_inside_node_modules_are_skipped_not_followed() {
        let base = tmp();
        let wt = tmp();
        let fora = tmp();
        std::fs::write(fora.path().join("segredo.txt"), "fora").unwrap();
        let nm = base.path().join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("ok.js"), "1").unwrap();
        let armadilha = nm.join("link-para-fora");
        junction(fora.path(), &armadilha).unwrap();
        let r = link_node_modules(base.path(), wt.path(), false).unwrap();
        assert_eq!(r.via, Via::Hardlink);
        assert_eq!(r.puladas, 1);
        assert_eq!(r.files, 1);
        assert!(!wt.path().join("node_modules").join("link-para-fora").exists(), "não seguiu o link");
        assert!(!wt.path().join("node_modules").join("link-para-fora").join("segredo.txt").exists());
        tira_link(&armadilha);
    }

    #[test]
    fn pnpm_projects_are_not_copied() {
        let base = tmp();
        let wt = tmp();
        std::fs::create_dir_all(base.path().join("node_modules")).unwrap();
        std::fs::write(base.path().join("pnpm-lock.yaml"), "x").unwrap();
        let r = link_node_modules(base.path(), wt.path(), false).unwrap();
        assert_eq!(r.via, Via::Pnpm);
        assert!(!wt.path().join("node_modules").exists());
        assert!(aviso(&r).unwrap().contains("pnpm install --offline"));
    }

    #[test]
    fn junction_refuses_quotes_and_control_chars() {
        let d = tmp();
        assert_eq!(junction(Path::new("C:\\a\"b"), &d.path().join("l")).unwrap_err().code(), "caminho_invalido");
        assert_eq!(junction(Path::new("C:\\R&D\\x"), &d.path().join("l")).unwrap_err().code(), "caminho_invalido");
        assert_eq!(junction(d.path(), Path::new("C:\\R%D\\x")).unwrap_err().code(), "caminho_invalido");
        assert_eq!(junction(d.path(), Path::new("C:\\x\u{7}y")).unwrap_err().code(), "caminho_invalido");
    }

    #[test]
    fn prewarm_uses_project_build_or_cargo_tests() {
        let p = serde_json::json!({"comandos": {"build": "npm run build"}});
        assert_eq!(comandos_aquecer(&stacks(&["node"]), Some(&p)), vec!["npm run build".to_string()]);
        assert_eq!(comandos_aquecer(&stacks(&["rust"]), None), vec!["cargo build --tests".to_string()]);
        assert!(comandos_aquecer(&stacks(&["python"]), None).is_empty());
    }

    #[test]
    fn aviso_text_uses_comma_decimal() {
        let s = Saved { via: Via::Hardlink, files: 10, ms: 2100, puladas: 0 };
        assert!(aviso(&s).unwrap().starts_with("node_modules ligado do checkout principal em 2,1 s. Os arquivos são compartilhados"));
        assert!(aviso(&Saved::default()).is_none());
    }
    #[test]
    fn mission_without_worktree_prewarms_nothing() {
        let m = Mission { id: "m".into(), ..Default::default() };
        let stacks = stacks(&["rust"]);
        assert!(comandos_da_missao(&m, &stacks, None, true).is_none(), "sem worktree não aquece");
        let com = Mission { worktree_path: Some("C:/wt".into()), ..m.clone() };
        assert_eq!(comandos_da_missao(&com, &stacks, None, true), Some(vec!["cargo build --tests".to_string()]));
        assert!(comandos_da_missao(&com, &stacks, None, false).is_none(), "projeto não confiado não aquece");
    }

    #[test]
    fn node_modules_over_the_cap_is_removed_and_reported() {
        let base = tmp();
        let wt = tmp();
        let nm = base.path().join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("a.js"), "1").unwrap();
        std::fs::write(nm.join("b.js"), "2").unwrap();
        let r = ligar(base.path(), wt.path(), false, 1).unwrap();
        assert_eq!(r.via, Via::Limite);
        assert!(!wt.path().join("node_modules").exists(), "cópia parcial apagada");
        assert_eq!(std::fs::read_to_string(nm.join("a.js")).unwrap(), "1", "base intacta");
    }

    #[test]
    fn editable_install_disables_the_shared_venv() {
        let base = tmp();
        let wt = tmp();
        let bin = if cfg!(windows) { "Scripts" } else { "bin" };
        std::fs::create_dir_all(base.path().join(".venv").join(bin)).unwrap();
        let sp = if cfg!(windows) { base.path().join(".venv").join("Lib").join("site-packages") } else { base.path().join(".venv").join("lib").join("python3.12").join("site-packages") };
        std::fs::create_dir_all(&sp).unwrap();
        std::fs::write(base.path().join("requirements.txt"), "x\n").unwrap();
        std::fs::write(wt.path().join("requirements.txt"), "x\n").unwrap();
        let venv_on = CacheBuild { venv: true, ..CacheBuild::de(None) };
        assert!(cache_env(base.path(), wt.path(), &stacks(&["python"]), &venv_on).iter().any(|(k, _)| k == "VIRTUAL_ENV"));
        std::fs::write(sp.join("meu_pacote.egg-link"), "/codigo/principal").unwrap();
        assert!(!cache_env(base.path(), wt.path(), &stacks(&["python"]), &venv_on).iter().any(|(k, _)| k == "VIRTUAL_ENV"), "editável não compartilha");
    }

    #[test]
    fn copy_never_deletes_what_it_did_not_create() {
        let base = tmp();
        let wt = tmp();
        let nm = base.path().join("node_modules");
        std::fs::create_dir_all(&nm).unwrap();
        std::fs::write(nm.join("a.js"), "1").unwrap();
        // outra instalação já começou na worktree: a cópia não pode criar nem apagar nada ali
        let alheio = wt.path().join("node_modules");
        std::fs::create_dir_all(&alheio).unwrap();
        std::fs::write(alheio.join("x"), "meu").unwrap();
        let r = ligar(base.path(), wt.path(), true, LIMITE_ARQUIVOS).unwrap();
        assert_eq!(r.via, Via::Nada);
        assert_eq!(std::fs::read_to_string(alheio.join("x")).unwrap(), "meu");
        // direto na cópia: dst que já existe falha com AlreadyExists e nada é apagado
        let mut c = Copia::default();
        let e = copia(&nm, &alheio, &mut c, LIMITE_ARQUIVOS, true).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        desfazer_copia(&c.criados);
        assert_eq!(std::fs::read_to_string(alheio.join("x")).unwrap(), "meu");
    }

    #[test]
    fn over_the_cap_rolls_back_only_its_own_links_and_dirs() {
        let base = tmp();
        let wt = tmp();
        let nm = base.path().join("node_modules");
        std::fs::create_dir_all(nm.join("sub")).unwrap();
        std::fs::write(nm.join("sub").join("a.js"), "1").unwrap();
        std::fs::write(nm.join("sub").join("b.js"), "2").unwrap();
        std::fs::write(nm.join("c.js"), "3").unwrap();
        let r = ligar(base.path(), wt.path(), false, 2).unwrap();
        assert_eq!(r.via, Via::Limite);
        assert!(!wt.path().join("node_modules").exists());
        assert_eq!(std::fs::read_to_string(nm.join("sub").join("a.js")).unwrap(), "1", "base intacta");
    }

    #[test]
    fn tool_caches_are_not_shared() {
        let base = tmp();
        let wt = tmp();
        let nm = base.path().join("node_modules");
        std::fs::create_dir_all(nm.join(".cache").join("x")).unwrap();
        std::fs::create_dir_all(nm.join(".vite")).unwrap();
        std::fs::write(nm.join(".cache").join("x").join("c"), "c").unwrap();
        std::fs::write(nm.join("k.js"), "k").unwrap();
        let r = ligar(base.path(), wt.path(), false, LIMITE_ARQUIVOS).unwrap();
        assert_eq!(r.files, 1);
        assert!(!wt.path().join("node_modules").join(".cache").exists());
        assert!(!wt.path().join("node_modules").join(".vite").exists());
    }

    #[test]
    fn pth_pointing_inside_the_workspace_is_editable() {
        let ws = tmp();
        let venv = ws.path().join(".venv");
        let sp = venv.join("Lib").join("site-packages");
        std::fs::create_dir_all(&sp).unwrap();
        assert!(!tem_editavel(&venv, ws.path()));
        std::fs::write(sp.join("projeto.pth"), format!("{}
", ws.path().join("src").display())).unwrap();
        assert!(tem_editavel(&venv, ws.path()), ".pth absoluto dentro do workspace");
        std::fs::write(sp.join("projeto.pth"), "/fora/do/workspace
").unwrap();
        assert!(!tem_editavel(&venv, ws.path()), ".pth para fora não conta");
    }
}
