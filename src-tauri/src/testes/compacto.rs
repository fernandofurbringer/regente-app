//! Feedback compacto de testes e build (X1.4 e H1-06). Pura: reduz a saída de um runner ao que o agente precisa
//! (resumo, nomes das falhas, primeiro erro de cada falha, erros de compilador com arquivo:linha). Linhas que
//! passam e barras de progresso somem. A saída completa continua no painel, para o humano.
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Cargo,
    Vitest,
    Jest,
    Pytest,
    Go,
    Dotnet,
    Tsc,
    Eslint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Falha {
    pub name: String,
    pub first_error: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    pub summary: String,
    pub failures: Vec<Falha>,
    /// `arquivo:linha mensagem`
    pub errors: Vec<String>,
    /// false = o parser não reconheceu a saída (o `summary` é só o aviso)
    pub reconhecido: bool,
    cap: usize,
}

/// Linhas de primeiro erro por falha, no máximo.
const MAX_LINHAS: usize = 20;

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
static PYTEST_SUMMARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^=+ (.*\d+ (?:passed|failed|error)\w*.* in [\d.]+s.*?) =+$").unwrap());
static TSC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\S.*?)\((\d+),(\d+)\): error (TS\d+): (.*)$").unwrap());
static CSC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\S.*?)\((\d+),\d+\): error (CS\d+): (.*)$").unwrap());
static PY_LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\S+\.py):(\d+): (.*)$").unwrap());
static GO_LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\S+\.go):(\d+): (.*)$").unwrap());
static RS_LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-->\s+(\S+?):(\d+)").unwrap());
static ESLINT_ERR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+(\d+):(\d+)\s+error\s+(.+)$").unwrap());

/// Descobre o runner pelo comando (primeiro) e, se o comando não disser, pelo formato da saída.
pub fn detect(command: &str, text: &str) -> Option<Runner> {
    let tokens: Vec<String> = command
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || "&;|".contains(c))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let tem = |n: &str| tokens.iter().any(|t| t == n);
    if tem("tsc") {
        return Some(Runner::Tsc);
    }
    if tem("eslint") {
        return Some(Runner::Eslint);
    }
    if tem("cargo") {
        return Some(Runner::Cargo);
    }
    if tem("vitest") {
        return Some(Runner::Vitest);
    }
    if tem("jest") {
        return Some(Runner::Jest);
    }
    if tem("pytest") {
        return Some(Runner::Pytest);
    }
    if tokens.windows(2).any(|w| w[0] == "go" && w[1] == "test") {
        return Some(Runner::Go);
    }
    if tem("dotnet") {
        return Some(Runner::Dotnet);
    }
    let t = strip_ansi(text);
    if t.contains("test result:") {
        Some(Runner::Cargo)
    } else if t.contains("Test Files") {
        Some(Runner::Vitest)
    } else if t.contains("Test Suites:") {
        Some(Runner::Jest)
    } else if t.contains("test session starts") {
        Some(Runner::Pytest)
    } else if t.contains("--- FAIL:") {
        Some(Runner::Go)
    } else if t.contains("Failed!  -") || t.contains("Passed!  -") {
        Some(Runner::Dotnet)
    } else if TSC.is_match(&t) || t.lines().any(|l| TSC.is_match(l)) {
        Some(Runner::Tsc)
    } else if t.lines().any(|l| l.starts_with('✖')) {
        Some(Runner::Eslint)
    } else {
        None
    }
}

/// Monta o digest. `cap` limita `text()` em caracteres.
pub fn digest(runner: Runner, text: &str, cap: usize) -> Digest {
    let t = strip_ansi(text);
    let lines: Vec<&str> = t.lines().collect();
    let p = match runner {
        Runner::Cargo => cargo(&lines),
        Runner::Vitest => vitest_jest(&lines, false),
        Runner::Jest => vitest_jest(&lines, true),
        Runner::Pytest => pytest(&lines),
        Runner::Go => go(&lines),
        Runner::Dotnet => dotnet(&lines),
        Runner::Tsc => tsc(&lines),
        Runner::Eslint => eslint(&lines),
    };
    let reconhecido = p.summary.is_some();
    Digest {
        summary: p.summary.unwrap_or_else(|| "sem resumo reconhecido".into()),
        failures: p.failures,
        errors: p.errors,
        reconhecido,
        cap,
    }
}

