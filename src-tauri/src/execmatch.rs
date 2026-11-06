//! Matcher de comandos longos (E1.4, parte pura). Diz se um comando de Bash/PowerShell é servidor de
//! desenvolvimento, teste ou watch, e portanto deve virar painel de execução. Tolera `cd x &&`, prefixos de
//! ambiente (`FOO=1 cmd`) e `npx`. Cada segmento de uma cadeia (`&&`, `;`, `||`, `|`) é avaliado.
use std::sync::LazyLock;

use regex::Regex;

static PREFIXO_AMBIENTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*=\S*$").unwrap());

pub fn is_long_running(cmd: &str) -> bool {
    cmd.split(|c: char| c == ';' || c == '|' || c == '&')
        .filter(|s| !s.trim().is_empty())
        .any(|seg| segmento_longo(seg))
}

fn segmento_longo(seg: &str) -> bool {
    let mut t: Vec<String> = seg.split_whitespace().map(|x| x.to_ascii_lowercase()).collect();
    while t.first().is_some_and(|x| PREFIXO_AMBIENTE.is_match(x)) {
        t.remove(0);
    }
    if t.first().is_some_and(|x| x == "npx" || x == "pnpx") {
        t.remove(0);
        while t.first().is_some_and(|x| x.starts_with('-')) {
            t.remove(0); // flags do npx, como `-y`
        }
    }
    let Some(prog) = t.first().map(String::as_str) else { return false };
    let arg = |i: usize| t.get(i).map(String::as_str).unwrap_or("");
    match prog {
        "npm" | "pnpm" | "yarn" => {
            let alvo = if arg(1) == "run" { arg(2) } else { arg(1) };
            matches!(alvo, "dev" | "start" | "test")
        }
        "vite" => true,
        "next" => arg(1) == "dev",
        "cargo" => matches!(arg(1), "test" | "run" | "watch"),
        "pytest" | "jest" | "vitest" => true,
        "docker" => arg(1) == "compose" && arg(2) == "up",
        "docker-compose" => arg(1) == "up",
        "python" | "python3" | "py" => t.windows(2).any(|w| w[0] == "-m" && w[1] == "http.server"),
        "go" => arg(1) == "test",
        "dotnet" => matches!(arg(1), "run" | "test" | "watch"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_long_running;

    #[test]
    fn comandos_longos_sao_reconhecidos() {
        let casos = [
            "npm run dev",
            "npm start",
            "npm test",
            "npm run test",
            "pnpm run dev",
            "pnpm test",
            "yarn dev",
            "yarn run start",
            "vite",
            "npx vite --port 5173",
            "next dev",
            "cargo test -q",
            "cargo run",
            "cargo watch -x check",
            "pytest -q",
            "jest --watch",
            "vitest",
            "npx vitest run",
            "docker compose up -d",
            "docker-compose up",
            "python -m http.server 8000",
            "python3 -m http.server",
            "go test ./...",
            "dotnet run",
            "dotnet test",
            "dotnet watch",
        ];
        for c in casos {
            assert!(is_long_running(c), "deveria ser longo: {c}");
        }
    }

    #[test]
    fn tolera_cd_ambiente_e_npx() {
        assert!(is_long_running("cd src-tauri && cargo test"));
        assert!(is_long_running("cd \"C:/meu projeto\" && npm run dev"));
        assert!(is_long_running("NODE_ENV=test npm test"));
        assert!(is_long_running("RUST_LOG=debug FOO=1 cargo run"));
        assert!(is_long_running("NODE_ENV=test npx vitest"));
        assert!(is_long_running("CI=1 npx -y jest"));
        assert!(is_long_running("echo ok; npm start"));
        assert!(is_long_running("NPM RUN DEV"), "case-insensitive");
    }

    #[test]
    fn comandos_curtos_nao_sao_longos() {
        let casos = [
            "",
            "   ",
            "ls -la",
            "git status",
            "npm install",
            "npm run build",
            "npm ci",
            "cargo build",
            "cargo check",
            "cargo fmt",
            "cd src && ls",
            "echo cargo test",
            "node server.js",
            "python script.py",
            "python -m pip install x",
            "docker compose down",
            "dotnet build",
            "go build ./...",
            "npmrun dev",
            "npm run devx",
            "rm -rf dist",
        ];
        for c in casos {
            assert!(!is_long_running(c), "não deveria ser longo: {c}");
        }
    }

    #[test]
    fn cadeia_com_um_segmento_longo_conta() {
        assert!(is_long_running("npm run build && npm run dev"));
        assert!(!is_long_running("npm run build && npm run lint"));
    }
}
