//! Minimal leveled logging to stderr. Callers pass only values that are safe
//! to print; tokens never reach this module.

use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub fn raised_by(self, steps: u8) -> Level {
        let all = [Level::Error, Level::Warn, Level::Info, Level::Debug];
        let idx = (self as usize).saturating_add(steps as usize);
        all[idx.min(all.len() - 1)]
    }
}

impl FromStr for Level {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Ok(Level::Error),
            "warn" | "warning" => Ok(Level::Warn),
            "info" => Ok(Level::Info),
            "debug" | "trace" => Ok(Level::Debug),
            other => Err(format!(
                "unknown log level {other:?}; use error, warn, info or debug"
            )),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Logger {
    level: Level,
}

impl Logger {
    pub fn new(level: Level) -> Self {
        Self { level }
    }

    pub fn info(&self, args: fmt::Arguments<'_>) {
        self.emit(Level::Info, "info", args);
    }

    pub fn debug(&self, args: fmt::Arguments<'_>) {
        self.emit(Level::Debug, "debug", args);
    }

    fn emit(&self, level: Level, label: &str, args: fmt::Arguments<'_>) {
        if level <= self.level {
            eprintln!("ak-kubelet-provider: {label}: {args}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raising_saturates_at_debug() {
        assert_eq!(Level::Warn.raised_by(1), Level::Info);
        assert_eq!(Level::Warn.raised_by(9), Level::Debug);
        assert_eq!(Level::Error.raised_by(0), Level::Error);
    }

    #[test]
    fn parses_levels() {
        assert_eq!("WARN".parse::<Level>().unwrap(), Level::Warn);
        assert!("loud".parse::<Level>().is_err());
    }
}
