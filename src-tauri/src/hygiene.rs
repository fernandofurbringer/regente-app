//! Higiene das instruções (RM-22, H1-11): mede o custo fixo dos arquivos de instrução e aponta cortes.
//! Só lê. Nunca grava nem edita: o usuário copia um prompt de limpeza e decide.
//! Tokens são estimativas (bytes / 4).

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const LINHAS_AVISO: usize = 200;
const LINHAS_ALTO: usize = 400;
const MEMORIA_LINHAS: usize = 200;
const MEMORIA_BYTES: usize = 25 * 1024;
/// Limite do Codex para AGENTS.md (`project_doc_max_bytes`, padrão).
const CODEX_BYTES: usize = 32 * 1024;
const BLOCO_LONGO: usize = 30;
const TABELA_LINHAS: usize = 15;
/// Linhas repetidas só contam se forem longas o bastante para importar.
const REPETIDA_MIN: usize = 20;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub arquivos: Vec<Arquivo>,
    pub avisos: Vec<Aviso>,
    /// Soma de tokens estimados dos arquivos lidos (imports incluídos): custo fixo por turno.
    pub tokens_fixos: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Arquivo {
    pub path: String,
    pub linhas: usize,
    pub bytes: usize,
    pub tokens_est: usize,
    pub importacoes: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Aviso {
    /// info | aviso | alto
    pub gravidade: &'static str,
    pub arquivo: String,
    pub codigo: &'static str,
    pub texto: String,
}

/// Teto de leitura: arquivo maior que isso é reportado, não lido.
const LEITURA_MAX: u64 = 1024 * 1024;

/// Lê um arquivo de texto comum (sem link, até 1 MiB). Tira o BOM UTF-8; bytes inválidos viram U+FFFD.
fn ler(p: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(p).ok()?;
    if crate::projeto::e_link(&meta) || !meta.is_file() || meta.len() > LEITURA_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(p).ok()?.take(LEITURA_MAX).read_to_end(&mut bytes).ok()?;
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Tamanho em MB (arredondado para cima) de um arquivo comum acima do teto de leitura.
fn grande_demais(p: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(p).ok()?;
    (!crate::projeto::e_link(&meta) && meta.is_file() && meta.len() > LEITURA_MAX).then(|| meta.len().div_ceil(LEITURA_MAX))
}

/// Arquivos de instrução do workspace (raiz e `.claude/rules/*.md`). Links não entram.
fn candidatos(ws: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = ["CLAUDE.md", "AGENTS.md", "GEMINI.md"].iter().map(|n| ws.join(n)).filter(|p| comum(p)).collect();
    if let Ok(rd) = std::fs::read_dir(ws.join(".claude").join("rules")) {
        let mut regras: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md") && comum(p)).collect();
        regras.sort();
        out.extend(regras);
    }
    out
}

/// Arquivo comum (não link nem pasta).
fn comum(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| !crate::projeto::e_link(&m) && m.is_file())
}

/// Varre o workspace. `memoria`: o MEMORY.md do projeto, quando o chamador o localizou.
pub fn scan(ws: &Path, memoria: Option<&Path>) -> Report {
    let mut rep = Report::default();
    let mut textos: Vec<(String, String)> = vec![];
    for p in candidatos(ws) {
        let rel = p.strip_prefix(ws).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        if let Some(mb) = grande_demais(&p) {
            rep.avisos.push(aviso("aviso", &rel, "arquivo_grande", format!("arquivo grande demais ({mb} MB); não foi lido.")));
            continue;
        }
        let Some(texto) = ler(&p) else { continue };
        textos.push((rel, texto));
    }
    if let Some(m) = memoria.filter(|m| m.is_file()) {
        match ler(m) {
            Some(texto) => {
                let linhas = texto.lines().count();
                if linhas > MEMORIA_LINHAS || texto.len() > MEMORIA_BYTES {
                    rep.avisos.push(aviso("aviso", "MEMORY.md", "memoria_grande", format!("MEMORY.md tem {linhas} linhas; o Claude carrega só as primeiras 200 linhas ou 25 KB.")));
                }
            }
            None if grande_demais(m).is_some() => {
                rep.avisos.push(aviso("aviso", "MEMORY.md", "memoria_grande", "MEMORY.md tem mais de 1 MB; o Claude carrega só as primeiras 200 linhas ou 25 KB.".into()));
            }
            None => {}
        }
    }
    let mut repetidas: HashMap<String, HashSet<String>> = HashMap::new();
    for (rel, texto) in &textos {
        let linhas = texto.lines().count();
        let imports = importacoes(texto);
        // import que sai do workspace (absoluto ou com `..`) não é lido
        for i in imports.iter().filter(|i| !dentro_do_workspace(i)) {
            rep.avisos.push(aviso("alto", rel, "import_fora", format!("import fora do workspace: @{i}")));
        }
        // imports `@arquivo` custam o mesmo que o texto colado: entram no total
        let raiz_real = std::fs::canonicalize(ws).ok();
        let seguros: Vec<PathBuf> = imports.iter().filter_map(|i| import_seguro(ws, raiz_real.as_deref(), i)).collect();
        let extra: usize = seguros.iter().filter_map(|p| ler(p)).map(|t| t.len() / 4).sum();
        let tokens = texto.len() / 4 + extra;
        rep.tokens_fixos += tokens;
        let lidos = seguros.len();
        rep.arquivos.push(Arquivo { path: rel.clone(), linhas, bytes: texto.len(), tokens_est: tokens, importacoes: lidos });
        if linhas > LINHAS_ALTO {
            rep.avisos.push(aviso("alto", rel, "linhas_demais", format!("{linhas} linhas (~{tokens} tokens em cada turno). Ideal: abaixo de 200.")));
        } else if linhas > LINHAS_AVISO {
            rep.avisos.push(aviso("aviso", rel, "linhas_demais", format!("{linhas} linhas (~{tokens} tokens em cada turno). Ideal: abaixo de 200.")));
        }
        if rel.eq_ignore_ascii_case("AGENTS.md") && texto.len() > CODEX_BYTES {
            rep.avisos.push(aviso("alto", rel, "codex_bytes", format!("{} bytes; o Codex lê até 32 KB deste arquivo.", texto.len())));
        }
        if rel == "CLAUDE.md" && !texto.contains("Compact instructions") && !texto.contains("Instruções de compactação") {
            rep.avisos.push(aviso("info", rel, "sem_compactacao", "Sem seção \"Compact instructions\": ela orienta o que o /compact preserva.".into()));
        }
        blocos_e_tabelas(rel, texto, &mut rep);
        let todos = texto.lines().filter(|l| l.contains("TODO") || l.contains("FIXME")).count();
        if todos > 0 {
            rep.avisos.push(aviso("info", rel, "todo", format!("{todos} marcadores TODO/FIXME: resolva ou tire do arquivo de instrução.")));
        }
        for l in texto.lines().map(str::trim).filter(|l| l.len() >= REPETIDA_MIN && !l.starts_with('#')) {
            repetidas.entry(l.to_string()).or_default().insert(rel.clone());
        }
    }
    let dup = repetidas.values().filter(|arqs| arqs.len() > 1).count();
    if dup > 0 {
        rep.avisos.push(aviso("aviso", "(vários)", "duplicadas", format!("{dup} linhas repetidas entre arquivos de instrução: mantenha em um só lugar.")));
    }
    rep
}

fn aviso(gravidade: &'static str, arquivo: &str, codigo: &'static str, texto: String) -> Aviso {
    Aviso { gravidade, arquivo: arquivo.to_string(), codigo, texto }
}

/// Marcador de cerca de código (``` ou ~~~) no começo da linha, se houver.
fn cerca(linha: &str) -> Option<&'static str> {
    let t = linha.trim_start();
    if t.starts_with("```") {
        Some("```")
    } else if t.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Linhas fora de blocos de código. Um bloco só fecha com o mesmo marcador que abriu.
fn fora_de_codigo(texto: &str) -> Vec<&str> {
    let mut marca: Option<&str> = None;
    let mut fora = vec![];
    for l in texto.lines() {
        match (marca, cerca(l)) {
            (None, Some(m)) => marca = Some(m),
            (Some(m), Some(n)) if m == n => marca = None,
            (None, None) => fora.push(l),
            _ => {}
        }
    }
    fora
}

/// Linhas `@caminho` (import do Claude Code) fora de código, sem espaço e sem o `@` inicial.
fn importacoes(texto: &str) -> Vec<String> {
    fora_de_codigo(texto).into_iter().map(str::trim).filter_map(|l| l.strip_prefix('@')).filter(|c| !c.is_empty() && !c.contains(' ')).map(String::from).collect()
}

/// Caminho relativo que fica dentro do workspace: sem raiz, sem prefixo de drive e sem `..`.
fn dentro_do_workspace(import: &str) -> bool {
    Path::new(import).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Import lido só se o caminho, já resolvido (junções e links dos diretórios do meio), continua dentro da raiz canônica.
fn import_seguro(ws: &Path, raiz_real: Option<&Path>, import: &str) -> Option<PathBuf> {
    if !dentro_do_workspace(import) {
        return None;
    }
    let real = std::fs::canonicalize(ws.join(import)).ok()?;
    real.starts_with(raiz_real?).then_some(real)
}

/// Blocos de código longos e tabelas grandes: candidatos a virar skill ou regra por caminho.
fn blocos_e_tabelas(rel: &str, texto: &str, rep: &mut Report) {
    let (mut marca, mut inicio, mut longo, mut tabela) = (None::<&str>, 0usize, false, 0usize);
    for (i, l) in texto.lines().enumerate() {
        match (marca, cerca(l)) {
            (None, Some(m)) => {
                marca = Some(m);
                inicio = i;
            }
            (Some(m), Some(n)) if m == n => {
                if i - inicio > BLOCO_LONGO {
                    longo = true;
                }
                marca = None;
            }
            (None, None) if l.trim_start().starts_with('|') => tabela += 1,
            _ => {}
        }
    }
    if longo {
        rep.avisos.push(aviso("info", rel, "bloco_longo", format!("Bloco de código com mais de {BLOCO_LONGO} linhas: mova para uma skill ou arquivo de referência.")));
    }
    if tabela > TABELA_LINHAS {
        rep.avisos.push(aviso("info", rel, "tabela_grande", format!("{tabela} linhas de tabela: mova para um arquivo de referência.")));
    }
}

/// Caminho do MEMORY.md do projeto no diretório de configuração do Claude (nome do diretório = caminho com não-alfanuméricos trocados por `-`).
pub fn memoria_do_projeto(config_dir: &Path, ws: &Path) -> PathBuf {
    let nome: String = ws.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    config_dir.join("projects").join(nome).join("memory").join("MEMORY.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn put(root: &Path, rel: &str, texto: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, texto).unwrap();
    }

    fn com_linhas(n: usize) -> String {
        (0..n).map(|i| format!("linha {i}\n")).collect()
    }

    #[test]
    fn empty_workspace_is_an_empty_report() {
        let d = ws();
        assert_eq!(scan(d.path(), None), Report::default());
    }

    #[test]
    fn line_thresholds_warn_and_flag() {
        let d = ws();
        put(d.path(), "CLAUDE.md", &com_linhas(200));
        assert!(!scan(d.path(), None).avisos.iter().any(|a| a.codigo == "linhas_demais"), "200 linhas ainda passa");
        put(d.path(), "CLAUDE.md", &com_linhas(201));
        let r = scan(d.path(), None);
        assert!(r.avisos.iter().any(|a| a.codigo == "linhas_demais" && a.gravidade == "aviso"));
        put(d.path(), "CLAUDE.md", &com_linhas(401));
        assert!(scan(d.path(), None).avisos.iter().any(|a| a.codigo == "linhas_demais" && a.gravidade == "alto"));
    }

    #[test]
    fn bom_and_windows_paths_are_read() {
        let d = ws();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("# Regras\r\n".as_bytes());
        std::fs::write(d.path().join("CLAUDE.md"), bytes).unwrap();
        put(d.path(), ".claude/rules/b.md", "regra");
        let r = scan(d.path(), None);
        assert_eq!(r.arquivos[0].path, "CLAUDE.md");
        assert_eq!(r.arquivos[1].path, ".claude/rules/b.md");
        assert_eq!(r.arquivos[0].bytes, "# Regras\r\n".len(), "o BOM não conta");
    }

    #[test]
    fn imports_add_to_the_fixed_cost() {
        let d = ws();
        put(d.path(), "extra.md", &"x".repeat(4000));
        put(d.path(), "CLAUDE.md", "@extra.md\n");
        let r = scan(d.path(), None);
        assert_eq!(r.arquivos[0].importacoes, 1);
        assert!(r.tokens_fixos >= 1000, "import de 4000 bytes soma ~1000 tokens");
    }

    #[test]
    fn missing_compact_section_is_info_and_present_clears_it() {
        let d = ws();
        put(d.path(), "CLAUDE.md", "# Projeto\n");
        assert!(scan(d.path(), None).avisos.iter().any(|a| a.codigo == "sem_compactacao" && a.gravidade == "info"));
        put(d.path(), "CLAUDE.md", "# Projeto\n\n## Compact instructions\nPreserve o plano.\n");
        assert!(!scan(d.path(), None).avisos.iter().any(|a| a.codigo == "sem_compactacao"));
    }

    #[test]
    fn duplicate_lines_across_files_are_reported() {
        let d = ws();
        let regra = "Use sempre cargo clippy antes de commitar.";
        put(d.path(), "CLAUDE.md", &format!("{regra}\n"));
        put(d.path(), "AGENTS.md", &format!("{regra}\n"));
        assert!(scan(d.path(), None).avisos.iter().any(|a| a.codigo == "duplicadas"));
    }

    #[test]
    fn long_blocks_tables_and_todos() {
        let d = ws();
        let bloco = format!("```\n{}```\n", com_linhas(31));
        let tabela: String = (0..16).map(|i| format!("| {i} | x |\n")).collect();
        put(d.path(), "CLAUDE.md", &format!("TODO: revisar\n{bloco}{tabela}"));
        let r = scan(d.path(), None);
        for c in ["bloco_longo", "tabela_grande", "todo"] {
            assert!(r.avisos.iter().any(|a| a.codigo == c), "{c}");
        }
    }

    #[test]
    fn memory_limits_and_codex_bytes() {
        let d = ws();
        let mem = d.path().join("MEMORY.md");
        std::fs::write(&mem, com_linhas(201)).unwrap();
        assert!(scan(d.path(), Some(&mem)).avisos.iter().any(|a| a.codigo == "memoria_grande"));
        put(d.path(), "AGENTS.md", &"a".repeat(CODEX_BYTES + 1));
        assert!(scan(d.path(), None).avisos.iter().any(|a| a.codigo == "codex_bytes" && a.gravidade == "alto"));
    }

    #[test]
    fn scan_never_writes() {
        let d = ws();
        put(d.path(), "CLAUDE.md", &com_linhas(300));
        let antes = std::fs::read(d.path().join("CLAUDE.md")).unwrap();
        let _ = scan(d.path(), None);
        assert_eq!(std::fs::read(d.path().join("CLAUDE.md")).unwrap(), antes);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "nenhum arquivo novo");
    }

    #[test]
    fn imports_outside_the_workspace_are_rejected_and_reported() {
        let d = ws();
        put(d.path(), "CLAUDE.md", "@/etc/passwd\n@..\\segredo.md\n@../../fora.md\n");
        let r = scan(d.path(), None);
        assert_eq!(r.avisos.iter().filter(|a| a.codigo == "import_fora").count(), 3);
        assert_eq!(r.arquivos[0].importacoes, 0, "nenhum import entra no custo");
    }

    #[test]
    fn tilde_fences_count_as_code_and_at_signs_inside_are_not_imports() {
        let d = ws();
        put(d.path(), "extra.md", &"x".repeat(4000));
        let bloco = format!("~~~\n{}~~~\n", com_linhas(31));
        put(d.path(), "CLAUDE.md", &format!("{bloco}```\n@extra.md\n```\n"));
        let r = scan(d.path(), None);
        assert!(r.avisos.iter().any(|a| a.codigo == "bloco_longo"), "bloco com ~~~ conta");
        assert_eq!(r.arquivos[0].importacoes, 0, "@ dentro de bloco não é import");
        assert!(!r.avisos.iter().any(|a| a.codigo == "import_fora"));
    }

    #[test]
    fn oversized_files_are_reported_not_read() {
        let d = ws();
        put(d.path(), "CLAUDE.md", &"a".repeat(2 * 1024 * 1024 + 5));
        let r = scan(d.path(), None);
        assert!(r.arquivos.is_empty(), "não entra no custo");
        assert!(r.avisos.iter().any(|a| a.codigo == "arquivo_grande" && a.texto == "arquivo grande demais (3 MB); não foi lido."));
    }

    #[test]
    fn project_memory_path_mangles_the_workspace_path() {
        let p = memoria_do_projeto(Path::new("C:/cfg"), Path::new("D:\\pessoal\\REA"));
        assert_eq!(p, Path::new("C:/cfg").join("projects").join("D--pessoal-REA").join("memory").join("MEMORY.md"));
    }
}
