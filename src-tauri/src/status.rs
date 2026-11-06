use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const IDLE_AFTER: Duration = Duration::from_millis(1500);
pub const WAKE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneStatus {
    #[serde(rename = "iniciando")]
    Starting,
    #[serde(rename = "trabalhando")]
    Working,
    #[serde(rename = "ocioso")]
    Idle,
    #[serde(rename = "concluido")]
    Done,
    #[serde(rename = "erro")]
    Error,
    #[serde(rename = "fechando")]
    Closing,
    #[serde(rename = "hibernado")]
    Hibernated,
    #[serde(rename = "acordando")]
    Waking,
    #[serde(rename = "pergunta")]
    Asking,
}

impl PaneStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "iniciando",
            Self::Working => "trabalhando",
            Self::Idle => "ocioso",
            Self::Done => "concluido",
            Self::Error => "erro",
            Self::Closing => "fechando",
            Self::Hibernated => "hibernado",
            Self::Waking => "acordando",
            Self::Asking => "pergunta",
        }
    }
}

/// Status of a live process from its activity. `waking` = it was spawned by a wake.
pub fn activity_status(now: Instant, last_output: Option<Instant>, waking: bool) -> PaneStatus {
    match last_output {
        None if waking => PaneStatus::Waking,
        None => PaneStatus::Starting,
        Some(t) if now.duration_since(t) < IDLE_AFTER => PaneStatus::Working,
        Some(_) => PaneStatus::Idle,
    }
}

/// Final status when the process exits. Hibernate/close keep the status they already have.
pub fn exit_status(current: PaneStatus, code: u32) -> PaneStatus {
    match current {
        PaneStatus::Hibernated | PaneStatus::Closing => current,
        _ if code == 0 => PaneStatus::Done,
        _ => PaneStatus::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activity_rules() {
        let now = Instant::now();
        assert_eq!(activity_status(now, None, false), PaneStatus::Starting);
        assert_eq!(activity_status(now, None, true), PaneStatus::Waking);
        assert_eq!(activity_status(now, Some(now - Duration::from_millis(200)), false), PaneStatus::Working);
        assert_eq!(activity_status(now, Some(now - Duration::from_secs(3)), false), PaneStatus::Idle);
    }
    #[test]
    fn exit_rules() {
        assert_eq!(exit_status(PaneStatus::Working, 0), PaneStatus::Done);
        assert_eq!(exit_status(PaneStatus::Idle, 1), PaneStatus::Error);
        assert_eq!(exit_status(PaneStatus::Hibernated, 1), PaneStatus::Hibernated);
        assert_eq!(exit_status(PaneStatus::Closing, 0), PaneStatus::Closing);
    }
    #[test]
    fn serde_uses_portuguese() {
        assert_eq!(serde_json::to_string(&PaneStatus::Hibernated).unwrap(), "\"hibernado\"");
        assert_eq!(PaneStatus::Idle.as_str(), "ocioso");
    }
    #[test]
    fn serde_matches_as_str_for_all_variants() {
        use PaneStatus::*;
        for s in [Starting, Working, Idle, Done, Error, Closing, Hibernated, Waking, Asking] {
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
    }
}
