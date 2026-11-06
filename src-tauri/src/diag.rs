use crate::error::{fail, AppResult};
use crate::util::cmd;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub fn init_logging(dir: &Path) -> tracing_appender::non_blocking::WorkerGuard {
    let _ = std::fs::create_dir_all(dir);
    let built = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("regente.log")
        .max_log_files(14)
        .build(dir);
    // RollingFileAppender::new panics on the same failure, so fall back to a sink.
    let (nb, guard) = match built {
        Ok(a) => tracing_appender::non_blocking(a),
        Err(_) => tracing_appender::non_blocking(std::io::sink()),
    };
    // dependencies stay at warn; the app's own crate logs at info. Override with REGENTE_LOG.
    let filter = tracing_subscriber::EnvFilter::try_from_env("REGENTE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,regente_lib=info"));
    let _ = tracing_subscriber::fmt().with_writer(nb).with_ansi(false).with_env_filter(filter).try_init();
    guard
}

static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"(?i)bearer\s+[A-Za-z0-9._~+/=\-]+").unwrap(), "Bearer ***"),
        (Regex::new(r"sk-[A-Za-z0-9_\-]{8,}").unwrap(), "sk-***"),
        (Regex::new(r"\b(?:sk|gsk|r8|hf|ghp|gho|github_pat|xox[abprs])[-_][A-Za-z0-9_\-]{8,}").unwrap(), "***"),
        (Regex::new(r"AIza[0-9A-Za-z_\-]{30,}").unwrap(), "***"),
        (Regex::new(r"eyJ[\w-]+\.[\w-]+\.[\w-]+").unwrap(), "***"),
        (Regex::new(r#"(?i)("?(?:api[_-]?key|auth[_-]?token|token|secret|password|authorization|key)"?\s*[:=]\s*)("[^"]*"|[^\s,}&]+)"#).unwrap(), "${1}***"),
        (Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap(), "***@***"),
    ]
});

pub fn redact(text: &str) -> String {
    RULES.iter().fold(text.to_string(), |acc, (re, rep)| re.replace_all(&acc, *rep).to_string())
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Zips logs + configs (redacted) into `out_dir`. Returns the zip path.
pub fn make_bundle(data_dir: &Path, out_dir: &Path, version: &str) -> AppResult<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let staging = Staging(std::env::temp_dir().join(format!("regente-diag-{stamp}")));
    std::fs::create_dir_all(&staging.0)?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(data_dir.join("logs")).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.extend(["providers.json", "settings.json", "prices.json"].iter().map(|f| data_dir.join(f)));
    let mut names = vec!["versao.txt".to_string()];
    for f in files.into_iter().filter(|f| f.is_file()) {
        let text = String::from_utf8_lossy(&std::fs::read(&f)?).to_string();
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        std::fs::write(staging.0.join(&name), redact(&text))?;
        names.push(name);
    }
    std::fs::write(staging.0.join("versao.txt"), format!("Regente {version}\n{}\n", std::env::consts::OS))?;
    std::fs::create_dir_all(out_dir)?;
    let zip = out_dir.join(format!("regente-diagnostico-{stamp}.zip"));
    let ok = cmd("tar.exe").args(["-a", "-cf"]).arg(&zip).arg("-C").arg(&staging.0).args(&names).status()?.success();
    if !ok || !zip.exists() {
        return fail("diag_failed", "falha ao compactar o diagnóstico");
    }
    Ok(zip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_keys_tokens_emails() {
        let s = redact(r#"key sk-ant-api03-abcdefgh "authToken": "xyz123" password=hunter2 mail fulano@empresa.com.br"#);
        assert!(!s.contains("abcdefgh") && !s.contains("xyz123") && !s.contains("hunter2") && !s.contains("fulano"), "{s}");
        assert!(s.contains("sk-***") && s.contains("***@***"));
    }

    #[test]
    fn redacts_more_secret_shapes() {
        let cases = [
            ("Authorization: Bearer abc.DEF-123xyz", "abc.DEF-123"),
            ("gsk_abcdefgh12345", "abcdefgh12345"),
            ("ghp_abcdefgh12345", "abcdefgh12345"),
            ("AIzaSyA1234567890abcdefghijklmnopqrstu", "A1234567890abcdef"),
            ("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcDEF123", "eyJzdWIi"),
            ("key=valor1", "valor1"),
            (r#"{"password": "com espaço secreto"}"#, "espaço"),
        ];
        for (input, secret) in cases {
            assert!(!redact(input).contains(secret), "{input} -> {}", redact(input));
        }
        for input in [
            r#""ANTHROPIC_AUTH_TOKEN": "zzsecret""#,
            "ANTHROPIC_API_KEY=zzsecret",
            "OPENROUTER_API_KEY=zzsecret",
            "XI_API_KEY=zzsecret",
            "access_token=zzsecret",
            r#""accessToken": "zzsecret""#,
            r#""client_secret": "zzsecret""#,
            r#""refresh_token": "zzsecret""#,
        ] {
            assert!(!redact(input).contains("zzsecret"), "{input} -> {}", redact(input));
        }
        assert_eq!(redact("keyboard: x"), "keyboard: x");
        assert_eq!(redact("tokens: 5"), "tokens: 5");
    }

    #[test]
    fn bundle_contains_redacted_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("logs")).unwrap();
        std::fs::write(d.path().join("logs").join("regente.log.2026-10-14"), "token=segredo42").unwrap();
        std::fs::write(d.path().join("settings.json"), "{}").unwrap();
        let zip = make_bundle(d.path(), &d.path().join("out"), "0.1.0").unwrap();
        let x = d.path().join("x");
        std::fs::create_dir_all(&x).unwrap();
        assert!(cmd("tar.exe").arg("-xf").arg(&zip).arg("-C").arg(&x).status().unwrap().success());
        let log = std::fs::read_to_string(x.join("regente.log.2026-10-14")).unwrap();
        assert!(!log.contains("segredo42"));
        assert!(x.join("versao.txt").exists() && x.join("settings.json").exists());
    }
}
