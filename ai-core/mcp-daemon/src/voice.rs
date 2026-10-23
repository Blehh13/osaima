//! Local voice: microphone -> whisper.cpp (speech to text) and Piper (text to speech) -> speakers.
//! Everything runs on this machine; nothing is sent to a cloud service.
//!
//! Binaries and models are found like llama-server in models.rs: an environment override, then
//! PATH, then ~/.local/share/osaima/..., then /usr/share/osaima/... (where the ISO ships them).

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const RATE: usize = 16_000;
const CHUNK: usize = RATE * 30 / 1000; // 30 ms
pub const MAX_SECS: u64 = 15;
const NO_SPEECH_SECS: u64 = 6;
const TRAILING_SILENCE_MS: usize = 900;

static LISTENING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static PLAYER: Mutex<Option<u32>> = Mutex::new(None);

// ---------- voice activity detection (pure, unit-tested) ----------

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Vad { Listening, Done, NoSpeech, TooLong }

/// Energy-based endpointing on 30 ms chunks. The first ~300 ms calibrate the room's noise floor;
/// speech starts after three loud chunks and ends after 900 ms of quiet.
pub struct Detector {
    chunks: usize,
    floor: f64,
    calib: Vec<f64>,
    loud_run: usize,
    quiet_run: usize,
    pub speaking: bool,
}

pub fn rms(pcm: &[i16]) -> f64 {
    if pcm.is_empty() { return 0.0; }
    (pcm.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / pcm.len() as f64).sqrt()
}

impl Detector {
    pub fn new() -> Self { Detector { chunks: 0, floor: 0.0, calib: vec![], loud_run: 0, quiet_run: 0, speaking: false } }

    fn threshold(&self) -> f64 { (self.floor * 3.0).max(350.0) }

    /// Feed one 30 ms chunk.
    pub fn feed(&mut self, chunk: &[i16]) -> Vad {
        let e = rms(chunk);
        self.chunks += 1;
        if self.chunks <= 10 && !self.speaking {
            // calibrate on the first 300 ms, unless it is already clearly speech
            if e < 2500.0 { self.calib.push(e); self.floor = self.calib.iter().sum::<f64>() / self.calib.len() as f64; }
            return Vad::Listening;
        }
        let loud = e > self.threshold();
        if self.speaking {
            self.quiet_run = if loud { 0 } else { self.quiet_run + 1 };
            if self.quiet_run * 30 >= TRAILING_SILENCE_MS { return Vad::Done; }
        } else {
            self.loud_run = if loud { self.loud_run + 1 } else { 0 };
            if self.loud_run >= 3 { self.speaking = true; self.quiet_run = 0; }
        }
        let ms = self.chunks * 30;
        if ms >= MAX_SECS as usize * 1000 { return Vad::TooLong; }
        if !self.speaking && ms >= NO_SPEECH_SECS as usize * 1000 { return Vad::NoSpeech; }
        Vad::Listening
    }
}

pub fn wav_bytes(pcm: &[i16]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF"); w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt "); w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&(RATE as u32).to_le_bytes()); w.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes()); w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data"); w.extend_from_slice(&data.to_le_bytes());
    for s in pcm { w.extend_from_slice(&s.to_le_bytes()); }
    w
}

/// Whisper prints bracketed markers for non-speech ("[BLANK_AUDIO]", "(music)"); drop them.
pub fn clean_transcript(raw: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in raw.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------- finding binaries and models ----------

fn home() -> PathBuf { PathBuf::from(std::env::var("HOME").unwrap_or_default()) }

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    path.split(':').map(|d| Path::new(d).join(name)).find(|p| p.is_file())
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var(var).ok().filter(|p| !p.is_empty()).map(PathBuf::from)
}

pub fn whisper_bin() -> Option<PathBuf> {
    if let Some(p) = env_path("OSAIMA_WHISPER_BIN") { return Some(p); }
    for n in ["whisper-cli", "whisper-cpp"] { if let Some(p) = on_path(n) { return Some(p); } }
    for d in [home().join(".local/share/osaima/whisper"), PathBuf::from("/usr/share/osaima/whisper")] {
        for n in ["whisper-cli", "bin/whisper-cli"] { let p = d.join(n); if p.is_file() { return Some(p); } }
    }
    None
}

pub fn whisper_model() -> Option<PathBuf> {
    if let Some(p) = env_path("OSAIMA_WHISPER_MODEL") { return Some(p); }
    for d in [home().join(".local/share/osaima/whisper/models"), PathBuf::from("/usr/share/osaima/whisper/models")] {
        for n in ["ggml-small.en.bin", "ggml-base.en.bin", "ggml-tiny.en.bin"] {
            let p = d.join(n); if p.is_file() { return Some(p); }
        }
    }
    None
}

