//! Local-model advice and download: detects GPUs (no root), recommends a GGUF
//! that fits, and downloads it (resumable, one at a time, user-approved).

use crate::security::AuditLog;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug)]
pub struct Candidate {
    pub id: &'static str,
    pub name: &'static str,
    pub file: &'static str,
    pub size_mb: u64,
    pub url: &'static str,
    pub why: &'static str,
}

// Sizes/URLs verified with HEAD requests against huggingface.co.
const QWEN7: Candidate = Candidate {
    id: "qwen2.5-7b-instruct", name: "Qwen2.5 7B Instruct", file: "qwen2.5-7b-instruct-Q4_K_M.gguf", size_mb: 4466,
    url: "https://huggingface.co/bartowski/Qwen2.5-7B-Instruct-GGUF/resolve/main/Qwen2.5-7B-Instruct-Q4_K_M.gguf",
    why: "Your graphics card has enough memory for a 7B model, which understands requests noticeably better than the small built-in one.",
};
const PHI4: Candidate = Candidate {
    id: "phi-4-mini", name: "Phi-4 mini", file: "phi-4-mini-Q4_K_M.gguf", size_mb: 2376,
    url: "https://huggingface.co/bartowski/microsoft_Phi-4-mini-instruct-GGUF/resolve/main/microsoft_Phi-4-mini-instruct-Q4_K_M.gguf",
    why: "Your graphics card can run a 3.8B model comfortably, a clear step up from the smallest models.",
};

fn candidates() -> [&'static Candidate; 2] { [&QWEN7, &PHI4] }

/// Picks a model from GPU memory (MB) and the model files already installed.
pub fn recommend(vram_mb: u64, installed: &[String]) -> Result<&'static Candidate, String> {
    let has = |c: &Candidate| installed.iter().any(|f| f.eq_ignore_ascii_case(c.file));
    if vram_mb >= 7500 {
        if !has(&QWEN7) { return Ok(&QWEN7); }
        return Err("The best model for this graphics card is already installed.".into());
    }
    if vram_mb >= 5000 {
        // Phi-4 mini (3.8B) is the shipped default; only offer it if it is missing.
        if !has(&PHI4) { return Ok(&PHI4); }
        return Err("A model that fits this graphics card is already installed.".into());
    }
    Err("No graphics card with enough memory found; paste a Gemini key for fast answers.".into())
}

pub fn models_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OSAIMA_MODELS_DIR") { if !d.is_empty() { return PathBuf::from(d); } }
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/osaima/models")
}

fn installed_files() -> Vec<String> {
    std::fs::read_dir(models_dir()).map(|rd| rd.flatten()
        .filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".gguf")).collect())
        .unwrap_or_default()
}

// ---------- hardware detection ----------

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_string())
}

/// PCI address ("01:00.0") to the lspci name of each display controller.
fn lspci_names() -> Vec<(String, String)> {
    run("lspci", &[]).map(|t| t.lines()
        .filter(|l| l.contains("VGA compatible") || l.contains("3D controller") || l.contains("Display controller"))
        .filter_map(|l| {
            let (addr, rest) = l.split_once(' ')?;
            Some((addr.to_string(), rest.splitn(2, ": ").nth(1).unwrap_or(rest).to_string()))
        }).collect()).unwrap_or_default()
}

/// Least VRAM a GeForce of this name ships with, for when NVIDIA's driver isn't
/// loaded (nouveau) and the card can't be asked. Laptop variants are often smaller,
/// so these are the low end.
pub fn nvidia_vram_guess(name: &str) -> u64 {
    let n = name.to_lowercase();
    const TABLE: &[(&str, u64)] = &[
        ("4090", 16384), ("4080", 12288), ("4070", 8192), ("4060", 8192), ("4050", 6144),
        ("3090", 24576), ("3080", 8192), ("3070", 8192), ("3060", 6144), ("3050", 4096),
        ("2080", 8192), ("2070", 8192), ("2060", 6144), ("1660", 6144), ("1650", 4096),
        ("1080", 8192), ("1070", 8192), ("1060", 3072),
    ];
    TABLE.iter().find(|(k, _)| n.contains(k)).map(|(_, v)| *v).unwrap_or(0)
}

