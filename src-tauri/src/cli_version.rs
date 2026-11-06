//! Versão instalada e última versão publicada de uma IA de linha de comando.

use crate::providers;
use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Primeira sequência `x.y[.z]` do texto (`claude --version` imprime `2.1.211 (Claude Code)`).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    for tok in text.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let parts: Vec<&str> = tok.split('.').collect();
        if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
            continue;
        }
        let nums: Option<Vec<u64>> = parts.iter().map(|p| p.parse().ok()).collect();
        if let Some(n) = nums {
            return Some((n[0], n[1], n.get(2).copied().unwrap_or(0)));
        }
    }
    None
}

/// `x.y.z` a partir do texto, para mostrar ao usuário.
pub fn display(text: &str) -> Option<String> {
    parse_version(text).map(|(a, b, c)| format!("{a}.{b}.{c}"))
}

/// `true` só se `latest` é estritamente maior que `current`; sem uma das duas leituras, não há atualização.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Nome de pacote do npm: minúsculas, dígitos e `-._~`, com escopo opcional `@escopo/`; sem `-` ou `.` no início.
/// Vai como argumento de `npm view`, então não pode virar flag nem caminho.
pub fn valid_npm_package(name: &str) -> bool {
    fn part(p: &str) -> bool {
        !p.is_empty()
            && !p.starts_with(['-', '.'])
            && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_' | '~'))
    }
    if name.len() > 214 {
        return false;
    }
    match name.strip_prefix('@') {
        Some(rest) => rest.split_once('/').is_some_and(|(scope, pkg)| part(scope) && part(pkg)),
        None => part(name),
    }
}

/// Roda `program args` (`.cmd` de npm passa pelo cmd.exe) e devolve a saída padrão; `None` se falhar ou estourar o tempo.
pub fn capture(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut env = Vec::new();
    let (prog, full) = providers::wrap_batch(program.to_string(), args.iter().map(|a| a.to_string()).collect(), &mut env).ok()?;
    let mut child = crate::util::cmd(&prog).args(&full).envs(env).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut b = String::new();
        let _ = out.read_to_string(&mut b);
        let _ = tx.send(b);
    });
    let t0 = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            // o neto do cmd (o node do npm) pode segurar o pipe depois que o pai sai: não espera além de 2 s
            return rx.recv_timeout(Duration::from_secs(2)).ok();
        }
        if t0.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_first_version_in_the_output() {
        assert_eq!(parse_version("2.1.211 (Claude Code)"), Some((2, 1, 211)));
        assert_eq!(parse_version("codex-cli 0.9.1\n"), Some((0, 9, 1)));
        assert_eq!(parse_version("v10.4"), Some((10, 4, 0)));
        assert_eq!(parse_version("sem versão aqui"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn newer_means_strictly_greater_by_number_not_text() {
        assert!(is_newer("2.1.300", "2.1.211"));
        assert!(is_newer("2.10.0", "2.9.9"), "compara número a número, não texto");
        assert!(!is_newer("2.1.211", "2.1.211"));
        assert!(!is_newer("2.1.100", "2.1.211"));
        assert!(!is_newer("n/d", "2.1.211"), "sem versão publicada não há atualização");
    }

    #[test]
    fn npm_package_names_follow_the_registry_rule() {
        assert!(valid_npm_package("@anthropic-ai/claude-code"));
        assert!(valid_npm_package("opencode-ai"));
        assert!(!valid_npm_package("Claude-Code"), "maiúscula não existe no registro");
        assert!(!valid_npm_package("-rf"), "não pode virar flag");
        assert!(!valid_npm_package(".oculto"));
        assert!(!valid_npm_package("@escopo"), "escopo sem barra");
        assert!(!valid_npm_package("a/b"), "sem escopo, barra não vale");
        assert!(!valid_npm_package("x y"));
        assert!(!valid_npm_package(""));
    }

    #[test]
    fn display_keeps_three_parts() {
        assert_eq!(display("2.1 (x)").as_deref(), Some("2.1.0"));
        assert_eq!(display("nada"), None);
    }
}