pub fn piper_bin() -> Option<PathBuf> {
    if let Some(p) = env_path("OSAIMA_PIPER") { return Some(p); }
    if let Some(p) = on_path("piper") { return Some(p); }
    for d in [home().join(".local/share/osaima/piper"), PathBuf::from("/usr/share/osaima/piper")] {
        for n in ["piper/piper", "piper"] { let p = d.join(n); if p.is_file() { return Some(p); } }
    }
    None
}

pub fn piper_voice() -> Option<PathBuf> {
    if let Some(p) = env_path("OSAIMA_PIPER_VOICE") { return Some(p); }
    for d in [home().join(".local/share/osaima/piper"), PathBuf::from("/usr/share/osaima/piper")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut found: Vec<PathBuf> = rd.flatten().map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "onnx").unwrap_or(false)).collect();
        found.sort();
        if let Some(p) = found.into_iter().next() { return Some(p); }
    }
    None
}

#[derive(Clone, Copy, PartialEq)]
enum Recorder { PwRecord, Arecord }

fn recorder() -> Option<Recorder> {
    if on_path("pw-record").is_some() { Some(Recorder::PwRecord) }
    else if on_path("arecord").is_some() { Some(Recorder::Arecord) }
    else { None }
}

fn player() -> Option<&'static str> {
    ["pw-play", "aplay"].into_iter().find(|n| on_path(n).is_some())
}

fn out(cmd: &str, args: &[&str]) -> Option<String> {
    let o = Command::new(cmd).args(args).stderr(Stdio::null()).output().ok()?;
    if o.status.success() { Some(String::from_utf8_lossy(&o.stdout).to_string()) } else { None }
}

/// Is there a capture device? Returns a plain-language reason when not.
fn mic_check() -> Result<(), String> {
    if recorder().is_none() {
        return Err("No recording tool is installed (pw-record from PipeWire, or arecord).".into());
    }
    if let Some(l) = out("arecord", &["-l"]) { if l.contains("card ") { return Ok(()); } }
    if let Some(s) = out("pactl", &["list", "short", "sources"]) {
        if s.lines().any(|l| !l.contains(".monitor")) { return Ok(()); }
    }
    if let Ok(c) = std::fs::read_to_string("/proc/asound/cards") {
        if c.contains('[') { return Ok(()); }
    }
    Err("No microphone was found. This computer has no audio input (virtual machines usually don't pass one through). Plug in a microphone or attach one to the VM, then try again.".into())
}

fn file_name(p: Option<PathBuf>) -> Option<String> {
    p.map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
}

pub fn status() -> Value {
    let mic = mic_check();
    let wbin = whisper_bin();
    let wmodel = whisper_model();
    let pbin = piper_bin();
    let pvoice = piper_voice();
    let play = player();
    let mut reasons: Vec<String> = vec![];
    if let Err(e) = &mic { reasons.push(e.clone()); }
    if wbin.is_none() { reasons.push("The speech recognizer (whisper.cpp) isn't installed.".into()); }
    if wmodel.is_none() { reasons.push("The speech recognition model (ggml-base.en.bin) isn't downloaded.".into()); }
    let can_listen = mic.is_ok() && wbin.is_some() && wmodel.is_some();
    let mut speak_reasons: Vec<String> = vec![];
    if pbin.is_none() { speak_reasons.push("The voice synthesizer (Piper) isn't installed.".into()); }
    if pvoice.is_none() { speak_reasons.push("No Piper voice file (.onnx) is downloaded.".into()); }
    if play.is_none() { speak_reasons.push("No audio player (pw-play or aplay) was found.".into()); }
    json!({
        "can_listen": can_listen,
        "can_speak": speak_reasons.is_empty(),
        "listening": LISTENING.load(Ordering::SeqCst),
        "mic": { "present": mic.is_ok(), "reason": mic.err() },
        "whisper": { "binary": wbin.map(|p| p.display().to_string()), "model": file_name(wmodel) },
        "piper": { "binary": pbin.map(|p| p.display().to_string()), "voice": file_name(pvoice), "player": play },
        "reasons": reasons,
        "speak_reasons": speak_reasons,
    })
}

