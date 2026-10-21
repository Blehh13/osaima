//! Tunable limits and timeouts, read from `OSAIMA_*` environment variables.
//!
//! Every value has a default and a validated range; an invalid value stops the
//! daemon at startup with a message naming the variable. The list, with
//! explanations, is in `docs/configuration.md`.
//!
//! Safety rules are deliberately *not* settings: PID 0, PID 1 and the daemon
//! itself can never be signalled, the signal and power-action lists are fixed,
//! and file search never leaves the home directory.

use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// How often CPU, memory and process data are re-sampled.
    pub sample_interval: Duration,
    /// Longest accepted JSON-RPC message; longer ones close the connection.
    pub max_message_bytes: usize,
    /// Time limit for helper programs and IPC (`wpctl`, `loginctl`, sway).
    pub helper_timeout: Duration,
    /// Time limit for each step of `check_connectivity`.
    pub connect_timeout: Duration,
    /// Host `check_connectivity` tests when none is given.
    pub connectivity_host: String,
    /// Process count `list_processes` returns when no limit is given.
    pub process_list_default: u64,
    /// Largest process count `list_processes` accepts.
    pub process_list_max: u64,
    /// How many directory levels `search_files` descends.
    pub search_max_depth: usize,
    /// How many directory entries `search_files` examines at most.
    pub search_max_entries: usize,
    /// Time budget for one `search_files` call.
    pub search_time_budget: Duration,
    /// Highest volume `set_volume` accepts, in percent.
    pub max_volume_percent: u64,
    /// Terminal used for desktop entries marked `Terminal=true`.
    pub terminal: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sample_interval: Duration::from_millis(2000),
            max_message_bytes: 1024 * 1024,
            helper_timeout: Duration::from_millis(5000),
            connect_timeout: Duration::from_millis(4000),
            connectivity_host: "example.com".into(),
            process_list_default: 15,
            process_list_max: 200,
            search_max_depth: 8,
            search_max_entries: 100_000,
            search_time_budget: Duration::from_millis(3000),
            max_volume_percent: 150,
            terminal: "foot".into(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct SettingsError {
    pub variable: &'static str,
    pub problem: String,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.variable, self.problem)
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    /// Read the process environment.
    pub fn from_env() -> Result<Self, SettingsError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Read settings through `get` (tests pass a map instead of the environment).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, SettingsError> {
        let d = Self::default();
        let settings = Self {
            sample_interval: millis(
                &get,
                "OSAIMA_SAMPLE_INTERVAL_MS",
                d.sample_interval,
                250,
                60_000,
            )?,
            max_message_bytes: number(
                &get,
                "OSAIMA_MAX_MESSAGE_BYTES",
                d.max_message_bytes as u64,
                1024,
                64 * 1024 * 1024,
            )? as usize,
            helper_timeout: millis(
                &get,
                "OSAIMA_HELPER_TIMEOUT_MS",
                d.helper_timeout,
                500,
                120_000,
            )?,
            connect_timeout: millis(
                &get,
                "OSAIMA_CONNECT_TIMEOUT_MS",
                d.connect_timeout,
                200,
                60_000,
            )?,
            connectivity_host: text(&get, "OSAIMA_CONNECTIVITY_HOST", &d.connectivity_host)?,
            process_list_default: number(
                &get,
                "OSAIMA_PROCESS_LIST_DEFAULT",
                d.process_list_default,
                1,
                1000,
            )?,
            process_list_max: number(&get, "OSAIMA_PROCESS_LIST_MAX", d.process_list_max, 1, 1000)?,
            search_max_depth: number(
                &get,
                "OSAIMA_SEARCH_MAX_DEPTH",
                d.search_max_depth as u64,
                1,
                64,
            )? as usize,
            search_max_entries: number(
                &get,
                "OSAIMA_SEARCH_MAX_ENTRIES",
                d.search_max_entries as u64,
                1000,
                10_000_000,
            )? as usize,
            search_time_budget: millis(
                &get,
                "OSAIMA_SEARCH_TIME_BUDGET_MS",
                d.search_time_budget,
                100,
                60_000,
            )?,
            max_volume_percent: number(
                &get,
                "OSAIMA_MAX_VOLUME_PERCENT",
                d.max_volume_percent,
                100,
                200,
            )?,
            terminal: terminal(&get, &d.terminal)?,
        };
        if settings.process_list_default > settings.process_list_max {
            return Err(SettingsError {
                variable: "OSAIMA_PROCESS_LIST_DEFAULT",
                problem: format!(
                    "{} is larger than OSAIMA_PROCESS_LIST_MAX ({})",
                    settings.process_list_default, settings.process_list_max
                ),
            });
        }
        Ok(settings)
    }
}

fn number(
    get: &impl Fn(&str) -> Option<String>,
    variable: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, SettingsError> {
    let Some(raw) = get(variable).filter(|v| !v.trim().is_empty()) else {
        return Ok(default);
    };
    let value: u64 = raw.trim().parse().map_err(|_| SettingsError {
        variable,
        problem: format!("{raw:?} is not a whole number"),
    })?;
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(SettingsError {
            variable,
            problem: format!("{value} is outside the allowed range {min} to {max}"),
        })
    }
}

