//! Testes relacionados primeiro (X1.3). Pura: detecta o runner pelos arquivos do projeto e monta os comandos
//! para os arquivos alterados. Sem casos reconhecidos, cai no comando da suíte completa.
use serde_json::{Map, Value};
use std::path::Path;

/// Runner de testes detectado. `Cargo` carrega o caminho relativo do `Cargo.toml` (crates aninhados).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runner {
    Cargo { manifest: String },
    Vitest,
    Jest,
    Pytest,
    Go,
    Dotnet,
}

impl Runner {
    /// Chave do template em `projeto.json` (`comandos.relacionados`).
    pub fn chave(&self) -> &'static str {
        match self {
            Runner::Cargo { .. } => "cargo",
            Runner::Vitest => "vitest",
            Runner::Jest => "jest",
            Runner::Pytest => "pytest",
            Runner::Go => "go",
            Runner::Dotnet => "dotnet",
        }
    }
}

const PULAR: [&str; 9] = ["node_modules", "target", "venv", ".venv", "__pycache__", "bin", "obj", "dist", "build"];
const PROFUNDIDADE: usize = 4;

/// Detecta os runners do projeto. Procura os manifestos até 4 níveis; pastas de dependência e de build são puladas.
pub fn detect_runner(root: &Path) -> Vec<Runner> {
    let mut achados = Vec::new();
    walk(root, root, 0, &mut achados);
    achados.sort();
    let mut out: Vec<Runner> = Vec::new();
    let mut tem_pytest_ini = false;
    let mut pyproject_ou_req: Vec<std::path::PathBuf> = Vec::new();
    for (rel, nome) in &achados {
        match nome.as_str() {
            "Cargo.toml" => push_uniq(&mut out, Runner::Cargo { manifest: rel.clone() }),
            "go.mod" => push_uniq(&mut out, Runner::Go),
            "pytest.ini" => tem_pytest_ini = true,
            "pyproject.toml" => pyproject_ou_req.push(root.join(rel)),
            "package.json" => {
                if let Ok(txt) = std::fs::read_to_string(root.join(rel)) {
                    for r in node_runners(&txt) {
                        push_uniq(&mut out, r);
                    }
                }
            }
            n if n.ends_with(".csproj") => push_uniq(&mut out, Runner::Dotnet),
            n if n.starts_with("requirements") && n.ends_with(".txt") => pyproject_ou_req.push(root.join(rel)),
            _ => {}
        }
    }
    if tem_pytest_ini || pyproject_ou_req.iter().any(|p| std::fs::read_to_string(p).map(|t| t.contains("pytest")).unwrap_or(false)) {
        push_uniq(&mut out, Runner::Pytest);
    }
    out
}

fn push_uniq(out: &mut Vec<Runner>, r: Runner) {
    if !out.contains(&r) {
        out.push(r);
    }
}

/// Vitest e Jest a partir de `dependencies`, `devDependencies` e `scripts` de um `package.json`.
pub fn node_runners(package_json: &str) -> Vec<Runner> {
    let Ok(v) = serde_json::from_str::<Value>(package_json) else { return Vec::new() };
    let deps: Vec<&str> = ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|c| v.get(*c).and_then(Value::as_object))
        .flat_map(|m| m.keys().map(String::as_str))
        .collect();
    let scripts: Vec<&str> = v
        .get("scripts")
        .and_then(Value::as_object)
        .map(|s| s.values().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let usa = |nome: &str| {
        deps.contains(&nome)
            || scripts
                .iter()
                .any(|sc| sc.split(|c: char| c.is_whitespace() || "&;|".contains(c)).any(|t| t == nome))
    };
    let mut out = Vec::new();
    if usa("vitest") {
        out.push(Runner::Vitest);
    }
    if usa("jest") {
        out.push(Runner::Jest);
    }
    out
}

fn walk(root: &Path, dir: &Path, nivel: usize, out: &mut Vec<(String, String)>) {
    let Ok(it) = std::fs::read_dir(dir) else { return };
    for e in it.flatten() {
        let nome = e.file_name().to_string_lossy().into_owned();
        let Ok(tipo) = e.file_type() else { continue };
        if tipo.is_dir() {
            if nivel < PROFUNDIDADE && !nome.starts_with('.') && !PULAR.contains(&nome.as_str()) {
                walk(root, &e.path(), nivel + 1, out);
            }
        } else if tipo.is_file() {
            let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/");
            out.push((rel, nome));
        }
    }
}

