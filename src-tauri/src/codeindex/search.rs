//! Consultas sobre o índice (ranking de definições) e busca regex no código, no estilo ripgrep (sem binário externo).

use super::outline::Symbol;
use super::{caminhar, rel_de, Index, ARQUIVO_MAX, MAX_FILES, TEMPO_MAX};
use crate::error::{AppError, AppResult};
use grep_regex::RegexMatcher;
use grep_searcher::{sinks::UTF8, BinaryDetection, SearcherBuilder};
use std::path::Path;

/// Linha de resultado: no máximo 200 caracteres.
const LINHA_MAX: usize = 200;

pub struct Acerto<'a> {
    pub rel: &'a str,
    pub simbolo: &'a Symbol,
    pub score: u32,
}

/// Definições pelo nome, da mais parecida para a menos: igual 100, prefixo 60, contém 30; +5 se for de topo.
/// Empate: caminho, depois linha, então a mesma consulta devolve sempre a mesma ordem.
pub fn ranking<'a>(idx: &'a Index, consulta: &str, kind: Option<&str>, glob: Option<&glob::Pattern>, limite: usize) -> Vec<Acerto<'a>> {
    let q = consulta.to_lowercase();
    let mut out = vec![];
    for (rel, e) in &idx.arquivos {
        if glob.is_some_and(|g| !g.matches(rel)) {
            continue;
        }
        for s in &e.simbolos {
            if kind.is_some_and(|k| k != s.kind.as_str()) {
                continue;
            }
            let n = s.name.to_lowercase();
            let base = if n == q {
                100
            } else if n.starts_with(&q) {
                60
            } else if n.contains(&q) {
                30
            } else {
                continue;
            };
            out.push(Acerto { rel: rel.as_str(), simbolo: s, score: base + if s.top { 5 } else { 0 } });
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.rel.cmp(b.rel)).then(a.simbolo.line.cmp(&b.simbolo.line)));
    out.truncate(limite);
    out
}

pub struct Opcoes {
    pub glob: Option<glob::Pattern>,
    /// Linhas de contexto antes e depois de cada acerto.
    pub contexto: usize,
    pub limite: usize,
    /// Teto do texto devolvido (caracteres).
    pub max_chars: usize,
}

/// Busca regex em todos os arquivos da raiz (.gitignore respeitado). Saída `arquivo:linha: texto`; contexto usa `-`.
pub fn grep(raiz: &Path, padrao: &str, op: &Opcoes) -> AppResult<String> {
    let m = RegexMatcher::new(padrao).map_err(|e| AppError::new("regex_invalida", e.to_string()))?;
    let mut buscador = SearcherBuilder::new().line_number(true).binary_detection(BinaryDetection::quit(b'\x00')).build();
    let mut saida = String::new();
    let mut total = 0usize;
    let mut cortado = false;
    let inicio = std::time::Instant::now();
    let mut visitados = 0usize;
    let mut parcial = false;
    'arquivos: for item in caminhar(raiz) {
        let Ok(e) = item else { continue };
        if !e.file_type().is_some_and(|t| t.is_file()) || e.metadata().map_or(true, |m| m.len() > ARQUIVO_MAX) {
            continue;
        }
        if inicio.elapsed() > TEMPO_MAX {
            parcial = true;
            break;
        }
        let path = e.path();
        let Some(rel) = rel_de(raiz, path) else { continue };
        if op.glob.as_ref().is_some_and(|g| !g.matches(&rel)) {
            continue;
        }
        let restante = op.limite.saturating_sub(total);
        if restante == 0 {
            break;
        }
        // o teto de arquivos conta só os que de fato são lidos (não os pulados pelo glob)
        visitados += 1;
        if visitados > MAX_FILES {
            parcial = true;
            break;
        }
        let mut linhas: Vec<u64> = vec![];
        let _ = buscador.search_path(&m, path, UTF8(|n: u64, _: &str| {
            linhas.push(n);
            Ok(linhas.len() < restante)
        }));
        if linhas.is_empty() {
            continue;
        }
        let Ok(texto) = std::fs::read_to_string(path) else { continue };
        let todas: Vec<&str> = texto.lines().collect();
        // última linha já impressa neste arquivo: contexto de acertos vizinhos não repete linhas
        let mut ultima = 0usize;
        for n in linhas {
            total += 1;
            let n = n as usize;
            let ini = n.saturating_sub(op.contexto).max(1).max(ultima + 1);
            let fim = (n + op.contexto).min(todas.len());
            for k in ini..=fim {
                let sep = if k == n { ':' } else { '-' };
                let linha = format!("{rel}:{k}{sep} {}\n", trunca(todas[k - 1], LINHA_MAX));
                if saida.len() + linha.len() > op.max_chars {
                    cortado = true;
                    break 'arquivos;
                }
                saida.push_str(&linha);
                ultima = k;
            }
        }
        if total >= op.limite {
            break;
        }
    }
    if parcial {
        saida.push_str("… (busca parcial: limite de arquivos ou de tempo)
");
    }
    if total == 0 {
        return Ok("Nenhum resultado.".into());
    }
    if cortado {
        saida.push_str(&format!("… (resposta cortada em {} caracteres)\n", op.max_chars));
    }
    Ok(saida)
}

