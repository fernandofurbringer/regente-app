//! Port do helper.rs do supertone-inc/supertonic (MIT). Ver THIRD_PARTY.md.
//! Pré-processamento do texto (normalização, símbolos, tag de idioma), divisão em blocos e índices unicode.

use crate::error::{fail, AppResult};
use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// Idiomas que o modelo aceita (a lista do original).
pub const IDIOMAS: &[&str] = &[
    "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu", "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi", "na",
];

/// Abreviações que não encerram frase: as do original, mais as do português (que não estão na lista).
const ABREVIACOES: &[&str] = &[
    "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Sr.", "Jr.", "St.", "Ave.", "Rd.", "Blvd.", "Dept.", "Inc.", "Ltd.", "Co.", "Corp.", "etc.", "vs.", "i.e.", "e.g.", "Ph.D.", "Sra.", "Srta.", "Dra.", "Profa.", "pág.", "nº.", "ex.",
];

/// Tamanho máximo de um bloco (em bytes), como no original.
pub const MAX_BLOCO: usize = 300;

static EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x{1F600}-\x{1F64F}\x{1F300}-\x{1F5FF}\x{1F680}-\x{1F6FF}\x{1F700}-\x{1F77F}\x{1F780}-\x{1F7FF}\x{1F800}-\x{1F8FF}\x{1F900}-\x{1F9FF}\x{1FA00}-\x{1FA6F}\x{1FA70}-\x{1FAFF}\x{2600}-\x{26FF}\x{2700}-\x{27BF}\x{1F1E6}-\x{1F1FF}]+").unwrap());
/// Espaço antes de pontuação: o espaço some.
static ESPACO_ANTES_PONTUACAO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" ([,.!?;:'])").unwrap());
static ESPACOS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static FIM_PONTUACAO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[.!?;:,'"\u{201C}\u{201D}\u{2018}\u{2019})\]}…。」』】〉》›»]$"#).unwrap());
static PARAGRAFOS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n").unwrap());
/// Fim de frase: pontuação seguida de espaço (o regex não tem lookbehind; as abreviações são conferidas à parte).
static FIM_FRASE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([.!?])\s+").unwrap());

/// Trocas de símbolos (travessões, aspas tipográficas, colchetes) pelo equivalente do modelo.
const TROCAS: &[(&str, &str)] = &[
    ("–", "-"),
    ("‑", "-"),
    ("—", "-"),
    ("_", " "),
    ("\u{201C}", "\""),
    ("\u{201D}", "\""),
    ("\u{2018}", "'"),
    ("\u{2019}", "'"),
    ("´", "'"),
    ("`", "'"),
    ("[", " "),
    ("]", " "),
    ("|", " "),
    ("/", " "),
    ("#", " "),
    ("→", " "),
    ("←", " "),
];

/// Símbolos que somem do texto.
const SIMBOLOS_REMOVIDOS: &[&str] = &["♥", "☆", "♡", "©", "\\"];

/// Expressões lidas por extenso.
const EXPRESSOES: &[(&str, &str)] = &[("@", " at "), ("e.g.,", "for example, "), ("i.e.,", "that is, ")];

/// Normaliza o texto e envolve com a tag do idioma (`<pt>...</pt>`). Idioma fora da lista é erro.
pub fn preprocessar(texto: &str, idioma: &str) -> AppResult<String> {
    if !IDIOMAS.contains(&idioma) {
        return fail("idioma_invalido", format!("idioma {idioma} não suportado"));
    }
    let mut t: String = texto.nfkd().collect();
    t = EMOJI.replace_all(&t, "").into_owned();
    for (de, para) in TROCAS {
        t = t.replace(de, para);
    }
    for simbolo in SIMBOLOS_REMOVIDOS {
        t = t.replace(simbolo, "");
    }
    for (de, para) in EXPRESSOES {
        t = t.replace(de, para);
    }
    t = ESPACO_ANTES_PONTUACAO.replace_all(&t, "$1").into_owned();
    while t.contains("\"\"") {
        t = t.replace("\"\"", "\"");
    }
    while t.contains("''") {
        t = t.replace("''", "'");
    }
    while t.contains("``") {
        t = t.replace("``", "`");
    }
    t = ESPACOS.replace_all(&t, " ").into_owned();
    let mut t = t.trim().to_string();
    // sem pontuação final, o modelo tende a cortar a fala: põe um ponto
    if !t.is_empty() && !FIM_PONTUACAO.is_match(&t) {
        t.push('.');
    }
    Ok(format!("<{idioma}>{t}</{idioma}>"))
}

/// Divide o texto em blocos de no máximo `max` bytes: por parágrafo, depois por frase, vírgula e, por fim, palavra.
pub fn blocos(texto: &str, max: usize) -> Vec<String> {
    let texto = texto.trim();
    if texto.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    // parágrafos curtos seguidos viram um bloco só (até `max`): muitos parágrafos pequenos não viram muitas inferências
    let mut juntos = String::new();
    for paragrafo in PARAGRAFOS.split(texto).map(str::trim).filter(|p| !p.is_empty()) {
        if paragrafo.len() <= max {
            if !juntos.is_empty() && juntos.len() + paragrafo.len() + 2 > max {
                out.push(std::mem::take(&mut juntos));
            }
            if !juntos.is_empty() {
                juntos.push_str("\n\n");
            }
            juntos.push_str(paragrafo);
            continue;
        }
        if !juntos.is_empty() {
            out.push(std::mem::take(&mut juntos));
        }
        let mut atual = String::new();
        let frases = frases(paragrafo);
        for frase in frases.iter().map(|f| f.trim()).filter(|f| !f.is_empty()) {
            if frase.len() > max {
                // frase longa: fecha o bloco atual e quebra por vírgula
                if !atual.is_empty() {
                    out.push(atual.trim().to_string());
                    atual.clear();
                }
                for parte in frase.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    if parte.len() > max {
                        // último recurso: por palavra; palavra maior que `max` é cortada por caractere
                        let mut palavras = String::new();
                        for w in parte.split_whitespace() {
                            for pedaco in partir(w, max) {
                                if palavras.len() + pedaco.len() + 1 > max && !palavras.is_empty() {
                                    out.push(palavras.trim().to_string());
                                    palavras.clear();
                                }
                                if !palavras.is_empty() {
                                    palavras.push(' ');
                                }
                                palavras.push_str(pedaco);
                            }
                        }
                        if !palavras.is_empty() {
                            out.push(palavras.trim().to_string());
                        }
                    } else {
                        if atual.len() + parte.len() + 1 > max && !atual.is_empty() {
                            out.push(atual.trim().to_string());
                            atual.clear();
                        }
                        if !atual.is_empty() {
                            atual.push_str(", ");
                        }
                        atual.push_str(parte);
                    }
                }
                continue;
            }
            if atual.len() + frase.len() + 1 > max && !atual.is_empty() {
                out.push(atual.trim().to_string());
                atual.clear();
            }
            if !atual.is_empty() {
                atual.push(' ');
            }
            atual.push_str(frase);
        }
        if !atual.is_empty() {
            out.push(atual.trim().to_string());
        }
    }
    if !juntos.is_empty() {
        out.push(juntos);
    }
    if out.is_empty() { vec![String::new()] } else { out }
}