// ---------- montagem dos comandos ----------

/// Comando da suíte completa, usado quando não há relacionados.
pub fn full(runner: &Runner) -> String {
    match runner {
        Runner::Cargo { manifest } => format!("cargo test --manifest-path {manifest} -q"),
        Runner::Vitest => "npx vitest run".into(),
        Runner::Jest => "npx jest".into(),
        Runner::Pytest => "pytest -q".into(),
        Runner::Go => "go test ./...".into(),
        Runner::Dotnet => "dotnet test".into(),
    }
}

fn default_template(runner: &Runner) -> &'static str {
    match runner {
        Runner::Cargo { .. } => "cargo test --manifest-path {crate} -q {filtro}",
        Runner::Vitest => "npx vitest related {arquivos} --run",
        Runner::Jest => "npx jest --findRelatedTests {arquivos}",
        Runner::Pytest => "pytest -q {arquivos}",
        Runner::Go => "go test {pacotes}",
        Runner::Dotnet => "dotnet test --filter \"{filtro}\"",
    }
}

/// Unidade de execução: um comando depois de renderizado o template.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct Unidade {
    arquivos: Vec<String>,
    filtro: String,
    pacotes: Vec<String>,
}

/// Comandos de testes relacionados aos `changed` (caminhos relativos à raiz). Templates de `projeto.json`
/// (`{arquivos}`, `{filtro}`, `{crate}`, `{pacotes}`) substituem o padrão do runner. Sem relacionados, ou com
/// caminhos que não dá para citar com segurança, devolve o comando da suíte completa.
pub fn related_tests(runner: &Runner, root: &Path, changed: &[String], templates: Option<&Map<String, Value>>) -> Vec<String> {
    let changed: Vec<String> = changed.iter().map(|c| c.trim().replace('\\', "/")).filter(|c| !c.is_empty()).collect();
    if changed.is_empty() || changed.iter().any(|c| !seguro(c)) {
        return vec![full(runner)];
    }
    let unidades = unidades(runner, root, &changed);
    // filtro, crate e nomes de teste entram no comando sem citação: só o conjunto seguro, senão a suíte inteira
    let campos_ok = unidades.iter().all(|u| u.filtro.split_whitespace().all(campo_seguro)) && runner_crate_seguro(runner);
    if !campos_ok {
        return vec![full(runner)];
    }
    if unidades.is_empty() {
        if *runner == Runner::Pytest {
            return vec!["pytest -q -x --lf".into()];
        }
        return vec![full(runner)];
    }
    let modelo = templates
        .and_then(|t| t.get(runner.chave()))
        .and_then(Value::as_str)
        .unwrap_or(default_template(runner));
    let mut out: Vec<String> = Vec::new();
    for u in &unidades {
        let cmd = render(modelo, runner, u);
        if !out.contains(&cmd) {
            out.push(cmd);
        }
    }
    out
}

fn render(modelo: &str, runner: &Runner, u: &Unidade) -> String {
    let crate_ = match runner {
        Runner::Cargo { manifest } => manifest.clone(),
        _ => String::new(),
    };
    let arquivos = u.arquivos.iter().map(|a| citar(a)).collect::<Vec<_>>().join(" ");
    let pacotes = u.pacotes.iter().map(|a| citar(a)).collect::<Vec<_>>().join(" ");
    let s = modelo
        .replace("{arquivos}", &arquivos)
        .replace("{filtro}", &u.filtro)
        .replace("{crate}", &crate_)
        .replace("{pacotes}", &pacotes);
    // placeholders vazios deixam espaços duplos; a citação de caminhos não é tocada porque é feita antes.
    let mut s = s.trim().to_string();
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    s
}