/// "NVIDIA Corporation TU117M [GeForce GTX 1650 Mobile / Max-Q] (rev a1)" to
/// "NVIDIA GeForce GTX 1650 Mobile / Max-Q".
pub fn short_name(vendor: &str, lspci: &str) -> String {
    let inner: Vec<&str> = lspci.split('[').skip(1).filter_map(|p| p.split(']').next())
        .filter(|p| !p.contains('/') || p.contains(' ')).filter(|p| *p != "AMD/ATI").collect();
    match inner.last() {
        Some(m) if m.to_lowercase().contains(&vendor.to_lowercase()) => m.to_string(),
        Some(m) => format!("{} {}", vendor, m),
        None => lspci.split(" (rev").next().unwrap_or(lspci).to_string(),
    }
}

pub fn detect_gpus() -> Vec<Value> {
    let mut gpus: Vec<Value> = vec![];
    // NVIDIA: nvidia-smi gives VRAM; /proc gives the name when the tool is missing.
    if let Some(t) = run("nvidia-smi", &["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]) {
        for l in t.lines().filter(|l| !l.trim().is_empty()) {
            let mut it = l.rsplitn(2, ',');
            let mem = it.next().and_then(|m| m.trim().parse::<u64>().ok()).unwrap_or(0);
            let name = it.next().unwrap_or("NVIDIA GPU").trim().to_string();
            gpus.push(json!({ "vendor": "NVIDIA", "name": name, "vram_mb": mem, "integrated": false }));
        }
    }
    if gpus.is_empty() {
        if let Ok(rd) = std::fs::read_dir("/proc/driver/nvidia/gpus") {
            for e in rd.flatten() {
                let txt = std::fs::read_to_string(e.path().join("information")).unwrap_or_default();
                let name = txt.lines().find_map(|l| l.strip_prefix("Model:")).map(|s| s.trim().to_string()).unwrap_or("NVIDIA GPU".into());
                gpus.push(json!({ "vendor": "NVIDIA", "name": name, "vram_mb": nvidia_vram_guess(&name), "vram_estimated": true, "integrated": false }));
            }
        }
    }
    let have_nvidia = !gpus.is_empty();
    // every other card via sysfs, named by its own PCI address
    let names = lspci_names();
    if let Ok(rd) = std::fs::read_dir("/sys/class/drm") {
        let mut cards: Vec<_> = rd.flatten().filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("card") && n.len() > 4 && n[4..].chars().all(|c| c.is_ascii_digit())
        }).collect();
        cards.sort_by_key(|e| e.file_name());
        for e in cards {
            let dev = e.path().join("device");
            let addr = std::fs::read_link(&dev).ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
                .map(|a| a.splitn(2, ':').nth(1).unwrap_or(&a).to_string()).unwrap_or_default();
            let name = names.iter().find(|(a, _)| *a == addr).map(|(_, n)| n.clone());
            let vendor = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
            let vendor = match vendor.trim() { "0x1002" => "AMD", "0x8086" => "Intel", "0x10de" => "NVIDIA", _ => continue };
            if vendor == "NVIDIA" {
                if have_nvidia { continue; }
                // nouveau or no driver: only the name is known
                let name = name.unwrap_or_else(|| "NVIDIA graphics".into());
                gpus.push(json!({ "vendor": vendor, "name": short_name(vendor, &name), "vram_mb": nvidia_vram_guess(&name), "vram_estimated": true,
                                  "integrated": false, "note": "NVIDIA's driver isn't loaded, so memory is estimated from the model name" }));
                continue;
            }
            let vram = std::fs::read_to_string(dev.join("mem_info_vram_total")).ok()
                .and_then(|s| s.trim().parse::<u64>().ok()).map(|b| b / 1048576).unwrap_or(0);
            // Intel graphics and AMD APUs (small carve-out of system RAM) share memory with the CPU
            let integrated = vendor == "Intel" || vram <= 2048;
            gpus.push(json!({ "vendor": vendor, "name": name.map(|n| short_name(vendor, &n)).unwrap_or_else(|| format!("{} graphics", vendor)), "vram_mb": vram, "integrated": integrated }));
        }
    }
    gpus
}

fn llama_server() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OSAIMA_LLAMA_SERVER") { if !p.is_empty() { return Some(PathBuf::from(p)); } }
    if let Some(p) = run("sh", &["-c", "command -v llama-server"]) { let p = p.trim(); if !p.is_empty() { return Some(p.into()); } }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/osaima/llama");
    let out = run("find", &[home.to_str()?, "-name", "llama-server", "-type", "f"])?;
    out.lines().next().map(PathBuf::from)
}

