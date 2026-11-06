//! Prompts salvos (Configurações → Prompts): `data/prompts.json`. As variáveis `{{...}}` são expandidas no
//! cliente, na hora de colar; aqui só guardam e validam o texto. Colar usa a mesma limpeza do Jarvis e do ciclo.

use crate::error::{fail, AppResult};
use crate::orchestrator::clean_text;
use crate::settings::save_json;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_PROMPTS: usize = 200;
const MAX_NAME: usize = 80;
const MAX_TEXT: usize = 8000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PromptDef {
    pub id: String,
    pub name: String,
    pub text: String,
    /// Envia Enter depois de colar (o prompt roda sem revisão). Padrão: não.
    pub send_enter: bool,
}

impl Default for PromptDef {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), text: String::new(), send_enter: false }
    }
}

fn file(data: &Path) -> PathBuf {
    data.join("prompts.json")
}

/// Os seis prompts iniciais, gravados na primeira leitura.
pub fn seeds() -> Vec<PromptDef> {
    let s = |id: &str, name: &str, text: &str| PromptDef { id: id.into(), name: name.into(), text: text.into(), send_enter: false };
    vec![
        s("revisar-mudancas", "revisar mudanças", "Revise as mudanças não commitadas deste repositório (git diff). Aponte bugs, riscos de segurança e pontos de simplificação, em ordem de gravidade. Não altere nenhum arquivo."),
        s("testes-arquivo", "escrever testes do arquivo aberto", "Escreva testes para o arquivo {{arquivo}}. Cubra os casos principais e as bordas, siga o padrão de testes que já existe no projeto e rode a suíte ao final."),
        s("explicar-erro", "explicar o erro da área de transferência", "Explique o erro abaixo: diga a causa provável e como corrigir.\n\n{{clipboard}}"),
        s("refatorar-selecao", "refatorar a seleção", "Refatore o trecho abaixo, de {{arquivo}}, mantendo o comportamento:\n\n{{selecao}}"),
        s("mensagem-commit", "gerar mensagem de commit", "Gere uma mensagem de commit em português para as mudanças atuais (git diff --staged; se nada estiver em stage, git diff). Título curto no imperativo e corpo explicando o porquê. Não faça o commit."),
        s("planejar-feature", "planejar a feature", "Antes de codar, planeje a feature descrita abaixo no workspace {{workspace}} (branch {{branch}}): liste os arquivos que mudam, a ordem dos passos e os testes. Espere minha confirmação.\n\n{{clipboard}}"),
    ]
}

/// Prompts salvos. Sem arquivo, grava os iniciais; arquivo ilegível devolve os iniciais sem gravar por cima.
pub fn list(data: &Path) -> AppResult<Vec<PromptDef>> {
    let path = file(data);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("prompts.json ilegível ({e}): usando os iniciais");
            seeds()
        })),
        Err(_) => {
            let s = seeds();
            std::fs::create_dir_all(data)?;
            save_json(&path, &s)?;
            Ok(s)
        }
    }
}

/// Valida e grava a lista inteira. Nome obrigatório e único (sem caixa), texto obrigatório; id vazio ou repetido ganha um novo.
pub fn save(data: &Path, prompts: Vec<PromptDef>) -> AppResult<Vec<PromptDef>> {
    if prompts.len() > MAX_PROMPTS {
        return fail("too_many", format!("no máximo {MAX_PROMPTS} prompts"));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut out = Vec::with_capacity(prompts.len());
    for mut p in prompts {
        let name = p.name.trim().to_string();
        if name.is_empty() {
            return fail("bad_name", "cada prompt precisa de um nome");
        }
        if name.chars().count() > MAX_NAME {
            return fail("bad_name", format!("nome com mais de {MAX_NAME} caracteres: {name}"));
        }
        if !names.insert(name.to_lowercase()) {
            return fail("duplicate", format!("já existe um prompt chamado \"{name}\""));
        }
        let text = clean_text(&p.text);
        if text.trim().is_empty() {
            return fail("bad_text", format!("o prompt \"{name}\" está vazio"));
        }
        if text.chars().count() > MAX_TEXT {
            return fail("bad_text", format!("o prompt \"{name}\" passa de {MAX_TEXT} caracteres"));
        }
        if p.id.is_empty() || p.id.len() > 64 || !ids.insert(p.id.clone()) {
            p.id = crate::util::new_id();
            ids.insert(p.id.clone());
        }
        out.push(PromptDef { id: p.id, name, text, send_enter: p.send_enter });
    }
    std::fs::create_dir_all(data)?;
    save_json(&file(data), &out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, name: &str, text: &str) -> PromptDef {
        PromptDef { id: id.into(), name: name.into(), text: text.into(), send_enter: false }
    }

    #[test]
    fn first_read_seeds_six_prompts_and_writes_them() {
        let d = tempfile::tempdir().unwrap();
        let got = list(d.path()).unwrap();
        assert_eq!(got.len(), 6);
        assert!(got.iter().all(|x| !x.id.is_empty() && !x.text.is_empty()));
        assert!(d.path().join("prompts.json").is_file());
        // the seeds use only variables the client knows
        assert!(got.iter().any(|x| x.text.contains("{{selecao}}")));
    }

    #[test]
    fn save_round_trips_and_cleans_text() {
        let d = tempfile::tempdir().unwrap();
        let saved = save(d.path(), vec![PromptDef { send_enter: true, ..p("", "  teste  ", "olá\x1b[201~\r\nmundo") }]).unwrap();
        assert_eq!(saved[0].name, "teste");
        assert_eq!(saved[0].text, "olá[201~\nmundo");
        assert!(!saved[0].id.is_empty());
        assert!(saved[0].send_enter);
        assert_eq!(list(d.path()).unwrap(), saved);
    }

    #[test]
    fn refuses_empty_duplicate_and_huge() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(save(d.path(), vec![p("a", "  ", "x")]).unwrap_err().code(), "bad_name");
        assert_eq!(save(d.path(), vec![p("a", "x", "   ")]).unwrap_err().code(), "bad_text");
        assert_eq!(save(d.path(), vec![p("a", "Commit", "x"), p("b", "commit", "y")]).unwrap_err().code(), "duplicate");
        assert_eq!(save(d.path(), vec![p("a", "x", &"z".repeat(MAX_TEXT + 1))]).unwrap_err().code(), "bad_text");
    }

    #[test]
    fn repeated_or_missing_id_gets_a_new_one() {
        let d = tempfile::tempdir().unwrap();
        let saved = save(d.path(), vec![p("same", "um", "x"), p("same", "dois", "y")]).unwrap();
        assert_ne!(saved[0].id, saved[1].id);
    }

    #[test]
    fn unreadable_file_is_not_overwritten_on_read() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("prompts.json"), "{não é json").unwrap();
        assert_eq!(list(d.path()).unwrap().len(), 6);
        assert_eq!(std::fs::read_to_string(d.path().join("prompts.json")).unwrap(), "{não é json");
    }
}