fn unidades(runner: &Runner, root: &Path, changed: &[String]) -> Vec<Unidade> {
    match runner {
        Runner::Cargo { manifest } => cargo_unidades(manifest, changed),
        Runner::Vitest | Runner::Jest => {
            let arq: Vec<String> = changed.iter().filter(|c| js_like(c)).cloned().collect();
            if arq.is_empty() { vec![] } else { vec![Unidade { arquivos: arq, ..Default::default() }] }
        }
        Runner::Pytest => {
            let mut arq: Vec<String> = Vec::new();
            for c in changed.iter().filter(|c| c.ends_with(".py")) {
                let nome = c.rsplit('/').next().unwrap_or(c);
                if nome.starts_with("test_") && nome.ends_with(".py") {
                    push_str_uniq(&mut arq, c.clone());
                    continue;
                }
                let stem = nome.trim_end_matches(".py");
                let cand = format!("tests/test_{stem}.py");
                if root.join(&cand).is_file() {
                    push_str_uniq(&mut arq, cand);
                }
            }
            if arq.is_empty() { vec![] } else { vec![Unidade { arquivos: arq, ..Default::default() }] }
        }
        Runner::Go => {
            let mut pac: Vec<String> = Vec::new();
            for c in changed.iter().filter(|c| c.ends_with(".go")) {
                let dir = match c.rfind('/') {
                    Some(i) => &c[..i],
                    None => "",
                };
                let p = if dir.is_empty() { ".".to_string() } else { format!("./{dir}") };
                push_str_uniq(&mut pac, p);
            }
            if pac.is_empty() { vec![] } else { vec![Unidade { pacotes: pac, ..Default::default() }] }
        }
        Runner::Dotnet => {
            let mut stems: Vec<String> = Vec::new();
            for c in changed.iter().filter(|c| c.ends_with(".cs")) {
                let nome = c.rsplit('/').next().unwrap_or(c);
                push_str_uniq(&mut stems, nome.trim_end_matches(".cs").to_string());
            }
            if stems.is_empty() {
                vec![]
            } else {
                let filtro = stems.iter().map(|s| format!("FullyQualifiedName~{s}")).collect::<Vec<_>>().join("|");
                vec![Unidade { arquivos: vec![], filtro, pacotes: vec![] }]
            }
        }
    }
}

fn cargo_unidades(manifest: &str, changed: &[String]) -> Vec<Unidade> {
    let dir = match manifest.rfind('/') {
        Some(i) => format!("{}/", &manifest[..i]),
        None => String::new(),
    };
    let mut filtros: Vec<String> = Vec::new();
    for c in changed.iter().filter(|c| c.starts_with(&dir)) {
        let rel = &c[dir.len()..];
        if rel == "Cargo.toml" {
            return vec![Unidade::default()]; // manifesto mudou: crate inteiro
        }
        if !rel.ends_with(".rs") {
            continue;
        }
        if let Some(t) = rel.strip_prefix("tests/") {
            if !t.contains('/') {
                let nome = t.trim_end_matches(".rs");
                push_str_uniq(&mut filtros, format!("--test {nome}"));
                continue;
            }
            return vec![Unidade::default()]; // subpasta de testes: crate inteiro
        }
        let nome = rel.rsplit('/').next().unwrap_or(rel).trim_end_matches(".rs");
        if matches!(nome, "lib" | "main" | "mod") {
            return vec![Unidade::default()];
        }
        push_str_uniq(&mut filtros, nome.to_string());
    }
    filtros.into_iter().map(|f| Unidade { filtro: f, ..Default::default() }).collect()
}