fn millis(
    get: &impl Fn(&str) -> Option<String>,
    variable: &'static str,
    default: Duration,
    min: u64,
    max: u64,
) -> Result<Duration, SettingsError> {
    number(get, variable, default.as_millis() as u64, min, max).map(Duration::from_millis)
}

fn text(
    get: &impl Fn(&str) -> Option<String>,
    variable: &'static str,
    default: &str,
) -> Result<String, SettingsError> {
    let value = get(variable)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string());
    // Host names and addresses only: this value is passed to name resolution.
    let valid = value.len() <= 253
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    if valid {
        Ok(value)
    } else {
        Err(SettingsError {
            variable,
            problem: format!("{value:?} is not a valid host name or address"),
        })
    }
}

/// `OSAIMA_TERMINAL`, else the conventional `TERMINAL`, else the default.
fn terminal(get: &impl Fn(&str) -> Option<String>, default: &str) -> Result<String, SettingsError> {
    let value = ["OSAIMA_TERMINAL", "TERMINAL"]
        .into_iter()
        .find_map(|name| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| default.to_string());
    if value.chars().any(char::is_whitespace) {
        return Err(SettingsError {
            variable: "OSAIMA_TERMINAL",
            problem: format!("{value:?} must be a single program name or path"),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(pairs: &[(&str, &str)]) -> Result<Settings, SettingsError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Settings::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn defaults_apply_when_nothing_is_set() {
        assert_eq!(settings(&[]).unwrap(), Settings::default());
        // Empty values count as unset.
        assert_eq!(
            settings(&[("OSAIMA_SAMPLE_INTERVAL_MS", "  ")]).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn every_variable_is_honoured() {
        let s = settings(&[
            ("OSAIMA_SAMPLE_INTERVAL_MS", "500"),
            ("OSAIMA_MAX_MESSAGE_BYTES", "4096"),
            ("OSAIMA_HELPER_TIMEOUT_MS", "9000"),
            ("OSAIMA_CONNECT_TIMEOUT_MS", "1500"),
            ("OSAIMA_CONNECTIVITY_HOST", "gentoo.org"),
            ("OSAIMA_PROCESS_LIST_DEFAULT", "5"),
            ("OSAIMA_PROCESS_LIST_MAX", "50"),
            ("OSAIMA_SEARCH_MAX_DEPTH", "3"),
            ("OSAIMA_SEARCH_MAX_ENTRIES", "5000"),
            ("OSAIMA_SEARCH_TIME_BUDGET_MS", "800"),
            ("OSAIMA_MAX_VOLUME_PERCENT", "100"),
            ("OSAIMA_TERMINAL", "alacritty"),
        ])
        .unwrap();
        assert_eq!(s.sample_interval, Duration::from_millis(500));
        assert_eq!(s.max_message_bytes, 4096);
        assert_eq!(s.helper_timeout, Duration::from_secs(9));
        assert_eq!(s.connect_timeout, Duration::from_millis(1500));
        assert_eq!(s.connectivity_host, "gentoo.org");
        assert_eq!((s.process_list_default, s.process_list_max), (5, 50));
        assert_eq!((s.search_max_depth, s.search_max_entries), (3, 5000));
        assert_eq!(s.search_time_budget, Duration::from_millis(800));
        assert_eq!(s.max_volume_percent, 100);
        assert_eq!(s.terminal, "alacritty");
    }

    #[test]
    fn bad_values_name_the_variable() {
        let cases = [
            ("OSAIMA_SAMPLE_INTERVAL_MS", "fast", "not a whole number"),
            (
                "OSAIMA_SAMPLE_INTERVAL_MS",
                "10",
                "outside the allowed range",
            ),
            ("OSAIMA_MAX_MESSAGE_BYTES", "-1", "not a whole number"),
            (
                "OSAIMA_MAX_VOLUME_PERCENT",
                "500",
                "outside the allowed range",
            ),
            ("OSAIMA_SEARCH_MAX_DEPTH", "0", "outside the allowed range"),
            ("OSAIMA_CONNECTIVITY_HOST", "a b;c", "not a valid host"),
            ("OSAIMA_CONNECTIVITY_HOST", "-oProxy", "not a valid host"),
            ("OSAIMA_TERMINAL", "foot -e", "single program"),
        ];
        for (name, value, problem) in cases {
            let err = settings(&[(name, value)]).unwrap_err();
            assert_eq!(err.variable, name, "{name}={value}");
            assert!(err.problem.contains(problem), "{err}");
        }
    }

    #[test]
    fn default_process_count_cannot_exceed_the_maximum() {
        let err = settings(&[("OSAIMA_PROCESS_LIST_MAX", "10")]).unwrap_err();
        assert_eq!(err.variable, "OSAIMA_PROCESS_LIST_DEFAULT");
        assert!(settings(&[("OSAIMA_PROCESS_LIST_MAX", "20")]).is_ok());
    }

    #[test]
    fn terminal_falls_back_to_the_conventional_variable() {
        assert_eq!(
            settings(&[("TERMINAL", "xterm")]).unwrap().terminal,
            "xterm"
        );
        let both = settings(&[("TERMINAL", "xterm"), ("OSAIMA_TERMINAL", "kitty")]).unwrap();
        assert_eq!(both.terminal, "kitty");
    }
}
