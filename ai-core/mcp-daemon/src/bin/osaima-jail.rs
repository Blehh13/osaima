//! osaima-jail — kernel-enforced limits for one AI-run command.
//!
//!   osaima-jail [--write DIR]... [--cpu SECONDS] -- PROGRAM [ARGS]...
//!
//! Applied in this process, then inherited by PROGRAM and everything it starts,
//! and impossible to lift from inside:
//!   - no_new_privs: setuid binaries (sudo, su, pkexec) cannot gain privileges
//!   - Landlock LSM: the whole filesystem is read/execute-only, except the
//!     --write directories (plus /tmp and /dev, which the sandbox makes private);
//!     on Linux 6.12+ also no abstract unix sockets or signals outside it
//!   - seccomp-bpf: syscalls an assistant never needs fail with EPERM (kernel
//!     modules, mount, ptrace, kexec, bpf, reboot, keyrings, namespaces, ...)
//!   - rlimits: CPU time, largest file written, open files, no core dumps
//!
//! The first line on stderr reports what the kernel actually enforced, e.g.
//! "osaima-jail: landlock=full seccomp=on", so the daemon never overstates it.

use landlock::{
    path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope, ABI,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;

/// Syscalls no assistant task needs; each would let a fooled model reach past
/// the sandbox or into the kernel.
const DENIED: &[i64] = &[
    libc::SYS_init_module, libc::SYS_finit_module, libc::SYS_delete_module,
    libc::SYS_kexec_load, libc::SYS_kexec_file_load, libc::SYS_reboot,
    libc::SYS_mount, libc::SYS_umount2, libc::SYS_pivot_root, libc::SYS_chroot,
    libc::SYS_swapon, libc::SYS_swapoff, libc::SYS_quotactl, libc::SYS_acct,
    libc::SYS_ptrace, libc::SYS_process_vm_readv, libc::SYS_process_vm_writev,
    libc::SYS_bpf, libc::SYS_perf_event_open, libc::SYS_userfaultfd,
    libc::SYS_keyctl, libc::SYS_add_key, libc::SYS_request_key,
    libc::SYS_setns, libc::SYS_unshare, libc::SYS_open_by_handle_at,
    libc::SYS_iopl, libc::SYS_ioperm, libc::SYS_syslog,
    libc::SYS_settimeofday, libc::SYS_clock_settime, libc::SYS_adjtimex,
];

fn landlock(write: &[String]) -> &'static str {
    let abi = ABI::V3;
    let mut writable: Vec<&str> = vec!["/tmp", "/dev"];
    writable.extend(write.iter().map(String::as_str));
    let res = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        // no abstract unix sockets (X11, other sessions) or signals outside the sandbox,
        // where the kernel supports it (Landlock ABI 6, Linux 6.12+)
        .and_then(|r| r.scope(Scope::from_all(ABI::V6)))
        .and_then(|r| r.create())
        .and_then(|r| r.add_rules(path_beneath_rules(["/"], AccessFs::from_read(abi))))
        .and_then(|r| r.add_rules(path_beneath_rules(&writable, AccessFs::from_all(abi))))
        .and_then(|r| r.restrict_self());
    match res {
        Ok(s) => match s.ruleset {
            RulesetStatus::FullyEnforced => "full",
            RulesetStatus::PartiallyEnforced => "partial",
            RulesetStatus::NotEnforced => "unsupported",
        },
        Err(_) => "error",
    }
}

fn seccomp() -> &'static str {
    let Ok(arch) = TargetArch::try_from(std::env::consts::ARCH) else { return "unsupported" };
    // an empty rule list matches the syscall unconditionally
    let rules: BTreeMap<i64, Vec<SeccompRule>> = DENIED.iter().map(|&s| (s, vec![])).collect();
    let filter = SeccompFilter::new(rules, SeccompAction::Allow, SeccompAction::Errno(libc::EPERM as u32), arch);
    let prog: Result<BpfProgram, _> = filter.map_err(|e| e.to_string()).and_then(|f| f.try_into().map_err(|e: seccompiler::BackendError| e.to_string()));
    match prog.map(|p| seccompiler::apply_filter(&p)) {
        Ok(Ok(())) => "on",
        _ => "error",
    }
}

fn limit(resource: libc::__rlimit_resource_t, value: u64) {
    let lim = libc::rlimit { rlim_cur: value, rlim_max: value };
    unsafe { libc::setrlimit(resource, &lim) };
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut write = Vec::new();
    let mut cpu: u64 = 60;
    let mut i = 0;
    while i < args.len() && args[i] != "--" {
        match args[i].as_str() {
            "--write" if i + 1 < args.len() => { write.push(args[i + 1].clone()); i += 2; }
            "--cpu" if i + 1 < args.len() => { cpu = args[i + 1].parse().unwrap_or(60).clamp(1, 1800); i += 2; }
            other => { eprintln!("osaima-jail: unknown option {}", other); std::process::exit(2); }
        }
    }
    let program: Vec<&String> = args.iter().skip(i + 1).collect();
    if program.is_empty() {
        eprintln!("usage: osaima-jail [--write DIR]... -- PROGRAM [ARGS]...");
        std::process::exit(2);
    }

    unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    limit(libc::RLIMIT_CPU, cpu);
    limit(libc::RLIMIT_FSIZE, 2 << 30);
    limit(libc::RLIMIT_NOFILE, 1024);
    limit(libc::RLIMIT_CORE, 0);
    let ll = landlock(&write);
    // seccomp last: the steps above use syscalls it could block
    let sc = seccomp();
    eprintln!("osaima-jail: landlock={} seccomp={}", ll, sc);

    let argv: Vec<CString> = program.iter().map(|a| CString::new(std::ffi::OsStr::new(a.as_str()).as_bytes()).unwrap()).collect();
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe { libc::execvp(ptrs[0], ptrs.as_ptr()) };
    eprintln!("osaima-jail: cannot run {}: {}", program[0], std::io::Error::last_os_error());
    std::process::exit(127);
}