impl Digest {
    /// Texto para o agente, no máximo `cap` caracteres.
    pub fn text(&self) -> String {
        let mut s = format!("{}\n", self.summary);
        for f in &self.failures {
            s.push_str(&format!("FALHOU: {}\n", f.name));
            for l in &f.first_error {
                s.push_str("  ");
                s.push_str(l);
                s.push('\n');
            }
        }
        for e in &self.errors {
            s.push_str("ERRO: ");
            s.push_str(e);
            s.push('\n');
        }
        let s = s.trim_end().to_string();
        if s.chars().count() <= self.cap {
            return s;
        }
        let marca = "\n[...cortado]";
        let n = self.cap.saturating_sub(marca.chars().count());
        let mut out: String = s.chars().take(n).collect();
        out.push_str(marca);
        out
    }
}

#[derive(Default)]
struct Parsed {
    summary: Option<String>,
    failures: Vec<Falha>,
    errors: Vec<String>,
}

fn strip_ansi(s: &str) -> String {
    ANSI.replace_all(s, "").replace('\r', "")
}

/// Coleta até `MAX_LINHAS` linhas não vazias a partir de `from`, parando na primeira que satisfaz `para`.
fn coleta(lines: &[&str], from: usize, para: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for l in lines.iter().skip(from) {
        let tl = l.trim();
        if para(tl) {
            break;
        }
        if !tl.is_empty() {
            out.push(l.trim_end().to_string());
            if out.len() == MAX_LINHAS {
                break;
            }
        }
    }
    out
}

fn cargo(lines: &[&str]) -> Parsed {
    let mut p = Parsed::default();
    let mut resultados: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let tl = l.trim();
        if tl.starts_with("test result:") {
            resultados.push(tl.to_string());
        }
        if let Some(rest) = tl.strip_prefix("test ").and_then(|r| r.strip_suffix(" ... FAILED")) {
            p.failures.push(Falha { name: rest.to_string(), first_error: vec![] });
        }
        // erros de compilação: `error[Exxxx]: msg` seguido de `--> arquivo:linha`
        if tl.starts_with("error[E") || (tl.starts_with("error:") && !tl.starts_with("error: test failed")) {
            let prox = lines.iter().skip(i + 1).take(2).find_map(|x| RS_LOC.captures(x));
            if let Some(c) = prox {
                let msg = tl.splitn(2, ": ").nth(1).unwrap_or(tl);
                let cod = if tl.starts_with("error[") { tl.split(']').next().unwrap_or("").trim_start_matches("error[") } else { "" };
                let rotulo = if cod.is_empty() { String::new() } else { format!("error[{cod}]") };
                p.errors.push(format!("{}:{} {} {}", &c[1], &c[2], if rotulo.is_empty() { "error" } else { &rotulo }, msg).replace("  ", " "));
            }
        }
    }
    for f in &mut p.failures {
        let cab = format!("---- {} stdout ----", f.name);
        if let Some(i) = lines.iter().position(|l| l.trim() == cab) {
            f.first_error = coleta(lines, i + 1, |tl| tl.starts_with("----") || tl == "failures:" || tl.starts_with("test result:"));
        }
    }
    p.summary = if resultados.is_empty() {
        lines.iter().map(|l| l.trim()).find(|l| l.starts_with("error: could not compile")).map(str::to_string)
    } else {
        Some(resultados.join("; "))
    };
    p
}

fn vitest_jest(lines: &[&str], jest: bool) -> Parsed {
    let mut p = Parsed::default();
    let mut resumo: Vec<String> = Vec::new();
    let para = |tl: &str| {
        tl.starts_with("FAIL ")
            || tl.starts_with("PASS ")
            || tl.starts_with('●')
            || tl.starts_with("Test Files")
            || tl.starts_with("Test Suites:")
            || tl.starts_with("Tests ")
            || tl.starts_with("Tests:")
            || tl.starts_with("Snapshots:")
            || tl.starts_with("Time:")
            || tl.starts_with("Duration")
    };
    for (i, l) in lines.iter().enumerate() {
        let tl = l.trim();
        let nome = if jest { tl.strip_prefix("● ") } else { tl.strip_prefix("FAIL ").map(str::trim) };
        if let Some(n) = nome {
            p.failures.push(Falha { name: n.to_string(), first_error: coleta(lines, i + 1, para) });
        }
        if tl.starts_with("Test Files") || tl.starts_with("Test Suites:") || tl.starts_with("Tests ") || tl.starts_with("Tests:") {
            resumo.push(tl.to_string());
        }
    }
    p.summary = if resumo.is_empty() { None } else { Some(resumo.join(" | ")) };
    p
}