/// Corta por caracteres (não por bytes), com reticências quando corta.
pub fn trunca(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut t: String = s.chars().take(n).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codeindex::{atualizar, MAX_FILES};
    use std::time::Duration;

    fn put(root: &Path, rel: &str, texto: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, texto).unwrap();
    }

    fn indice(raiz: &Path) -> Index {
        let mut idx = Index::default();
        atualizar(&mut idx, raiz, MAX_FILES, Duration::from_secs(60));
        idx
    }

    fn op(limite: usize, contexto: usize, max_chars: usize) -> Opcoes {
        Opcoes { glob: None, contexto, limite, max_chars }
    }

    #[test]
    fn exact_name_ranks_first_and_order_is_deterministic() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "a.rs", "pub fn salvar() {}\nfn salvar_tudo() {}\n");
        put(d.path(), "b.rs", "fn salvar() {}\nfn outra_salvar() {}\n");
        let idx = indice(d.path());
        let r1: Vec<(String, u32)> = ranking(&idx, "salvar", None, None, 10).iter().map(|a| (a.rel.to_string(), a.simbolo.line)).collect();
        let r2: Vec<(String, u32)> = ranking(&idx, "salvar", None, None, 10).iter().map(|a| (a.rel.to_string(), a.simbolo.line)).collect();
        assert_eq!(r1, r2, "mesma consulta, mesma ordem");
        assert_eq!(r1[0], ("a.rs".to_string(), 1), "igual ao nome e de topo vem antes");
        assert_eq!(r1.len(), 4);
        assert_eq!(ranking(&idx, "salvar", Some("struct"), None, 10).len(), 0, "filtro por tipo");
        assert_eq!(ranking(&idx, "salvar", None, None, 2).len(), 2, "limite");
    }

    #[test]
    fn grep_respects_gitignore_limits_and_context() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), ".gitignore", "oculto/\n");
        put(d.path(), "a.txt", "um\nalvo\ndois\ntres\nalvo\n");
        put(d.path(), "oculto/b.txt", "alvo escondido\n");
        let s = grep(d.path(), "alvo", &op(30, 1, 6000)).unwrap();
        assert!(!s.contains("oculto"), "{s}");
        assert!(s.contains("a.txt:2: alvo") && s.contains("a.txt:1- um") && s.contains("a.txt:3- dois"), "{s}");
        // limite de acertos: só o primeiro
        let um = grep(d.path(), "alvo", &op(1, 0, 6000)).unwrap();
        assert_eq!(um.lines().count(), 1, "{um}");
        // teto de caracteres: corta e avisa
        let cortado = grep(d.path(), "alvo", &op(30, 0, 20)).unwrap();
        assert!(cortado.contains("resposta cortada"), "{cortado}");
        assert!(cortado.len() <= 20 + 60);
    }

    #[test]
    fn grep_cap_counts_only_searched_files() {
        // arquivos fora do glob não são lidos, então não gastam o teto de 5000
        let d = tempfile::tempdir().unwrap();
        for i in 0..5000 {
            put(d.path(), &format!("a/n{i:04}.txt"), "sem nada\n");
        }
        put(d.path(), "b/alvo.rs", "aqui está o alvo\n");
        let mut o = op(30, 0, 6000);
        o.glob = Some(glob::Pattern::new("*.rs").unwrap());
        let s = grep(d.path(), "alvo", &o).unwrap();
        assert!(s.contains("b/alvo.rs:1: aqui está o alvo"), "{s}");
        assert!(!s.contains("busca parcial"), "{s}");
    }

    #[test]
    fn grep_long_lines_are_truncated_and_bad_regex_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        put(d.path(), "longo.txt", &format!("{}alvo\n", "x".repeat(300)));
        let s = grep(d.path(), "alvo", &op(5, 0, 6000)).unwrap();
        assert!(s.lines().next().unwrap().chars().count() < 230, "{s}");
        assert_eq!(grep(d.path(), "(", &op(5, 0, 6000)).unwrap_err().code(), "regex_invalida");
    }

    #[test]
    fn trunca_counts_characters() {
        assert_eq!(trunca("ação", 3), "açã…");
        assert_eq!(trunca("ok", 3), "ok");
    }
}
