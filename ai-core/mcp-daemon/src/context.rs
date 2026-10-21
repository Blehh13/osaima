use serde_json::{json, Value};
use sysinfo::{System};
use std::sync::Mutex;

pub struct ContextProvider {
    sys: Mutex<System>,
}

impl ContextProvider {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        Self {
            sys: Mutex::new(sys),
        }
    }

    pub fn get_system_stats(&self) -> Value {
        let mut sys = self.sys.lock().unwrap();
        sys.refresh_memory();
        sys.refresh_cpu();

        let total_mem = sys.total_memory();
        let used_mem = sys.used_memory();
        let cpus = sys.cpus();
        
        let cpu_usage: f32 = if !cpus.is_empty() {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
        } else {
            0.0
        };

        // disk: the filesystem holding the home folder (on the live ISO, the RAM overlay)
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        let disk = std::ffi::CString::new(home).ok().and_then(|p| {
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            (unsafe { libc::statvfs(p.as_ptr(), &mut st) } == 0).then(|| {
                let total = st.f_blocks as u64 * st.f_frsize as u64;
                json!({ "total_bytes": total, "used_bytes": total - st.f_bavail as u64 * st.f_frsize as u64 })
            })
        });
        json!({
            "disk": disk,
            "os": System::name().unwrap_or_else(|| "Unknown".to_string()),
            "kernel": System::kernel_version().unwrap_or_else(|| "Unknown".to_string()),
            "host": System::host_name().unwrap_or_else(|| "Unknown".to_string()),
            "memory": {
                "total_bytes": total_mem,
                "used_bytes": used_mem,
            },
            "cpu": {
                "usage_percent": cpu_usage,
                "cores": cpus.len(),
            }
        })
    }
}