fn pytest(lines: &[&str]) -> Parsed {
    let mut p = Parsed::default();
    let mut secoes: Vec<(String, Vec<String>)> = Vec::new();
    let mut atual: Option<usize> = None;
    for l in lines {
        let tl = l.trim();
        if tl.starts_with("___") && tl.ends_with("___") && tl.len() > 6 {
            secoes.push((tl.trim_matches('_').trim().to_string(), Vec::new()));
            atual = Some(secoes.len() - 1);
            continue;
        }
        if tl.starts_with("===") {
            atual = None;
        }
        if let Some(i) = atual {
            if !tl.is_empty() {
                secoes[i].1.push(tl.to_string());
            }
        }
        if let Some(rest) = tl.strip_prefix("FAILED ") {
            let nome = rest.split(" - ").next().unwrap_or(rest).to_string();
            p.failures.push(Falha { name: nome, first_error: vec![] });
        }
        if let Some(c) = PY_LOC.captures(tl) {
            p.errors.push(format!("{}:{} {}", &c[1], &c[2], &c[3]));
        }
        if p.summary.is_none() {
            if let Some(c) = PYTEST_SUMMARY.captures(tl) {
                p.summary = Some(c[1].to_string());
            }
        }
    }
    for f in &mut p.failures {
        let func = f.name.rsplit("::").next().unwrap_or(&f.name).to_string();
        if let Some((_, linhas)) = secoes.iter().find(|(n, _)| *n == func) {
            f.first_error = linhas.iter().take(MAX_LINHAS).cloned().collect();
        }
    }
    p
}