fn js_like(c: &str) -> bool {
    [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"].iter().any(|e| c.ends_with(e))
}

fn push_str_uniq(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// Caminho seguro para o comando: sem aspas, NUL, quebra de linha nem metacaracteres de shell (`$`, `` ` ``, `%`, `!`,
/// `&`, `|`, `<`, `>`, `^`, `;`, parênteses).
fn seguro(c: &str) -> bool {
    !c.chars().any(|ch| matches!(ch, '"' | '\0' | '\n' | '\r' | '$' | '`' | '%' | '!' | '&' | '|' | '<' | '>' | '^' | ';' | '(' | ')'))
}

/// Token que pode ir sem citação no comando: letras, números e `_./@+:=~-`.
fn campo_seguro(t: &str) -> bool {
    t.is_empty() || t.chars().all(|c| c.is_ascii_alphanumeric() || "_./@+:=~-".contains(c))
}

/// O crate do manifesto de cargo (`{crate}`) também vai sem citação.
fn runner_crate_seguro(runner: &Runner) -> bool {
    match runner {
        Runner::Cargo { manifest } => manifest.split_whitespace().all(campo_seguro),
        _ => true,
    }
}

/// Cita um caminho para bash e cmd: texto simples passa direto, o resto vai entre aspas duplas.
fn citar(c: &str) -> String {
    let simples = c.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '@' | '+' | ':' | '='));
    if simples && !c.is_empty() { c.to_string() } else { format!("\"{c}\"") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn proj(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        d
    }

    #[test]
    fn detecta_runners_pelos_manifestos() {
        let d = proj(&[
            ("package.json", r#"{"devDependencies":{"vitest":"^5"},"scripts":{"test":"jest"}}"#),
            ("Cargo.toml", "[package]\nname=\"x\""),
            ("crates/a/Cargo.toml", "[package]"),
            ("go.mod", "module x"),
            ("pytest.ini", "[pytest]"),
            ("app/App.csproj", "<Project/>"),
            ("node_modules/x/Cargo.toml", "[package]"),
        ]);
        let r = detect_runner(d.path());
        assert!(r.contains(&Runner::Vitest));
        assert!(r.contains(&Runner::Jest));
        assert!(r.contains(&Runner::Cargo { manifest: "Cargo.toml".into() }));
        assert!(r.contains(&Runner::Cargo { manifest: "crates/a/Cargo.toml".into() }));
        assert!(r.contains(&Runner::Go));
        assert!(r.contains(&Runner::Pytest));
        assert!(r.contains(&Runner::Dotnet));
        assert!(!r.contains(&Runner::Cargo { manifest: "node_modules/x/Cargo.toml".into() }));
    }

    #[test]
    fn pytest_por_requirements_e_sem_pistas() {
        let d = proj(&[("requirements-dev.txt", "pytest==8.3\n")]);
        assert_eq!(detect_runner(d.path()), vec![Runner::Pytest]);
        let vazio = proj(&[("README.md", "x")]);
        assert!(detect_runner(vazio.path()).is_empty());
    }

    #[test]
    fn node_runners_ignora_pacote_com_nome_parecido() {
        assert_eq!(node_runners(r#"{"dependencies":{"vitest-utils":"1"}}"#), vec![]);
        assert_eq!(node_runners(r#"{"scripts":{"t":"npx vitest run"}}"#), vec![Runner::Vitest]);
        assert_eq!(node_runners("não é json"), vec![]);
    }

    #[test]
    fn tabela_de_comandos_por_runner() {
        let d = proj(&[("tests/test_calc.py", ""), ("src/calc.py", ""), ("pkg/a/a.go", ""), ("src/Calc.cs", "")]);
        let root = d.path();
        let cargo = Runner::Cargo { manifest: "Cargo.toml".into() };
        let casos: Vec<(Runner, Vec<&str>, Vec<&str>)> = vec![
            (cargo.clone(), vec!["src/formato.rs"], vec!["cargo test --manifest-path Cargo.toml -q formato"]),
            (cargo.clone(), vec!["tests/integra.rs"], vec!["cargo test --manifest-path Cargo.toml -q --test integra"]),
            (cargo.clone(), vec!["src/lib.rs"], vec!["cargo test --manifest-path Cargo.toml -q"]),
            (cargo.clone(), vec!["src/a.rs", "src/mod.rs"], vec!["cargo test --manifest-path Cargo.toml -q"]),
            (cargo.clone(), vec!["docs/x.md"], vec!["cargo test --manifest-path Cargo.toml -q"]),
            (Runner::Vitest, vec!["src/lib/errors.ts", "README.md"], vec!["npx vitest related src/lib/errors.ts --run"]),
            (Runner::Vitest, vec!["README.md"], vec!["npx vitest run"]),
            (Runner::Jest, vec!["src/a.js"], vec!["npx jest --findRelatedTests src/a.js"]),
            (Runner::Pytest, vec!["src/calc.py"], vec!["pytest -q tests/test_calc.py"]),
            (Runner::Pytest, vec!["src/outro.py"], vec!["pytest -q -x --lf"]),
            (Runner::Go, vec!["pkg/a/a.go", "main.go"], vec!["go test ./pkg/a ."]),
            (Runner::Dotnet, vec!["src/Calc.cs"], vec!["dotnet test --filter \"FullyQualifiedName~Calc\""]),
            (Runner::Dotnet, vec![], vec!["dotnet test"]),
        ];
        for (runner, changed, esperado) in casos {
            let changed = s(&changed);
            let got = related_tests(&runner, root, &changed, None);
            let esperado: Vec<String> = esperado.iter().map(|x| x.to_string()).collect();
            assert_eq!(got, esperado, "runner {runner:?} changed {changed:?}");
        }
    }

    #[test]
    fn go_sem_arquivo_go_cai_na_suite_completa() {
        let d = proj(&[]);
        assert_eq!(related_tests(&Runner::Go, d.path(), &s(&["go.mod"]), None), vec!["go test ./...".to_string()]);
    }

    #[test]
    fn caminho_com_aspas_ou_metacaractere_cai_na_suite_completa() {
        let d = proj(&[]);
        let r = Runner::Vitest;
        assert_eq!(related_tests(&r, d.path(), &s(&["a\"b.ts"]), None), vec!["npx vitest run"]);
        assert_eq!(related_tests(&r, d.path(), &s(&["a$b.ts"]), None), vec!["npx vitest run"]);
        assert_eq!(related_tests(&r, d.path(), &s(&["a\nb.ts"]), None), vec!["npx vitest run"]);
        assert_eq!(related_tests(&r, d.path(), &[], None), vec!["npx vitest run"]);
    }

    #[test]
    fn caminho_com_espaco_e_citado() {
        let d = proj(&[]);
        let got = related_tests(&Runner::Vitest, d.path(), &s(&["src/minha pasta/a.ts"]), None);
        assert_eq!(got, vec!["npx vitest related \"src/minha pasta/a.ts\" --run"]);
    }

    #[test]
    fn barra_invertida_vira_barra_normal() {
        let d = proj(&[]);
        let got = related_tests(&Runner::Vitest, d.path(), &s(&["src\\lib\\a.ts"]), None);
        assert_eq!(got, vec!["npx vitest related src/lib/a.ts --run"]);
    }

    #[test]
    fn template_do_projeto_substitui_o_padrao() {
        let d = proj(&[]);
        let t = json!({"vitest": "pnpm vitest related {arquivos} --run --silent", "cargo": "cargo nextest run {filtro}"});
        let m = t.as_object().unwrap();
        assert_eq!(
            related_tests(&Runner::Vitest, d.path(), &s(&["src/a.ts"]), Some(m)),
            vec!["pnpm vitest related src/a.ts --run --silent"]
        );
        let cargo = Runner::Cargo { manifest: "Cargo.toml".into() };
        assert_eq!(related_tests(&cargo, d.path(), &s(&["src/formato.rs"]), Some(m)), vec!["cargo nextest run formato"]);
    }

    #[test]
    fn cargo_com_crate_aninhado_usa_o_manifesto_certo() {
        let d = proj(&[]);
        let r = Runner::Cargo { manifest: "crates/a/Cargo.toml".into() };
        assert_eq!(
            related_tests(&r, d.path(), &s(&["crates/a/src/x.rs"]), None),
            vec!["cargo test --manifest-path crates/a/Cargo.toml -q x"]
        );
        // arquivo de outro crate não entra
        assert_eq!(
            related_tests(&r, d.path(), &s(&["src/x.rs"]), None),
            vec!["cargo test --manifest-path crates/a/Cargo.toml -q"]
        );
    }

    #[test]
    fn render_limpa_placeholder_vazio() {
        let u = Unidade::default();
        assert_eq!(render("cargo test --manifest-path {crate} -q {filtro}", &Runner::Cargo { manifest: "Cargo.toml".into() }, &u), "cargo test --manifest-path Cargo.toml -q");
    }

    #[test]
    fn caminho_ou_crate_com_metacaractere_cai_na_suite_completa() {
        let d = proj(&[("x&y/Cargo.toml", "[package]\nname = \"x\"\n"), ("x&y/src/a.rs", "")]);
        let cargo = Runner::Cargo { manifest: "x&y/Cargo.toml".into() };
        assert_eq!(related_tests(&cargo, d.path(), &s(&["x&y/src/a.rs"]), None), vec![full(&cargo)]);
        let d2 = proj(&[]);
        assert_eq!(related_tests(&Runner::Vitest, d2.path(), &s(&["a&calc.rs"]), None), vec![full(&Runner::Vitest)]);
        // vírgula passa em `seguro` mas não é um nome de teste seguro: a suíte inteira
        let got = related_tests(&Runner::Cargo { manifest: "Cargo.toml".into() }, d2.path(), &s(&["src/a,b.rs"]), None);
        assert_eq!(got, vec![full(&Runner::Cargo { manifest: "Cargo.toml".into() })]);
    }
}
