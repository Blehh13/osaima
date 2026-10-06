//! Shell backend limits and timeouts, read once from `OSAIMA_*` environment
//! variables. Every value has a default and a validated range; an invalid value
//! stops the shell at startup with a message naming the variable.

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Terminal app commands are killed (with everything they started) after this long.
    pub command_timeout: Duration,
    /// Terminal app output beyond this many bytes is dropped.
    pub command_output_limit: usize,
    /// How long to wait for the AI Core to answer one request.
    pub core_timeout: Duration,
    /// How long to wait for the assistant service to accept one request.
    pub agent_timeout: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            command_timeout: Duration::from_secs(30),
            command_output_limit: 256 * 1024,
            core_timeout: Duration::from_millis(3000),
            agent_timeout: Duration::from_millis(15_000),
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

impl Settings {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, SettingsError> {
        let d = Self::default();
        Ok(Self {
            command_timeout: Duration::from_secs(number(
                &get,
                "OSAIMA_SHELL_COMMAND_TIMEOUT_S",
                d.command_timeout.as_secs(),
                1,
                3600,
            )?),
            command_output_limit: number(
                &get,
                "OSAIMA_SHELL_OUTPUT_LIMIT_BYTES",
                d.command_output_limit as u64,
                1024,
                64 * 1024 * 1024,
            )? as usize,
            core_timeout: Duration::from_millis(number(
                &get,
                "OSAIMA_CORE_TIMEOUT_MS",
                d.core_timeout.as_millis() as u64,
                200,
                120_000,
            )?),
            agent_timeout: Duration::from_millis(number(
                &get,
                "OSAIMA_AGENT_TIMEOUT_MS",
                d.agent_timeout.as_millis() as u64,
                500,
                300_000,
            )?),
        })
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

static ACTIVE: OnceLock<Settings> = OnceLock::new();

/// Read the environment and make the result available through [`get`].
/// Call once at startup; an invalid value is an error, not a silent default.
pub fn init() -> Result<&'static Settings, SettingsError> {
    let settings = Settings::from_lookup(|name| std::env::var(name).ok())?;
    Ok(ACTIVE.get_or_init(|| settings))
}

/// The active settings (defaults if [`init`] was never called, as in unit tests).
pub fn get() -> &'static Settings {
    ACTIVE.get_or_init(Settings::default)
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
    }

    #[test]
    fn variables_are_honoured() {
        let s = settings(&[
            ("OSAIMA_SHELL_COMMAND_TIMEOUT_S", "90"),
            ("OSAIMA_SHELL_OUTPUT_LIMIT_BYTES", "4096"),
            ("OSAIMA_CORE_TIMEOUT_MS", "1000"),
            ("OSAIMA_AGENT_TIMEOUT_MS", "20000"),
        ])
        .unwrap();
        assert_eq!(s.command_timeout, Duration::from_secs(90));
        assert_eq!(s.command_output_limit, 4096);
        assert_eq!(s.core_timeout, Duration::from_secs(1));
        assert_eq!(s.agent_timeout, Duration::from_secs(20));
    }

    #[test]
    fn bad_values_name_the_variable() {
        for (name, value) in [
            ("OSAIMA_SHELL_COMMAND_TIMEOUT_S", "0"),
            ("OSAIMA_SHELL_OUTPUT_LIMIT_BYTES", "ten"),
            ("OSAIMA_CORE_TIMEOUT_MS", "5"),
            ("OSAIMA_AGENT_TIMEOUT_MS", "-3"),
        ] {
            assert_eq!(settings(&[(name, value)]).unwrap_err().variable, name);
        }
    }
}
