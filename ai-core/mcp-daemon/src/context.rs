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

        json!({
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