fn join_reasons(v: &Value) -> String {
    v.as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

// ---------- listening ----------

fn spawn_recorder(r: Recorder) -> std::io::Result<Child> {
    let mut c = match r {
        Recorder::PwRecord => {
            let mut c = Command::new("pw-record");
            c.args(["--rate", "16000", "--channels", "1", "--format", "s16", "-"]);
            c
        }
        Recorder::Arecord => {
            let mut c = Command::new("arecord");
            c.args(["-q", "-t", "raw", "-f", "S16_LE", "-r", "16000", "-c", "1"]);
            c
        }
    };
    c.stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
}

/// Record until the speaker stops (or 15 s), then transcribe. Blocking; call from spawn_blocking.
pub fn listen() -> Result<Value, String> {
    let st = status();
    if st["can_listen"] != json!(true) { return Err(join_reasons(&st["reasons"])); }
    if LISTENING.swap(true, Ordering::SeqCst) { return Err("Already listening.".into()); }
    CANCEL.store(false, Ordering::SeqCst);
    let res = listen_inner();
    LISTENING.store(false, Ordering::SeqCst);
    res
}

fn listen_inner() -> Result<Value, String> {
    let r = recorder().ok_or("No recording tool.")?;
    let mut child = spawn_recorder(r).map_err(|e| format!("Couldn't start the recorder: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("Recorder gave no output.")?;
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) { if n == 0 || tx.send(buf[..n].to_vec()).is_err() { break; } }
    });

    let mut det = Detector::new();
    let mut pcm: Vec<i16> = vec![];
    let mut raw: Vec<u8> = vec![];
    let mut header_checked = false;
    let mut fed = 0usize; // samples already given to the detector
    let started = Instant::now();
    let mut verdict = Vad::Listening;
    let mut cancelled = false;
    while verdict == Vad::Listening {
        if CANCEL.load(Ordering::SeqCst) { cancelled = true; break; }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(b) => raw.extend_from_slice(&b),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if started.elapsed() > Duration::from_secs(NO_SPEECH_SECS + 2) && pcm.is_empty() {
                    let _ = child.kill(); let _ = child.wait();
                    return Err("The microphone isn't delivering any audio. Check that it is plugged in and not muted.".into());
                }
                continue;
            }
            Err(_) => break, // recorder exited
        }
        if !header_checked {
            if raw.len() < 44 { continue; }
            header_checked = true;
            if &raw[..4] == b"RIFF" { raw.drain(..44); }
        }
        let n = raw.len() / 2;
        pcm.extend((0..n).map(|i| i16::from_le_bytes([raw[2 * i], raw[2 * i + 1]])));
        raw.drain(..n * 2);
        while pcm.len() - fed >= CHUNK && verdict == Vad::Listening {
            verdict = det.feed(&pcm[fed..fed + CHUNK]);
            fed += CHUNK;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    if cancelled { return Ok(json!({ "text": "", "cancelled": true, "ms": 0 })); }
    if verdict == Vad::NoSpeech || !det.speaking {
        return Ok(json!({ "text": "", "no_speech": true, "ms": 0, "message": "I didn't hear anything." }));
    }
    let audio_ms = pcm.len() * 1000 / RATE;
    let t0 = Instant::now();
    let (text, model) = transcribe(&pcm)?;
    Ok(json!({ "text": text, "ms": t0.elapsed().as_millis() as u64, "audio_ms": audio_ms, "model": model }))
}

pub fn transcribe(pcm: &[i16]) -> Result<(String, String), String> {
    let bin = whisper_bin().ok_or("whisper.cpp isn't installed.")?;
    let model = whisper_model().ok_or("No whisper model found.")?;
    let dir = runtime_dir();
    let wav = dir.join(format!("voice-{}.wav", std::process::id()));
    std::fs::File::create(&wav).and_then(|mut f| f.write_all(&wav_bytes(pcm))).map_err(|e| format!("Couldn't write the recording: {e}"))?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8).to_string();
    let o = Command::new(&bin)
        .args(["-m", model.to_str().unwrap_or(""), "-f", wav.to_str().unwrap_or(""), "-nt", "-np", "-l", "en", "-t", &threads])
        .stdin(Stdio::null()).stderr(Stdio::null()).output();
    let _ = std::fs::remove_file(&wav);
    let o = o.map_err(|e| format!("Couldn't run whisper: {e}"))?;
    if !o.status.success() { return Err("The speech recognizer failed.".into()); }
    let name = model.file_stem().unwrap_or_default().to_string_lossy().replace("ggml-", "");
    Ok((clean_transcript(&String::from_utf8_lossy(&o.stdout)), name))
}