fn go(lines: &[&str]) -> Parsed {
    let mut p = Parsed::default();
    let mut resumo: Vec<String> = Vec::new();
    let mut bloco_inicio = 0usize;
    for (i, l) in lines.iter().enumerate() {
        let tl = l.trim();
        if tl.starts_with("=== RUN ") {
            bloco_inicio = i + 1;
        }
        if let Some(rest) = tl.strip_prefix("--- FAIL: ") {
            let nome = rest.split(' ').next().unwrap_or(rest).to_string();
            let primeiro: Vec<String> = lines[bloco_inicio..i]
                .iter()
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty() && !x.starts_with("=== "))
                .take(MAX_LINHAS)
                .collect();
            p.failures.push(Falha { name: nome, first_error: primeiro });
        }
        if let Some(c) = GO_LOC.captures(l) {
            p.errors.push(format!("{}:{} {}", &c[1], &c[2], &c[3]));
        }
        if (tl.starts_with("ok") || tl.starts_with("FAIL")) && tl.contains('\t') {
            resumo.push(tl.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    p.summary = if resumo.is_empty() { None } else { Some(resumo.join("; ")) };
    p
}

fn dotnet(lines: &[&str]) -> Parsed {
    let mut p = Parsed::default();
    let mut atual: Option<usize> = None;
    let mut coletando = false;
    let para = |tl: &str| tl.starts_with("Stack Trace") || tl.starts_with("Failed ") || tl.starts_with("Passed!") || tl.starts_with("Failed!") || tl.starts_with("Standard Output");
    for l in lines {
        let tl = l.trim();
        if let Some(rest) = tl.strip_prefix("Failed ").filter(|_| !tl.starts_with("Failed!")) {
            let nome = rest.split(" [").next().unwrap_or(rest).to_string();
            p.failures.push(Falha { name: nome, first_error: vec![] });
            atual = Some(p.failures.len() - 1);
            coletando = false;
        }
        if tl == "Error Message:" {
            coletando = true;
            continue;
        }
        if coletando {
            if para(tl) {
                coletando = false;
            } else if !tl.is_empty() {
                if let Some(i) = atual {
                    if p.failures[i].first_error.len() < MAX_LINHAS {
                        p.failures[i].first_error.push(tl.to_string());
                    }
                }
            }
        }
        if let Some(c) = CSC.captures(tl) {
            p.errors.push(format!("{}:{} {} {}", &c[1], &c[2], &c[3], &c[4]));
        }
        if (tl.starts_with("Failed!") || tl.starts_with("Passed!")) && p.summary.is_none() {
            p.summary = Some(tl.to_string());
        }
    }
    p
}

fn tsc(lines: &[&str]) -> Parsed {
    let mut p = Parsed::default();
    for l in lines {
        if let Some(c) = TSC.captures(l.trim_end()) {
            p.errors.push(format!("{}:{} {} {}", &c[1], &c[2], &c[4], &c[5]));
        }
        if l.starts_with("Found ") {
            p.summary = Some(l.trim().to_string());
        }
    }
    if p.summary.is_none() {
        p.summary = Some(if p.errors.is_empty() { "sem erros".into() } else { format!("{} erro(s)", p.errors.len()) });
    }
    p
}

fn eslint(lines: &[&str]) -> Parsed {
    let mut p = Parsed::default();
    let mut arquivo = String::new();
    for l in lines {
        let tl = l.trim();
        if tl.starts_with('✖') {
            p.summary = Some(tl.to_string());
        } else if !l.starts_with(' ') && !tl.is_empty() {
            arquivo = tl.to_string();
        } else if let Some(c) = ESLINT_ERR.captures(l) {
            p.errors.push(format!("{}:{} {}", arquivo, &c[1], &c[3]));
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testes::fixture;

    fn dg(runner: Runner, nome: &str) -> Digest {
        digest(runner, &fixture(nome), 2500)
    }

    #[test]
    fn cargo_passa_sem_falhas() {
        let d = dg(Runner::Cargo, "cargo_ok");
        assert_eq!(d.summary, "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s");
        assert!(d.failures.is_empty() && d.errors.is_empty());
    }

    #[test]
    fn cargo_falha_traz_nomes_e_primeiro_erro() {
        let d = dg(Runner::Cargo, "cargo_fail");
        let nomes: Vec<&str> = d.failures.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(nomes, ["formato::tests::quebra_linha", "testes::compacto::tests::soma"]);
        assert_eq!(d.failures[0].first_error[0], "thread 'formato::tests::quebra_linha' panicked at src/formato.rs:88:9:");
        assert!(d.failures[0].first_error.iter().any(|l| l.contains("right: \"b\"")));
        assert!(d.summary.starts_with("test result: FAILED. 1 passed; 2 failed"));
        assert!(!d.text().contains("\u{1b}"));
    }

    #[test]
    fn cargo_erro_de_compilacao_traz_arquivo_e_linha() {
        let d = dg(Runner::Cargo, "cargo_build_fail");
        assert_eq!(d.errors, vec!["src/formato.rs:12 error[E0308] mismatched types", "src/execmatch.rs:7 error[E0425] cannot find value `foo` in this scope"]);
        assert_eq!(d.summary, "error: could not compile `regente` (lib test) due to 2 previous errors");
    }

    #[test]
    fn vitest_passa_e_falha() {
        let ok = dg(Runner::Vitest, "vitest_ok");
        assert_eq!(ok.summary, "Test Files  2 passed (2) | Tests  20 passed (20)");
        assert!(ok.failures.is_empty());
        let f = dg(Runner::Vitest, "vitest_fail");
        assert_eq!(f.failures.len(), 1);
        assert_eq!(f.failures[0].name, "src/lib/errors.test.ts > errors > detecta ts");
        assert_eq!(f.failures[0].first_error[0], "AssertionError: expected [] to have a length of 1 but got +0");
        assert_eq!(f.summary, "Test Files  1 failed | 1 passed (2) | Tests  1 failed | 19 passed (20)");
    }

    #[test]
    fn jest_passa_e_falha() {
        let ok = dg(Runner::Jest, "jest_ok");
        assert_eq!(ok.summary, "Test Suites: 2 passed, 2 total | Tests:       20 passed, 20 total");
        assert!(ok.failures.is_empty());
        let f = dg(Runner::Jest, "jest_fail");
        assert_eq!(f.failures.len(), 1);
        assert_eq!(f.failures[0].name, "errors › detecta ts");
        assert!(f.failures[0].first_error.iter().any(|l| l.contains("Received length: 0")));
        assert!(!f.failures[0].first_error.iter().any(|l| l.starts_with("Test Suites")));
    }

    #[test]
    fn pytest_passa_e_falha() {
        let ok = dg(Runner::Pytest, "pytest_ok");
        assert_eq!(ok.summary, "2 passed in 0.02s");
        assert!(ok.failures.is_empty());
        let f = dg(Runner::Pytest, "pytest_fail");
        assert_eq!(f.failures, vec![Falha { name: "tests/test_calc.py::test_soma".into(), first_error: vec![
            "def test_soma():".into(), ">       assert soma(1, 2) == 4".into(), "E       assert 3 == 4".into(),
            "E        +  where 3 = soma(1, 2)".into(), "tests/test_calc.py:8: AssertionError".into(),
        ] }]);
        assert_eq!(f.summary, "1 failed, 2 passed in 0.12s");
        assert_eq!(f.errors, vec!["tests/test_calc.py:8 AssertionError"]);
    }

    #[test]
    fn go_passa_e_falha() {
        let ok = dg(Runner::Go, "go_ok");
        assert_eq!(ok.summary, "ok regente/pkg/a 0.123s; ok regente/pkg/b (cached)");
        assert!(!ok.text().contains("TestOutro"));
        assert!(ok.failures.is_empty());
        let f = dg(Runner::Go, "go_fail");
        assert_eq!(f.failures, vec![Falha { name: "TestSoma".into(), first_error: vec!["a_test.go:12: esperado 4, obteve 3".into()] }]);
        assert_eq!(f.summary, "FAIL regente/pkg/a 0.210s");
        assert_eq!(f.errors, vec!["a_test.go:12 esperado 4, obteve 3"]);
    }

    #[test]
    fn dotnet_passa_e_falha() {
        let ok = dg(Runner::Dotnet, "dotnet_ok");
        assert!(ok.summary.starts_with("Passed!  - Failed:     0"));
        assert!(ok.failures.is_empty());
        let f = dg(Runner::Dotnet, "dotnet_fail");
        assert_eq!(f.failures.len(), 1);
        assert_eq!(f.failures[0].name, "Regente.Tests.CalcTests.Soma");
        assert_eq!(f.failures[0].first_error, vec!["Assert.Equal() Failure: Valores diferentes", "Expected: 4", "Actual:   3"]);
        assert_eq!(f.errors, vec!["/src/Tests/CalcTests.cs:14 CS1002 ; expected"]);
    }

    #[test]
    fn tsc_passa_e_falha() {
        let ok = dg(Runner::Tsc, "tsc_ok");
        assert_eq!(ok.summary, "sem erros");
        let f = dg(Runner::Tsc, "tsc_fail");
        assert_eq!(f.errors, vec![
            "src/lib/errors.ts:12 TS2322 Type 'string' is not assignable to type 'number'.",
            "src/lib/errors.ts:20 TS2339 Property 'foo' does not exist on type 'Block'.",
        ]);
        assert_eq!(f.summary, "Found 2 errors in the same file, starting at: src/lib/errors.ts:12");
    }

    #[test]
    fn eslint_passa_e_falha() {
        let ok = dg(Runner::Eslint, "eslint_ok");
        assert_eq!(ok.summary, "sem resumo reconhecido");
        assert!(ok.errors.is_empty());
        let f = dg(Runner::Eslint, "eslint_fail");
        assert_eq!(f.errors, vec!["/repo/src/lib/errors.ts:12 'x' is defined but never used  @typescript-eslint/no-unused-vars"]);
        assert_eq!(f.summary, "✖ 2 problems (1 error, 1 warning)");
    }

    #[test]
    fn texto_respeita_o_cap_e_marca_o_corte() {
        let d = digest(Runner::Cargo, &fixture("cargo_fail"), 120);
        let t = d.text();
        assert!(t.chars().count() <= 120, "{} chars", t.chars().count());
        assert!(t.ends_with("[...cortado]"));
        let inteiro = dg(Runner::Cargo, "cargo_fail").text();
        assert!(inteiro.chars().count() <= 2500);
        assert!(!inteiro.ends_with("[...cortado]"));
    }

    #[test]
    fn texto_ocupa_menos_que_a_saida_crua() {
        for (r, n) in [(Runner::Cargo, "cargo_fail"), (Runner::Vitest, "vitest_fail"), (Runner::Pytest, "pytest_fail")] {
            assert!(dg(r, n).text().len() < fixture(n).len(), "{n}");
        }
    }

    #[test]
    fn detect_pelo_comando_e_pela_saida() {
        let jest = fixture("jest_fail");
        let vitest = fixture("vitest_ok");
        let casos: Vec<(&str, &str, Option<Runner>)> = vec![
            ("cargo test -q", "", Some(Runner::Cargo)),
            ("npx vitest run", "", Some(Runner::Vitest)),
            ("npx jest --ci", "", Some(Runner::Jest)),
            ("python -m pytest -q", "", Some(Runner::Pytest)),
            ("go test ./...", "", Some(Runner::Go)),
            ("dotnet test", "", Some(Runner::Dotnet)),
            ("npx tsc --noEmit", "", Some(Runner::Tsc)),
            ("npx eslint src", "", Some(Runner::Eslint)),
            ("npm test", &jest, Some(Runner::Jest)),
            ("npm test", &vitest, Some(Runner::Vitest)),
            ("make", "nada aqui", None),
        ];
        for (cmd, texto, esperado) in casos {
            assert_eq!(detect(cmd, texto), esperado, "{cmd}");
        }
    }
}