/// True when this llama-server build lists a GPU backend device.
pub fn parse_gpu_offload(list_devices: &str) -> bool {
    list_devices.lines().any(|l| {
        let l = l.trim().to_lowercase();
        ["cuda", "vulkan", "rocm", "hip", "sycl", "metal"].iter().any(|b| l.starts_with(b)) && l.contains(':')
    })
}

fn gpu_offload() -> bool {
    let Some(s) = llama_server() else { return false };
    let o = Command::new(&s).arg("--list-devices").output().ok();
    o.map(|o| parse_gpu_offload(&format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))).unwrap_or(false)
}

fn ram_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo").ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("MemTotal:").and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())))
        .map(|k| k / 1024).unwrap_or(0)
}

fn current_model() -> Value {
    match std::env::var("OSAIMA_LOCAL_MODEL").ok().filter(|s| !s.is_empty()) {
        Some(p) => json!(Path::new(&p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or(p)),
        None => Value::Null,
    }
}

pub fn gpu_info() -> Value {
    let gpus = detect_gpus();
    // integrated graphics share system memory and don't help a model run faster
    let vram = gpus.iter().filter(|g| g["integrated"] != true).filter_map(|g| g["vram_mb"].as_u64()).max().unwrap_or(0);
    let offload = !gpus.is_empty() && gpu_offload();
    let (rec, reason) = match recommend(vram, &installed_files()) {
        Ok(c) => (json!({ "id": c.id, "name": c.name, "size_mb": c.size_mb, "url": c.url, "why": c.why }), Value::Null),
        Err(why) => (Value::Null, json!(why)),
    };
    json!({
        "gpus": gpus, "ram_mb": ram_mb(), "gpu_offload": offload, "current_model": current_model(),
        "recommendation": rec, "no_recommendation_reason": reason,
        "offload_note": if !gpus.is_empty() && !offload {
            "The local engine installed here runs on the processor only, so a bigger model would be slower, not faster, until a GPU-enabled engine is installed."
        } else { "" },
    })
}

// ---------- download ----------

#[derive(Clone)]
struct State { state: &'static str, bytes: u64, total: u64, speed_bps: f64, error: Option<String>, id: String }

static STATE: Mutex<Option<State>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);

fn set(f: impl FnOnce(&mut State)) {
    let mut g = STATE.lock().unwrap();
    if let Some(s) = g.as_mut() { f(s); }
}

pub fn eta_s(total: u64, bytes: u64, speed_bps: f64) -> Option<u64> {
    if speed_bps < 1.0 || total <= bytes { return None; }
    Some(((total - bytes) as f64 / speed_bps).ceil() as u64)
}

pub fn status() -> Value {
    let g = STATE.lock().unwrap();
    match g.as_ref() {
        None => json!({ "state": "idle", "bytes": 0, "total": 0, "speed_bps": 0, "eta_s": null, "error": null }),
        Some(s) => json!({ "state": s.state, "id": s.id, "bytes": s.bytes, "total": s.total,
            "speed_bps": s.speed_bps.round(),
            "eta_s": if s.state == "downloading" { json!(eta_s(s.total, s.bytes, s.speed_bps)) } else { Value::Null },
            "error": s.error }),
    }
}

pub fn cancel() -> Value {
    CANCEL.store(true, Ordering::SeqCst);
    json!({ "cancelled": true })
}

pub fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

/// Starts the download of a recommended model. `approved` must be true.
pub fn start(id: &str, approved: bool, audit: Arc<AuditLog>) -> Result<Value, String> {
    if !approved {
        return Err("Not done: downloading a model needs your approval first.".into());
    }
    let c = candidates().into_iter().find(|c| c.id == id).ok_or("Unknown model.")?;
    begin(c.url.to_string(), models_dir(), c.file.to_string(), c.size_mb * 1048576, Some(c.id.to_string()), audit)
}

fn begin(url: String, dir: PathBuf, file: String, expect: u64, id: Option<String>, audit: Arc<AuditLog>) -> Result<Value, String> {
    {
        let mut g = STATE.lock().unwrap();
        if matches!(g.as_ref().map(|s| s.state), Some("downloading") | Some("verifying")) {
            return Err("A model download is already running.".into());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create the models folder: {}", e))?;
        let part = dir.join(format!("{}.part", file));
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let need = expect.saturating_sub(have) + 300 * 1048576;
        if let Some(free) = free_bytes(&dir) {
            if free < need {
                return Err(format!("Not enough free disk space: {} MB needed, {} MB free.", need / 1048576, free / 1048576));
            }
        }
        CANCEL.store(false, Ordering::SeqCst);
        *g = Some(State { state: "downloading", bytes: have, total: expect, speed_bps: 0.0, error: None, id: id.clone().unwrap_or_default() });
    }
    audit.record("model_download_start", json!({ "id": id, "file": file, "url": url }));
    let magic: Option<&'static [u8]> = if file.ends_with(".gguf") { Some(b"GGUF") } else { None };
    tokio::spawn(async move {
        match download(&url, &dir, &file, magic).await {
            Ok(path) => {
                let saved = if id.is_some() { crate::llm::save_env_key("OSAIMA_LOCAL_MODEL", &path.to_string_lossy()) } else { Ok(()) };
                match saved {
                    Ok(()) => {
                        // so gpu_info reports it as the current model without a restart
                        if id.is_some() { std::env::set_var("OSAIMA_LOCAL_MODEL", &path); }
                        set(|s| s.state = "done")
                    }
                    Err(e) => set(|s| { s.state = "error"; s.error = Some(format!("Downloaded, but could not save the setting: {}", e)); }),
                }
                audit.record("model_download_done", json!({ "id": id, "file": file, "path": path }));
            }
            Err(e) => {
                let cancelled = e == "cancelled";
                set(|s| { if cancelled { s.state = "idle"; s.error = None; } else { s.state = "error"; s.error = Some(e.clone()); } });
                audit.record(if cancelled { "model_download_cancelled" } else { "model_download_failed" }, json!({ "id": id, "file": file, "error": e }));
            }
        }
    });
    Ok(status())
}

async fn download(url: &str, dir: &Path, file: &str, magic: Option<&[u8]>) -> Result<PathBuf, String> {
    use tokio::io::AsyncWriteExt;
    let part = dir.join(format!("{}.part", file));
    let dest = dir.join(file);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
    let mut req = client.get(url);
    if have > 0 { req = req.header("Range", format!("bytes={}-", have)); }
    let mut resp = req.send().await.map_err(|e| format!("Could not reach the download server: {}", e))?;
    let code = resp.status().as_u16();
    if code == 200 {
        have = 0; // server ignored Range: restart
    } else if code != 206 && !(code == 416 && have > 0) {
        return Err(format!("The download server answered {}.", code));
    }
    if code != 416 {
        let remaining = resp.content_length().unwrap_or(0);
        if remaining > 0 { set(|s| s.total = have + remaining); }
        let mut f = tokio::fs::OpenOptions::new().create(true).write(true).append(have > 0).truncate(have == 0).open(&part)
            .await.map_err(|e| format!("Cannot write the file: {}", e))?;
        let mut bytes = have;
        let (mut t0, mut b0, mut speed) = (Instant::now(), bytes, 0.0f64);
        set(|s| s.bytes = bytes);
        loop {
            if CANCEL.load(Ordering::SeqCst) { return Err("cancelled".into()); }
            match tokio::time::timeout(std::time::Duration::from_secs(60), resp.chunk()).await {
                Err(_) => return Err("The download stalled; try again to resume.".into()),
                Ok(Err(e)) => return Err(format!("The download was interrupted ({}); try again to resume.", e)),
                Ok(Ok(None)) => break,
                Ok(Ok(Some(chunk))) => {
                    f.write_all(&chunk).await.map_err(|e| format!("Cannot write the file: {}", e))?;
                    bytes += chunk.len() as u64;
                    let dt = t0.elapsed().as_secs_f64();
                    if dt >= 1.0 {
                        let inst = (bytes - b0) as f64 / dt;
                        speed = if speed == 0.0 { inst } else { speed * 0.6 + inst * 0.4 };
                        t0 = Instant::now();
                        b0 = bytes;
                    }
                    let sp = speed;
                    set(|s| { s.bytes = bytes; s.speed_bps = sp; });
                }
            }
        }
        f.flush().await.map_err(|e| e.to_string())?;
    }
    set(|s| { s.state = "verifying"; s.speed_bps = 0.0; });
    let size = std::fs::metadata(&part).map(|m| m.len()).map_err(|e| e.to_string())?;
    if let Some(m) = magic {
        use std::io::Read;
        let mut head = vec![0u8; m.len()];
        std::fs::File::open(&part).and_then(|mut f| f.read_exact(&mut head)).map_err(|e| e.to_string())?;
        if head != m {
            let _ = std::fs::remove_file(&part);
            return Err("The downloaded file is not a valid model; it was removed.".into());
        }
    }
    set(|s| { s.bytes = size; s.total = size; });
    std::fs::rename(&part, &dest).map_err(|e| format!("Cannot finish the file: {}", e))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    #[test]
    fn vram_from_nvidia_names() {
        assert_eq!(super::nvidia_vram_guess("NVIDIA Corporation AD107M [GeForce RTX 4060 Max-Q / Mobile] (rev a1)"), 8192);
        assert_eq!(super::nvidia_vram_guess("NVIDIA Corporation TU117M [GeForce GTX 1650 Mobile / Max-Q] (rev a1)"), 4096);
        assert_eq!(super::nvidia_vram_guess("Some Quadro"), 0);
        assert_eq!(super::short_name("NVIDIA", "NVIDIA Corporation TU117M [GeForce GTX 1650 Mobile / Max-Q] (rev a1)"), "NVIDIA GeForce GTX 1650 Mobile / Max-Q");
        assert_eq!(super::short_name("AMD", "Advanced Micro Devices, Inc. [AMD/ATI] Picasso/Raven 2 [Radeon Vega Series / Radeon Vega Mobile Series] (rev c2)"), "AMD Radeon Vega Series / Radeon Vega Mobile Series");
    }

    use super::*;

    #[test]
    fn picks_by_vram() {
        assert_eq!(recommend(12000, &[]).unwrap().id, "qwen2.5-7b-instruct");
        assert_eq!(recommend(8192, &[]).unwrap().id, "qwen2.5-7b-instruct");
        assert_eq!(recommend(6000, &[]).unwrap().id, "phi-4-mini");
        assert!(recommend(4096, &[]).is_err());
        assert!(recommend(0, &[]).unwrap_err().contains("Gemini"));
    }
    #[test]
    fn skips_installed() {
        assert!(recommend(6000, &["phi-4-mini-Q4_K_M.gguf".into()]).is_err());
        assert!(recommend(16000, &["qwen2.5-7b-instruct-Q4_K_M.gguf".into()]).is_err());
    }
    #[test]
    fn eta_math() {
        assert_eq!(eta_s(1000, 0, 100.0), Some(10));
        assert_eq!(eta_s(1000, 995, 100.0), Some(1));
        assert_eq!(eta_s(1000, 1000, 100.0), None);
        assert_eq!(eta_s(1000, 0, 0.0), None);
    }
    #[test]
    fn offload_parse() {
        assert!(!parse_gpu_offload("Available devices:\n  CPU: 16 cores\n"));
        assert!(parse_gpu_offload("Available devices:\n  CUDA0: NVIDIA RTX 3060 (12288 MiB)\n"));
        assert!(parse_gpu_offload("Available devices:\n  Vulkan0: AMD Radeon (8192 MiB)\n"));
    }

    // Local file server with Range support.
    async fn serve(body: Vec<u8>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                let body = body.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let start = req.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.trim().trim_end_matches('-').parse::<usize>().ok());
                    let (head, data) = match start {
                        Some(st) => (format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() - st), &body[st..]),
                        None => (format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()), &body[..]),
                    };
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(data).await;
                });
            }
        });
        format!("http://{}/f.bin", addr)
    }

    #[tokio::test]
    async fn downloads_and_resumes() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let url = serve(body.clone()).await;
        let dir = std::env::temp_dir().join(format!("osaima-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // pre-seed a partial file to exercise Range resume
        std::fs::write(dir.join("f.bin.part"), &body[..50_000]).unwrap();
        *STATE.lock().unwrap() = Some(State { state: "downloading", bytes: 0, total: 0, speed_bps: 0.0, error: None, id: "t".into() });
        CANCEL.store(false, Ordering::SeqCst);
        let p = download(&url, &dir, "f.bin", None).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), body);
        assert!(!dir.join("f.bin.part").exists());
        assert_eq!(status()["bytes"], 200_000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