fn runtime_dir() -> PathBuf {
    let d = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join("osaima");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn cancel() -> Value {
    let was = LISTENING.load(Ordering::SeqCst);
    if was { CANCEL.store(true, Ordering::SeqCst); }
    json!({ "cancelled": was })
}

// ---------- speaking ----------

/// Shorten to a couple of sentences; long answers are for reading, not hearing.
pub fn speakable(text: &str) -> String {
    let t: String = text.chars().filter(|c| !matches!(c, '*' | '`' | '#' | '_')).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= 300 { return t; }
    let cut: String = t.chars().take(300).collect();
    match cut.rfind(|c| c == '.' || c == '!' || c == '?') {
        Some(i) if i > 40 => cut[..=i].to_string(),
        _ => cut,
    }
}

pub fn speak(text: &str) -> Result<Value, String> {
    let st = status();
    if st["can_speak"] != json!(true) { return Err(join_reasons(&st["speak_reasons"])); }
    let t = speakable(text);
    if t.is_empty() { return Ok(json!({ "spoken": false })); }
    stop();
    let bin = piper_bin().ok_or("Piper missing")?;
    let voice = piper_voice().ok_or("voice missing")?;
    let play = player().ok_or("player missing")?;
    let wav = runtime_dir().join(format!("speak-{}.wav", std::process::id()));
    let t0 = Instant::now();
    let mut p = Command::new(&bin);
    p.args(["--model", voice.to_str().unwrap_or(""), "--output_file", wav.to_str().unwrap_or("")])
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(d) = bin.parent() { p.current_dir(d); }
    let mut child = p.spawn().map_err(|e| format!("Couldn't start Piper: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(t.as_bytes()).map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() { return Err("Piper couldn't make the audio.".into()); }
    let synth_ms = t0.elapsed().as_millis() as u64;
    let pl = Command::new(play).arg(&wav).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|e| format!("Couldn't play audio: {e}"))?;
    *PLAYER.lock().unwrap() = Some(pl.id());
    // reap in the background so no zombie is left and the file is removed when playback ends
    std::thread::spawn(move || { let mut pl = pl; let _ = pl.wait(); let _ = std::fs::remove_file(&wav); });
    Ok(json!({ "spoken": true, "chars": t.chars().count(), "synth_ms": synth_ms }))
}

pub fn stop() -> Value {
    let pid = PLAYER.lock().unwrap().take();
    if let Some(pid) = pid { let _ = Command::new("kill").arg(pid.to_string()).stderr(Stdio::null()).status(); }
    json!({ "stopped": pid.is_some() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: f64, n: usize) -> Vec<i16> { (0..n).map(|i| (amp * (i as f64 * 0.3).sin()) as i16).collect() }
    fn run(det: &mut Detector, pcm: &[i16]) -> Vad {
        let mut v = Vad::Listening;
        for c in pcm.chunks_exact(CHUNK) { v = det.feed(c); if v != Vad::Listening { break; } }
        v
    }

    #[test] fn rms_of_silence_is_zero() { assert_eq!(rms(&vec![0i16; 480]), 0.0); }

    #[test] fn speech_then_silence_ends() {
        let mut pcm = tone(60.0, RATE / 2);
        pcm.extend(tone(6000.0, RATE));
        pcm.extend(tone(60.0, RATE * 2));
        let mut d = Detector::new();
        assert_eq!(run(&mut d, &pcm), Vad::Done);
        assert!(d.speaking);
    }

    #[test] fn only_silence_gives_no_speech() {
        let mut d = Detector::new();
        assert_eq!(run(&mut d, &tone(50.0, RATE * 8)), Vad::NoSpeech);
    }

    #[test] fn constant_speech_hits_max() {
        let mut pcm = tone(50.0, RATE / 2);
        pcm.extend(tone(8000.0, RATE * 20));
        let mut d = Detector::new();
        assert_eq!(run(&mut d, &pcm), Vad::TooLong);
    }

    #[test] fn short_pause_does_not_end() {
        let mut pcm = tone(60.0, RATE / 2);
        pcm.extend(tone(6000.0, RATE / 2));
        pcm.extend(tone(60.0, RATE / 3)); // 333 ms gap < 900 ms
        pcm.extend(tone(6000.0, RATE / 2));
        let mut d = Detector::new();
        assert_eq!(run(&mut d, &pcm), Vad::Listening);
    }

    #[test] fn click_is_not_speech() {
        let mut pcm = tone(60.0, RATE / 2);
        pcm.extend(tone(9000.0, 480)); // one 30 ms click
        pcm.extend(tone(60.0, RATE * 7));
        let mut d = Detector::new();
        assert_eq!(run(&mut d, &pcm), Vad::NoSpeech);
    }

    #[test] fn noisy_room_raises_threshold() {
        let mut pcm = tone(900.0, RATE * 8); // steady hum is the floor, not speech
        pcm.truncate(RATE * 8);
        let mut d = Detector::new();
        assert_eq!(run(&mut d, &pcm), Vad::NoSpeech);
    }

    #[test] fn wav_header_is_44_bytes() {
        let w = wav_bytes(&[1, 2, 3]);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(&w[..4], b"RIFF");
    }

    #[test] fn transcript_markers_removed() {
        assert_eq!(clean_transcript(" [BLANK_AUDIO]\n"), "");
        assert_eq!(clean_transcript(" Open the terminal. (music)\n"), "Open the terminal.");
    }

    #[test] fn speakable_is_short() {
        let long = "Sentence one is here. ".repeat(40);
        assert!(speakable(&long).chars().count() <= 300);
        assert_eq!(speakable("**Done**"), "Done");
    }
}
