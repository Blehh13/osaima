//! Setting a process's CPU priority, and putting it back.
//!
//! The kernel applies nice values and scheduling policies per thread, so every thread of
//! the process is set. Threads a process starts later inherit their creator's setting. The
//! original values are remembered so they can be restored when the daemon stops.

use std::fs;
use std::io;

use crate::policy::{Sched, Setting};

/// What a thread had before the daemon changed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Original {
    pub tid: i32,
    pub nice: i32,
    /// A `SCHED_*` constant.
    pub policy: i32,
}

pub trait Actuator {
    /// Remember the current settings of every thread of `pid`.
    fn read(&mut self, pid: i32) -> io::Result<Vec<Original>>;
    fn apply(&mut self, pid: i32, setting: Setting) -> io::Result<()>;
    /// Put the remembered settings back. Threads that no longer exist are skipped.
    fn restore(&mut self, originals: &[Original]) -> io::Result<()>;
}

/// Talks to the kernel.
#[derive(Default)]
pub struct KernelActuator;

fn threads(pid: i32) -> io::Result<Vec<i32>> {
    let mut tids = Vec::new();
    for entry in fs::read_dir(format!("/proc/{pid}/task"))? {
        if let Some(tid) = entry?.file_name().to_str().and_then(|n| n.parse().ok()) {
            tids.push(tid);
        }
    }
    Ok(tids)
}

fn get_nice(tid: i32) -> io::Result<i32> {
    // getpriority can legitimately return -1, so errno is what says whether it failed
    unsafe {
        *libc::__errno_location() = 0;
        let value = libc::getpriority(libc::PRIO_PROCESS, tid as libc::id_t);
        if value == -1 && *libc::__errno_location() != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }
}

fn set_nice(tid: i32, nice: i32) -> io::Result<()> {
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, nice) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn get_policy(tid: i32) -> io::Result<i32> {
    let value = unsafe { libc::sched_getscheduler(tid) };
    if value < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

fn set_policy(tid: i32, policy: i32) -> io::Result<()> {
    // none of the policies used here has a real-time priority
    let param = libc::sched_param { sched_priority: 0 };
    if unsafe { libc::sched_setscheduler(tid, policy, &param) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn policy_of(sched: Sched) -> i32 {
    match sched {
        Sched::Normal => libc::SCHED_OTHER,
        Sched::Batch => libc::SCHED_BATCH,
        Sched::Idle => libc::SCHED_IDLE,
    }
}

/// Run `f` on every thread, tolerating threads that exit meanwhile; the first real error is
/// returned after all threads were tried.
fn for_each_thread(tids: &[i32], mut f: impl FnMut(i32) -> io::Result<()>) -> io::Result<()> {
    let mut first_error = None;
    for &tid in tids {
        match f(tid) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {} // the thread is gone
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

impl Actuator for KernelActuator {
    fn read(&mut self, pid: i32) -> io::Result<Vec<Original>> {
        let mut originals = Vec::new();
        for tid in threads(pid)? {
            // a thread that exits while being read is simply left out
            if let (Ok(nice), Ok(policy)) = (get_nice(tid), get_policy(tid)) {
                originals.push(Original { tid, nice, policy });
            }
        }
        Ok(originals)
    }

    fn apply(&mut self, pid: i32, setting: Setting) -> io::Result<()> {
        let policy = policy_of(setting.sched);
        for_each_thread(&threads(pid)?, |tid| {
            // the policy first: leaving SCHED_IDLE would otherwise ignore the nice value
            set_policy(tid, policy)?;
            if setting.sched != Sched::Idle {
                set_nice(tid, setting.nice)?;
            }
            Ok(())
        })
    }

    fn restore(&mut self, originals: &[Original]) -> io::Result<()> {
        let tids: Vec<i32> = originals.iter().map(|o| o.tid).collect();
        for_each_thread(&tids, |tid| {
            let original = originals
                .iter()
                .find(|o| o.tid == tid)
                .expect("tid came from the list");
            set_policy(tid, original.policy)?;
            set_nice(tid, original.nice)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    struct Sleeper(Child);
    impl Sleeper {
        fn new() -> Self {
            Sleeper(
                Command::new("sleep")
                    .arg("60")
                    .spawn()
                    .expect("the sleep command is needed for this test"),
            )
        }
        fn pid(&self) -> i32 {
            self.0.id() as i32
        }
    }
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn the_original_settings_are_read() {
        let child = Sleeper::new();
        let originals = KernelActuator.read(child.pid()).unwrap();
        assert_eq!(originals.len(), 1);
        assert_eq!(originals[0].tid, child.pid());
        assert_eq!(originals[0].policy, libc::SCHED_OTHER);
    }

    /// Making a process nicer is allowed without privilege, so this works for any user.
    #[test]
    fn a_process_can_be_made_nicer_and_the_change_is_visible() {
        let child = Sleeper::new();
        let before = get_nice(child.pid()).unwrap();
        let target = (before + 5).min(19);
        KernelActuator
            .apply(
                child.pid(),
                Setting {
                    nice: target,
                    sched: Sched::Normal,
                },
            )
            .unwrap();
        assert_eq!(get_nice(child.pid()).unwrap(), target);
    }

    #[test]
    fn batch_and_idle_policies_can_be_set_without_privilege() {
        let child = Sleeper::new();
        KernelActuator
            .apply(
                child.pid(),
                Setting {
                    nice: 10,
                    sched: Sched::Batch,
                },
            )
            .unwrap();
        assert_eq!(get_policy(child.pid()).unwrap(), libc::SCHED_BATCH);
        KernelActuator
            .apply(
                child.pid(),
                Setting {
                    nice: 19,
                    sched: Sched::Idle,
                },
            )
            .unwrap();
        assert_eq!(get_policy(child.pid()).unwrap(), libc::SCHED_IDLE);
    }

    #[test]
    fn a_process_that_is_gone_is_an_error_not_a_crash() {
        let mut child = Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert!(KernelActuator
            .apply(
                pid,
                Setting {
                    nice: 5,
                    sched: Sched::Normal
                }
            )
            .is_err());
        assert!(KernelActuator.read(pid).is_err());
    }

    #[test]
    fn restoring_a_thread_that_exited_is_fine() {
        let ghost = Original {
            tid: i32::MAX,
            nice: 0,
            policy: libc::SCHED_OTHER,
        };
        assert!(KernelActuator.restore(&[ghost]).is_ok());
    }
}
