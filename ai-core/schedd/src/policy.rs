//! What priority each kind of process gets.
//!
//! Two kernel knobs are used, both per thread and both reversible:
//!
//! - the **nice value** (-20 to 19), which changes how large a share of a busy CPU a
//!   thread gets: each step is about 25% more or less. Raising it (a nicer, slower
//!   process) is always allowed; lowering it needs privilege, so the daemon runs as root.
//! - the **scheduling policy**: `batch` tells the kernel the thread is throughput-oriented
//!   (it won't preempt others eagerly on wake-up), `idle` makes it run only when nothing
//!   else wants the CPU.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::Class;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sched {
    Normal,
    Batch,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub nice: i32,
    pub sched: Sched,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    settings: HashMap<Class, Setting>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            settings: HashMap::from([
                (Class::Interactive, Setting { nice: -5, sched: Sched::Normal }),
                (Class::Media, Setting { nice: -3, sched: Sched::Normal }),
                (Class::Batch, Setting { nice: 10, sched: Sched::Batch }),
                (Class::Background, Setting { nice: 19, sched: Sched::Idle }),
            ]),
        }
    }
}

impl Policy {
    pub fn setting(&self, class: Class) -> Setting {
        self.settings[&class]
    }

    /// The defaults with the entries of a JSON object applied on top, for example
    /// `{"batch": {"nice": 5, "sched": "batch"}}`. Nice values must be within -20 to 19.
    pub fn from_json(text: &str) -> Result<Policy, String> {
        let given: HashMap<Class, Setting> = serde_json::from_str(text).map_err(|e| format!("the policy file is not valid: {e}"))?;
        let mut policy = Policy::default();
        for (class, setting) in given {
            if !(-20..=19).contains(&setting.nice) {
                return Err(format!("{class}: nice must be from -20 to 19, not {}", setting.nice));
            }
            policy.settings.insert(class, setting);
        }
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_class_has_a_setting_and_the_order_makes_sense() {
        let p = Policy::default();
        for class in Class::ALL {
            let s = p.setting(class);
            assert!((-20..=19).contains(&s.nice), "{class}: {}", s.nice);
        }
        assert!(p.setting(Class::Interactive).nice < 0);
        assert!(p.setting(Class::Media).nice < 0);
        assert!(p.setting(Class::Batch).nice > 0);
        assert_eq!(p.setting(Class::Batch).sched, Sched::Batch);
        assert_eq!(p.setting(Class::Background).sched, Sched::Idle);
        // the favoured kinds must out-rank the neglected ones
        assert!(p.setting(Class::Media).nice < p.setting(Class::Batch).nice);
    }

    #[test]
    fn a_file_overrides_only_what_it_names() {
        let p = Policy::from_json(r#"{"batch": {"nice": 5, "sched": "normal"}}"#).unwrap();
        assert_eq!(p.setting(Class::Batch), Setting { nice: 5, sched: Sched::Normal });
        assert_eq!(p.setting(Class::Interactive), Policy::default().setting(Class::Interactive));
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(Policy::from_json(r#"{"batch": {"nice": 40, "sched": "batch"}}"#).unwrap_err().contains("nice must be"));
        assert!(Policy::from_json(r#"{"batch": {"nice": 1, "sched": "fifo"}}"#).is_err());
        assert!(Policy::from_json(r#"{"gaming": {"nice": 1, "sched": "normal"}}"#).is_err());
        assert!(Policy::from_json("[]").is_err());
    }
}
