//! Saídas enxutas (H1-06 + X1.4): a saída de um comando de shell que deu certo vira um resumo no `PostToolUse`
//! (`updatedToolOutput`), e a saída completa vai para `<cwd>/.regente/saidas/`. Falhas nunca são trocadas: o
//! `PostToolUseFailure` só ganha um digest quando o perfil pede (`digestFalhas`). Quem chama falha aberta.
use crate::testes::compacto;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

/// Ferramentas de shell cuja saída é resumida.
pub fn e_shell(tool: &str) -> bool {
    matches!(tool, "Bash" | "PowerShell")
}

/// Teto da pasta `.regente/saidas/` (a mais antiga sai quando passa disso).
pub const LIMITE_PASTA: u64 = 50 * 1024 * 1024;
/// Teto do digest de falha (`additionalContext`).
pub const DIGEST_FALHA_MAX: usize = 1500;

pub struct Cfg {
    /// Só resume saídas acima disso (caracteres/bytes). 0 = desligado.
    pub resumir_acima: usize,
    pub cabeca: usize,
    pub cauda: usize,
    pub max_erros: usize,
}

impl Default for Cfg {
    fn default() -> Self {
        Self { resumir_acima: 12_000, cabeca: 40, cauda: 120, max_erros: 60 }
    }
}

#[derive(Debug, PartialEq)]
pub struct Trimmed {
    pub text: String,
    pub omitted_lines: usize,
}

static ERRO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\w*error|fail(ed|ure)?|panic(ked)?|exception|traceback|fatal|denied|not found)\b|warning: unused").unwrap()
});
static STATUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)exit(ed)?( with)? (code|status)\s*:?\s*-?\d+").unwrap());

/// Resume `text` quando passa de `cfg.resumir_acima`. None = mantém a saída como veio.
pub fn trim(tool: &str, command: &str, text: &str, cfg: &Cfg) -> Option<Trimmed> {
    if !e_shell(tool) || cfg.resumir_acima == 0 || text.len() <= cfg.resumir_acima {
        return None;
    }
    let total = text.lines().count();
    if let Some(r) = compacto::detect(command, text) {
        let dg = compacto::digest(r, text, 4000);
        // parser que não reconhece a saída não resume nada: cai no corte por cabeça/cauda, nunca troca por um aviso
        if dg.reconhecido {
            let d = dg.text();
            let mut out = d.clone();
            // o digest é só o que o parser reconhece: as linhas de erro ficam de fora sem isto
            for l in text.lines().filter(|l| ERRO.is_match(l) && !d.contains(l.trim())).take(cfg.max_erros) {
                out.push('\n');
                out.push_str(&limita(l, 500));
            }
            let shown = out.lines().count();
            return Some(Trimmed { text: out, omitted_lines: total.saturating_sub(shown) });
        }
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= cfg.cabeca + cfg.cauda {
        return None; // poucas linhas longas: cortar por linha não economiza nada
    }
    let meio = &lines[cfg.cabeca..lines.len() - cfg.cauda];
    let erros: Vec<&str> = meio.iter().copied().filter(|l| ERRO.is_match(l)).take(cfg.max_erros).collect();
    let omitted = meio.len() - erros.len();
    let mut out: Vec<String> = lines[..cfg.cabeca].iter().map(|l| limita(l, 500)).collect();
    out.push("[...]".into());
    out.extend(erros.iter().map(|l| limita(l, 500)));
    out.push("[...]".into());
    out.extend(lines[lines.len() - cfg.cauda..].iter().map(|l| limita(l, 500)));
    // o status de saída (se houver) fica no fim, mesmo que esteja no meio
    if let Some(st) = lines.iter().rev().find(|l| STATUS.is_match(l)) {
        if !out.iter().any(|l| l == st) {
            out.push(limita(st, 500));
        }
    }
    Some(Trimmed { text: out.join("\n"), omitted_lines: omitted })
}

/// Digest de uma falha de shell (`error` do `PostToolUseFailure`), no máximo `DIGEST_FALHA_MAX` caracteres.
/// A primeira linha (`Exit code N`) sempre fica.
pub fn digest_falha(command: &str, error: &str) -> String {
    let primeira = error.lines().next().unwrap_or_default();
    let corpo = match compacto::detect(command, error) {
        Some(r) => compacto::digest(r, error, DIGEST_FALHA_MAX).text(),
        None => error.lines().skip(1).filter(|l| ERRO.is_match(l)).take(20).collect::<Vec<_>>().join("\n"),
    };
    let mut out = format!("{primeira}\n{corpo}");
    if out.chars().count() > DIGEST_FALHA_MAX {
        out = out.chars().take(DIGEST_FALHA_MAX - 12).collect::<String>() + "\n[...cortado]";
    }
    out
}

fn limita(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + " [...]"
}

/// Nome de arquivo gerido pelo Regente nesta pasta: só estes entram na retenção e podem ser apagados.
static GERIDO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,128}\.txt$").unwrap());

