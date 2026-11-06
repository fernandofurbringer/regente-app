//! Busca nos painéis (P1.7): o buffer de cada painel e a conversa do Claude, sem diferenciar caixa.
//! Trechos de até `EXCERPT` caracteres; no máximo `MAX_HITS` resultados no total.

use crate::orchestrator::strip_ansi;
use serde::Serialize;
use std::path::Path;

pub const MAX_HITS: usize = 200;
/// Caracteres antes do trecho encontrado, e tamanho do trecho.
const CONTEXT: usize = 70;
const EXCERPT: usize = 180;
/// Bytes lidos do histórico em disco por painel e por busca (o spill tem no máximo 20 MB por painel).
pub const DISK_PER_PANE: usize = 8 * 1024 * 1024;
pub const DISK_TOTAL: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub pane_id: String,
    pub pane_name: String,
    pub workspace_id: String,
    /// "terminal" (buffer), "historico" (disco) ou "conversa" (JSONL do Claude, em Markdown)
    pub source: &'static str,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub hits: Vec<Hit>,
    /// Havia mais resultados do que o limite.
    pub truncated: bool,
    /// O histórico em disco de algum painel é maior que o limite lido: o trecho mais antigo ficou de fora.
    pub disk_limited: bool,
}

/// O painel dono dos trechos de uma fonte.
pub struct Owner {
    pub pane_id: String,
    pub pane_name: String,
    pub workspace_id: String,
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Trecho de `line` em volta da primeira ocorrência de `query` (sem caixa), ou None. Caracteres de controle viram espaço.
/// Caracteres varridos por linha na busca (64 mil: duas cópias ficam em ~512 KB, contra dezenas de MB numa linha gigante).
const LINHA_MAX: usize = 64 * 1024;

pub fn excerpt(line: &str, query: &str) -> Option<String> {
    let q: Vec<char> = query.trim().chars().map(fold).collect();
    if q.is_empty() {
        return None;
    }
    // ponytail: uma "linha" sem quebra (barra de progresso, TUI) pode ter megabytes; só o começo dela é varrido
    let chars: Vec<char> = line.chars().take(LINHA_MAX).collect();
    let folded: Vec<char> = chars.iter().map(|c| fold(*c)).collect();
    let at = folded.windows(q.len()).position(|w| w == q.as_slice())?;
    let start = at.saturating_sub(CONTEXT);
    let end = (start + EXCERPT).min(chars.len());
    let text: String = chars[start..end].iter().map(|c| if c.is_control() { ' ' } else { *c }).collect();
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(text.trim());
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

/// Trechos das linhas de `raw` (ANSI tirado antes) que contêm `query`, até `room` deles.
pub fn scan(raw: &str, query: &str, room: usize) -> Vec<String> {
    let plain = strip_ansi(raw);
    plain.lines().filter_map(|l| excerpt(l.trim_end_matches('\r'), query)).take(room).collect()
}

/// Como `scan`, mas do fim para o começo: os trechos mais recentes primeiro.
pub fn scan_recent(raw: &str, query: &str, room: usize) -> Vec<String> {
    let plain = strip_ansi(raw);
    plain.lines().rev().filter_map(|l| excerpt(l.trim_end_matches('\r'), query)).take(room).collect()
}

/// Busca no histórico em disco de um painel (`scrollback.1.log` + `scrollback.log` em `dir`), lendo no máximo
/// `max_bytes` do fim. Devolve os trechos (mais recentes primeiro, até `room`) e quantos bytes foram lidos.
pub fn scan_disk(dir: &Path, query: &str, max_bytes: usize, room: usize) -> (Vec<String>, usize) {
    let Some(bytes) = crate::spill::tail(dir, max_bytes) else { return (vec![], 0) };
    let read = bytes.len();
    (scan_recent(&String::from_utf8_lossy(&bytes), query, room), read)
}

/// Acrescenta os trechos de uma fonte aos resultados, sem passar de `MAX_HITS`. true = sobrou algum de fora.
pub fn extend(out: &mut Vec<Hit>, owner: &Owner, source: &'static str, excerpts: Vec<String>) -> bool {
    let room = MAX_HITS.saturating_sub(out.len());
    let over = excerpts.len() > room;
    out.extend(excerpts.into_iter().take(room).map(|excerpt| Hit {
        pane_id: owner.pane_id.clone(),
        pane_name: owner.pane_name.clone(),
        workspace_id: owner.workspace_id.clone(),
        source,
        excerpt,
    }));
    over
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> Owner {
        Owner { pane_id: "p1".into(), pane_name: "agente".into(), workspace_id: "w1".into() }
    }

    #[test]
    fn match_ignores_case() {
        assert!(excerpt("Erro FATAL no build", "fatal").is_some());
        assert!(excerpt("tudo certo", "erro").is_none());
        assert_eq!(excerpt("abc", "  "), None);
    }

    #[test]
    fn linha_gigante_so_tem_o_comeco_varrido() {
        let longa = format!("{}ERRO", "x".repeat(LINHA_MAX));
        assert_eq!(excerpt(&longa, "erro"), None, "o casamento depois do teto fica de fora");
        assert!(excerpt(&format!("ERRO{}", "x".repeat(LINHA_MAX * 4)), "erro").is_some());
    }

    #[test]
    fn excerpt_is_cut_around_the_match_with_ellipses() {
        let line = format!("{}ERRO{}", "a".repeat(200), "b".repeat(200));
        let ex = excerpt(&line, "erro").unwrap();
        assert!(ex.starts_with('…') && ex.ends_with('…'), "{ex}");
        assert!(ex.contains("ERRO"));
        assert!(ex.chars().count() <= EXCERPT + 2, "{}", ex.chars().count());
    }

    #[test]
    fn ansi_is_stripped_before_matching_and_control_chars_become_spaces() {
        let raw = "\x1b[31mfalha\x1b[0m aqui\r\n\x1b[1mok\x1b[0m\n";
        assert_eq!(scan(raw, "falha aqui", 10), vec!["falha aqui".to_string()]);
        assert!(scan(raw, "\x1b", 10).is_empty());
        assert!(excerpt("x\u{7}falha", "x falha").is_none());
        assert_eq!(excerpt("a\tb falha", "falha").unwrap(), "a b falha");
    }

    #[test]
    fn scan_respects_room() {
        let raw = "erro 1\nerro 2\nerro 3\n";
        assert_eq!(scan(raw, "erro", 2).len(), 2);
        assert_eq!(scan(raw, "erro", 0).len(), 0);
    }

    #[test]
    fn extend_caps_at_max_and_reports_overflow() {
        let mut out = Vec::new();
        let many: Vec<String> = (0..MAX_HITS).map(|i| format!("linha {i}")).collect();
        assert!(!extend(&mut out, &owner(), "terminal", many));
        assert_eq!(out.len(), MAX_HITS);
        // cheio: mais uma fonte com resultados avisa, e nada passa do limite
        assert!(extend(&mut out, &owner(), "conversa", vec!["mais".into()]));
        assert_eq!(out.len(), MAX_HITS);
    }

    #[test]
    fn extend_partial_fill_then_overflow() {
        let mut out = Vec::new();
        assert!(extend(&mut out, &owner(), "terminal", (0..MAX_HITS - 1).map(|i| i.to_string()).chain(["x".into(), "y".into()]).collect()));
        assert_eq!(out.len(), MAX_HITS);
        assert_eq!(out[0].source, "terminal");
        assert_eq!(out[0].pane_name, "agente");
    }

    #[test]
    fn scan_recent_returns_newest_first() {
        let raw = "erro 1\nok\nerro 2\nerro 3\n";
        assert_eq!(scan_recent(raw, "erro", 2), vec!["erro 3".to_string(), "erro 2".to_string()]);
        assert!(scan_recent(raw, "erro", 0).is_empty());
    }

    /// Histórico com rotação: `.1` = "erro antigo" + linha qualquer; ativo = "erro recente".
    fn spill_com_rotacao() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let mut s = crate::spill::Spill::with_limit(d.path().to_path_buf(), 40);
        s.append(b"erro antigo\n").unwrap(); // 12
        s.append(b"linha qualquer aqui\n").unwrap(); // 20 (total 32)
        s.append(b"erro recente\n").unwrap(); // 13: passaria de 40, o ativo vira .1
        d
    }

    #[test]
    fn scan_disk_reads_both_spill_files_newest_first() {
        let d = spill_com_rotacao();
        let (hits, read) = scan_disk(d.path(), "ERRO", DISK_PER_PANE, 10);
        assert_eq!(hits, vec!["erro recente".to_string(), "erro antigo".to_string()]);
        assert_eq!(read, 45);
    }

    #[test]
    fn scan_disk_respects_the_byte_cap_and_the_room() {
        let d = spill_com_rotacao();
        // 13 bytes = só a cauda ("erro recente\n"): o trecho antigo nem é lido
        let (hits, read) = scan_disk(d.path(), "erro", 13, 10);
        assert_eq!(hits, vec!["erro recente".to_string()]);
        assert_eq!(read, 13);
        let (hits, _) = scan_disk(d.path(), "erro", DISK_PER_PANE, 1);
        assert_eq!(hits, vec!["erro recente".to_string()]);
    }

    #[test]
    fn scan_disk_without_history_is_empty_and_strips_ansi() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(scan_disk(d.path(), "erro", DISK_PER_PANE, 10), (vec![], 0));
        let mut s = crate::spill::Spill::with_limit(d.path().to_path_buf(), 1000);
        s.append(b"\x1b[31mfalha\x1b[0m no build\r\n").unwrap();
        assert_eq!(scan_disk(d.path(), "falha no", DISK_PER_PANE, 10).0, vec!["falha no build".to_string()]);
    }
}
