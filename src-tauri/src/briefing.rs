//! "Me sugere" do briefing da missão: o provedor da IA auxiliar (ou o Decisor) reescreve o rascunho em seções.
//! Usa a mesma chamada de `gitflow` (chave no cofre, endereço validado). Sem chave, erro claro. O texto volta
//! para a tela, e só entra no briefing depois que o usuário aceita.

use crate::error::{fail, AppResult};
use crate::gitflow;
use crate::runtime::AppState;
use serde::Serialize;

/// Teto do rascunho enviado e da resposta aceita.
pub const BRIEF_MAX: usize = 4000;

const BRIEF_SYSTEM: &str = "Você organiza o briefing de uma missão de desenvolvimento de software, em português do Brasil. \
Reescreva o rascunho do usuário nestas seções, nesta ordem: Objetivo; Escopo; Critérios de aceite (lista com hífens). \
Não invente requisitos: quando uma informação faltar, escreva 'a definir'. Responda só com o texto, sem comentários.";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BriefSuggestion {
    pub text: String,
    /// Sempre "ia": sem chave o comando falha com `no_key`, e a tela não grava nada sozinha.
    pub source: String,
}

/// Resposta da IA sem cercas de código e no teto; `None` se vazia.
pub fn clean_brief(text: &str) -> Option<String> {
    let body: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("```")).collect();
    let out: String = body.join("\n").trim().chars().take(BRIEF_MAX).collect();
    (!out.is_empty()).then_some(out)
}

/// Rascunho cortado no teto antes de ir para a IA.
pub fn cap_brief(text: &str) -> String {
    text.chars().take(BRIEF_MAX).collect()
}

pub async fn suggest(s: &AppState, draft: &str) -> AppResult<BriefSuggestion> {
    let draft = draft.trim();
    if draft.is_empty() {
        return fail("empty_brief", "escreva um rascunho antes de pedir sugestão");
    }
    let Some(p) = gitflow::provider(s)? else {
        return fail("no_key", "para 'Me sugere', configure uma chave da IA auxiliar ou do Decisor no Cofre");
    };
    let text = gitflow::complete(&p, BRIEF_SYSTEM, &cap_brief(draft)).await?;
    match clean_brief(&text) {
        Some(text) => Ok(BriefSuggestion { text, source: "ia".into() }),
        None => fail("ai_empty", "a IA não devolveu texto; tente de novo"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_brief_drops_fences_and_empty_answers() {
        assert_eq!(clean_brief("```\n## Objetivo\nx\n```").as_deref(), Some("## Objetivo\nx"));
        assert_eq!(clean_brief("   \n "), None);
    }

    #[test]
    fn long_drafts_and_answers_are_capped() {
        let long = "a".repeat(BRIEF_MAX + 50);
        assert_eq!(cap_brief(&long).chars().count(), BRIEF_MAX);
        assert_eq!(clean_brief(&long).unwrap().chars().count(), BRIEF_MAX);
    }

    #[test]
    fn without_a_key_the_error_says_what_to_configure() {
        let d = tempfile::tempdir().unwrap();
        let s = crate::runtime::AppState::new(d.path().into(), "C:/x/regente.exe".into(), std::sync::Arc::new(|_: &str, _: serde_json::Value| {})).unwrap();
        let err = tauri::async_runtime::block_on(suggest(&*s, "fazer um app de tarefas")).unwrap_err();
        assert_eq!(err.code(), "no_key");
        assert!(err.to_string().contains("Cofre"), "{err}");
    }
}