/// Corta uma palavra em pedaços de até `max` bytes, sempre em fronteira de caractere.
fn partir(w: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut inicio = 0;
    for (i, c) in w.char_indices() {
        if i + c.len_utf8() - inicio > max && i > inicio {
            out.push(&w[inicio..i]);
            inicio = i;
        }
    }
    if inicio < w.len() {
        out.push(&w[inicio..]);
    }
    out
}

/// Frases do texto: corta depois de `.`, `!` ou `?` seguido de espaço, exceto depois de abreviação.
fn frases(texto: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inicio = 0;
    for m in FIM_FRASE.find_iter(texto) {
        let combinado = format!("{}{}", texto[inicio..m.start()].trim(), &texto[m.start()..m.start() + 1]);
        if ABREVIACOES.iter().any(|a| combinado.ends_with(a)) {
            continue;
        }
        out.push(texto[inicio..m.end()].to_string());
        inicio = m.end();
    }
    if inicio < texto.len() {
        out.push(texto[inicio..].to_string());
    }
    if out.is_empty() { vec![texto.to_string()] } else { out }
}

/// Índices unicode do texto pelo indexador (-1 = caractere fora da tabela) e a máscara de uns do mesmo tamanho.
pub fn ids_e_mascara(indexador: &[i64], texto: &str) -> (Vec<i64>, Vec<f32>) {
    let ids: Vec<i64> = texto.chars().map(|c| indexador.get(c as usize).copied().unwrap_or(-1)).collect();
    let mascara = vec![1.0; ids.len()];
    (ids, mascara)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocessa_com_tag_de_idioma_e_ponto_final() {
        let esperado: String = "Olá mundo".nfkd().collect();
        assert_eq!(preprocessar("Olá   mundo", "pt").unwrap(), format!("<pt>{esperado}.</pt>"));
        assert!(preprocessar("ok", "xx").is_err());
    }

    #[test]
    fn troca_simbolos_e_tira_emoji() {
        let t = preprocessar("a — b 😀 #tag", "pt").unwrap();
        assert!(t.contains("a - b"));
        assert!(!t.contains('😀') && !t.contains('#'));
    }

    #[test]
    fn blocos_respeitam_o_limite_e_nao_cortam_abreviacao() {
        let longo = "Esta é uma frase. ".repeat(40);
        let b = blocos(&longo, 300);
        assert!(b.len() > 1 && b.iter().all(|c| c.len() <= 300));
        assert_eq!(blocos("O Dr. Silva chegou. Ele saiu.", 300), vec!["O Dr. Silva chegou. Ele saiu.".to_string()]);
        assert_eq!(blocos("   ", 300), vec![String::new()]);
    }

    #[test]
    fn paragrafos_pequenos_se_juntam_e_palavra_longa_se_corta() {
        let muitos = (0..50).map(|i| format!("Linha {i}.")).collect::<Vec<_>>().join("\n\n");
        let b = blocos(&muitos, 300);
        assert!(b.len() <= 3 && b.iter().all(|c| c.len() <= 300), "{} blocos", b.len());
        let grande = "a".repeat(700);
        let b = blocos(&grande, 300);
        assert!(b.len() == 3 && b.iter().all(|c| c.len() <= 300));
        assert_eq!(b.concat(), grande);
        let acento = "ã".repeat(200);
        assert!(blocos(&acento, 300).iter().all(|c| c.len() <= 300));
    }

    #[test]
    fn ids_usam_o_indexador_e_mascara_tem_o_tamanho_do_texto() {
        let mut indexador = vec![0i64; 200];
        for (i, v) in indexador.iter_mut().enumerate() {
            *v = i as i64 + 1000;
        }
        let (ids, mask) = ids_e_mascara(&indexador, "<pt>a.</pt>");
        assert_eq!(ids.len(), "<pt>a.</pt>".chars().count());
        assert_eq!(ids[0], '<' as i64 + 1000);
        assert!(mask.iter().all(|&m| m == 1.0));
        let (ids2, _) = ids_e_mascara(&indexador, "ā");
        assert_eq!(ids2, vec![-1]);
    }
}
