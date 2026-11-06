use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn data_dir_from(regente_home: Option<String>, appdata: Option<String>) -> PathBuf {
    if let Some(h) = regente_home.filter(|s| !s.trim().is_empty()) {
        return PathBuf::from(h);
    }
    PathBuf::from(appdata.unwrap_or_else(|| ".".into())).join("Regente")
}

/// `%APPDATA%\Regente` (or `REGENTE_HOME`). Creates the folder.
pub fn data_dir() -> PathBuf {
    let d = data_dir_from(std::env::var("REGENTE_HOME").ok(), std::env::var("APPDATA").ok());
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

/// External process with no console window flashing on screen.
pub fn cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Lock tolerante a envenenamento: o estado protegido continua utilizável após um panic em outra thread.
pub fn lk<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regente_home_wins() {
        assert_eq!(data_dir_from(Some("D:\\x".into()), Some("C:\\a".into())), PathBuf::from("D:\\x"));
    }
    #[test]
    fn falls_back_to_appdata() {
        assert_eq!(data_dir_from(None, Some("C:\\a".into())), PathBuf::from("C:\\a").join("Regente"));
        assert_eq!(data_dir_from(Some("  ".into()), Some("C:\\a".into())), PathBuf::from("C:\\a").join("Regente"));
    }
    #[test]
    fn today_is_iso_date() {
        assert_eq!(today().len(), 10);
        assert_eq!(&today()[4..5], "-");
    }
}