/// Grava a saída completa em `<root>/.regente/saidas/<id>.txt` e aplica a retenção. `root` é a raiz do workspace
/// (fixada pelo app no spawn, não o `cwd` que o agente informa). None se `root` não for absoluto, o `id` não for seguro,
/// a pasta for um link ou a gravação falhar: sem o arquivo, a saída não é trocada.
pub fn guardar(root: &str, id: &str, text: &str) -> Option<PathBuf> {
    if !Path::new(root).is_absolute() || id.is_empty() || id.len() > 128 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let base = Path::new(root);
    match guardar_em(base, &base.join(".regente").join("saidas"), id, text, LIMITE_PASTA) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("saída não guardada ({e}): {}", base.display());
            None
        }
    }
}

fn recusa_link(p: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if crate::projeto::e_link(&m) => Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "link em .regente/saidas recusado")),
        _ => Ok(()),
    }
}

fn guardar_em(base: &Path, dir: &Path, id: &str, text: &str, limite: u64) -> std::io::Result<PathBuf> {
    // nunca grava nem apaga por link: .regente e saidas não podem ser links, e o alvo não pode ser link
    recusa_link(&base.join(".regente"))?;
    recusa_link(dir)?;
    std::fs::create_dir_all(dir)?;
    // fora do git: a pasta some do status (exclude do repositório, ou um .gitignore que ignora tudo)
    if crate::missions::ensure_excluded(base).is_err() {
        // create_new: não segue link nem sobrescreve um .gitignore que já existe
        let gi = dir.join(".gitignore");
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&gi) {
            Ok(mut f) => std::io::Write::write_all(&mut f, b"*\n")?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let alvo = dir.join(format!("{id}.txt"));
    let novo = text.len() as u64;
    // retenção: só arquivos comuns com nome gerido; a mais antiga sai até caber (o que está sendo gravado não conta)
    let mut outros: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let nome = e.file_name();
        let Some(nome) = nome.to_str().filter(|n| GERIDO.is_match(n)) else { continue };
        let Ok(m) = e.metadata() else { continue }; // lstat: não segue link
        if m.file_type().is_file() && !crate::projeto::e_link(&m) && nome != format!("{id}.txt") {
            outros.push((m.modified().unwrap_or(SystemTime::UNIX_EPOCH), e.path(), m.len()));
        }
    }
    outros.sort_by_key(|(t, _, _)| *t);
    let mut total: u64 = outros.iter().map(|(_, _, n)| n).sum::<u64>() + novo;
    for (_, p, n) in &outros {
        if total <= limite {
            break;
        }
        if std::fs::remove_file(p).is_ok() {
            total -= n;
        }
    }
    // o alvo: nunca sobrescreve link; remove só arquivo comum e grava com create_new
    match std::fs::symlink_metadata(&alvo) {
        Ok(m) if crate::projeto::e_link(&m) || !m.file_type().is_file() => {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "alvo da saída não é arquivo comum"));
        }
        Ok(_) => std::fs::remove_file(&alvo)?,
        Err(_) => {}
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&alvo)?;
    f.write_all(text.as_bytes())?;
    Ok(alvo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Cfg {
        Cfg::default()
    }

    #[test]
    fn short_output_is_left_alone() {
        assert_eq!(trim("Bash", "ls", "ok\n", &cfg()), None);
        let zero = Cfg { resumir_acima: 0, ..Cfg::default() };
        assert_eq!(trim("Bash", "ls", &"x\n".repeat(50_000), &zero), None, "0 = desligado");
    }

    #[test]
    fn other_tools_are_left_alone() {
        assert_eq!(trim("Read", "", &"x\n".repeat(20_000), &cfg()), None);
    }

    #[test]
    fn long_output_keeps_head_tail_and_errors() {
        let mut t = String::new();
        for i in 0..1000 {
            t.push_str(&format!("linha {i}\n"));
        }
        t.push_str("erro: falhou aqui\n");
        t = t.repeat(3);
        t.push_str("[exited with code 1]");
        let r = trim("Bash", "make", &t, &cfg()).unwrap();
        assert!(r.text.starts_with("linha 0\n"));
        assert!(r.text.contains("erro: falhou aqui"), "erro do meio fica");
        assert!(r.text.ends_with("[exited with code 1]"), "status fica");
        assert!(r.omitted_lines > 2000);
        assert!(r.text.len() < t.len() / 3);
    }

    #[test]
    fn runner_digest_is_used_for_passing_cargo_tests() {
        let mut t = String::new();
        for i in 0..800 {
            t.push_str(&format!("test modulo::caso_{i} ... ok\n"));
        }
        t.push_str("test result: ok. 800 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n");
        let r = trim("Bash", "cargo test -q", &t, &cfg()).unwrap();
        assert!(r.text.contains("800 passed"), "{}", r.text);
        assert!(!r.text.contains("caso_5 "), "linhas 'ok' somem");
    }

    #[test]
    fn a_failing_runner_keeps_its_failure() {
        let mut t = String::new();
        for i in 0..800 {
            t.push_str(&format!("test modulo::caso_{i} ... ok\n"));
        }
        t.push_str("test modulo::quebra ... FAILED\n");
        t.push_str("test result: FAILED. 799 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n");
        let r = trim("Bash", "cargo test", &t, &cfg()).unwrap();
        assert!(r.text.contains("modulo::quebra"), "a falha nunca some: {}", r.text);
    }

    #[test]
    fn few_long_lines_are_not_cut() {
        let t = "x".repeat(20_000);
        assert_eq!(trim("Bash", "cat a", &t, &cfg()), None);
    }

    #[test]
    fn failure_digest_is_capped_and_keeps_the_exit_line() {
        let e = format!("Exit code 2\n{}", "erro: coisa\n".repeat(5000));
        let d = digest_falha("make", &e);
        assert!(d.starts_with("Exit code 2\n"));
        assert!(d.chars().count() <= DIGEST_FALHA_MAX);
    }

    #[test]
    fn saved_file_lands_in_regente_saidas_and_bad_inputs_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let cwd = d.path().to_string_lossy().to_string();
        let p = guardar(&cwd, "toolu_01-ab", "conteudo").unwrap();
        assert_eq!(p, d.path().join(".regente").join("saidas").join("toolu_01-ab.txt"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "conteudo");
        assert!(guardar("relativo/pasta", "x", "a").is_none(), "cwd relativo recusado");
        assert!(guardar(&cwd, "../fora", "a").is_none(), "id inseguro recusado");
        assert!(guardar(&cwd, "", "a").is_none());
    }

    #[test]
    fn retention_deletes_the_oldest_files_first() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("saidas");
        for (i, nome) in ["a", "b", "c"].iter().enumerate() {
            guardar_em(d.path(), &dir, nome, &"x".repeat(100), 1000).unwrap();
            // mtimes distintos: o de índice menor é o mais antigo
            let f = std::fs::File::options().write(true).open(dir.join(format!("{nome}.txt"))).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64 * 10)).unwrap();
        }
        // 300 de antigos + 800 novos > 1000: sai só o mais antigo ("a"), e 200 + 800 = 1000 cabe
        guardar_em(d.path(), &dir, "novo", &"y".repeat(800), 1000).unwrap();
        assert!(!dir.join("a.txt").exists(), "o mais antigo saiu");
        assert!(dir.join("c.txt").exists());
        assert!(dir.join("novo.txt").exists());
        // só os .txt geridos (o .gitignore que o Regente põe fora do git não conta)
        let total: u64 = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "txt")).map(|e| e.metadata().unwrap().len()).sum();
        assert!(total <= 1000, "{total}");
    }

    fn ligar(alvo: &Path, link: &Path, dir: bool) -> bool {
        #[cfg(windows)]
        let r = if dir { std::os::windows::fs::symlink_dir(alvo, link) } else { std::os::windows::fs::symlink_file(alvo, link) };
        #[cfg(unix)]
        let r = {
            let _ = dir;
            std::os::unix::fs::symlink(alvo, link)
        };
        r.is_ok()
    }

    #[test]
    fn a_parser_without_summary_falls_back_to_head_and_tail() {
        let mut t = String::new();
        for i in 0..1500 {
            t.push_str(&format!("passo {i} de compilacao\n"));
        }
        t.push_str("fim\n");
        let r = trim("Bash", "cargo run", &t, &cfg()).unwrap();
        assert!(r.text.starts_with("passo 0 "), "{}", r.text);
        assert!(r.text.contains("fim"));
        assert!(!r.text.contains("sem resumo"), "aviso não troca a saída");
    }

    #[test]
    fn a_linked_saidas_folder_gets_nothing_written_or_deleted() {
        let fora = tempfile::tempdir().unwrap();
        let alvo = fora.path().join("alvo");
        std::fs::create_dir_all(&alvo).unwrap();
        std::fs::write(alvo.join("velho.txt"), "x".repeat(10)).unwrap();
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".regente")).unwrap();
        if !ligar(&alvo, &d.path().join(".regente").join("saidas"), true) {
            eprintln!("symlink indisponível; teste de link pulado");
            return;
        }
        assert!(guardar(&d.path().to_string_lossy(), "toolu", &"x".repeat(50)).is_none());
        assert!(!alvo.join("toolu.txt").exists());
        assert!(alvo.join("velho.txt").exists(), "nada é apagado pelo link");
    }

    #[test]
    fn a_foreign_file_in_saidas_is_never_deleted() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(".regente").join("saidas");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notas.md"), "x".repeat(900)).unwrap();
        guardar_em(d.path(), &dir, "novo", &"y".repeat(800), 1000).unwrap();
        assert!(dir.join("notas.md").exists(), "nome fora do padrão fica");
        assert!(dir.join("novo.txt").exists());
    }

    #[test]
    fn a_planted_link_at_the_target_is_refused() {
        let fora = tempfile::tempdir().unwrap();
        let seguro = fora.path().join("seguro.txt");
        std::fs::write(&seguro, "original").unwrap();
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(".regente").join("saidas");
        std::fs::create_dir_all(&dir).unwrap();
        if !ligar(&seguro, &dir.join("toolu.txt"), false) {
            eprintln!("symlink indisponível; teste de link pulado");
            return;
        }
        assert!(guardar(&d.path().to_string_lossy(), "toolu", "ataque").is_none());
        assert_eq!(std::fs::read_to_string(&seguro).unwrap(), "original");
    }

    #[test]
    fn five_megabytes_are_trimmed_fast() {
        let mut t = String::with_capacity(5_200_000);
        for i in 0..90_000 {
            t.push_str(&format!("linha de saida numero {i} com um pouco de texto\n"));
        }
        t.push_str("[exited with code 0]\n");
        let inicio = std::time::Instant::now();
        let r = trim("Bash", "ls -la", &t, &cfg()).unwrap();
        let ms = inicio.elapsed().as_millis();
        assert!(r.text.len() < 20_000);
        eprintln!("trim 5 MB: {ms} ms");
        // o teto de 100 ms vale no release (medido: 17 ms); o debug é lento demais para cravar
        #[cfg(not(debug_assertions))]
        assert!(ms < 100, "{ms} ms");
    }
}
