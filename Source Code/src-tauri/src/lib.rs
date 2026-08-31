use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub struct RunState {
    pub cancel: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobEvent {
    id: String,
    status: String,
    progress: f64,
    original_size: u64,
    final_size: u64,
    saved: i64,
    out_path: Option<String>,
    message: Option<String>,
}

#[derive(Clone, serde::Deserialize)]
struct FileInput {
    id: String,
    path: String,
}

impl JobEvent {
    fn new(id: &str, original_size: u64) -> Self {
        JobEvent {
            id: id.to_string(),
            status: "working".into(),
            progress: 0.0,
            original_size,
            final_size: 0,
            saved: 0,
            out_path: None,
            message: None,
        }
    }
}

fn emit(app: &AppHandle, ev: JobEvent) {
    let _ = app.emit("job-event", ev);
}

const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "3gp",
];
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct ModeParams {
    crf: &'static str,
    preset: &'static str,
    audio_kbps: &'static str,
    jpeg_q: u8,
    webp_q: &'static str,
}

fn mode_params(mode: &str) -> ModeParams {
    match mode {
        "high" => ModeParams {
            crf: "20",
            preset: "slow",
            audio_kbps: "160",
            jpeg_q: 88,
            webp_q: "85",
        },
        "max" => ModeParams {
            crf: "28",
            preset: "medium",
            audio_kbps: "96",
            jpeg_q: 66,
            webp_q: "65",
        },
        _ => ModeParams {
            crf: "23",
            preset: "medium",
            audio_kbps: "128",
            jpeg_q: 78,
            webp_q: "78",
        },
    }
}

fn find_ffmpeg(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SQUISH_FFMPEG") {
        let pb = PathBuf::from(&p);
        if pb.exists() {
            return Some(pb);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c1 = dir.join("ffmpeg.exe");
            if c1.exists() {
                return Some(c1);
            }
            if let Some(pdir) = dir.parent() {
                let c2 = pdir.join("binaries").join("ffmpeg.exe");
                if c2.exists() {
                    return Some(c2);
                }
                let c3 = pdir.join("ffmpeg.exe");
                if c3.exists() {
                    return Some(c3);
                }
            }
        }
    }
    if let Ok(rd) = app.path().resource_dir() {
        let c = rd.join("ffmpeg.exe");
        if c.exists() {
            return Some(c);
        }
    }
    None
}

fn ffmpeg_cmd(bundled: &Option<PathBuf>) -> Command {
    let mut command = match bundled {
        Some(p) => Command::new(p),
        None => Command::new("ffmpeg"),
    };
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn unique_output(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    for n in 2..1000 {
        let cand = dir.join(format!("{}_{}{}", stem, n, ext));
        if !cand.exists() {
            return cand;
        }
    }
    path
}

fn build_output_path(src: &Path, out_dir: Option<&str>, forced_ext: Option<&str>) -> PathBuf {
    let dir: PathBuf = match out_dir {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => src.parent().unwrap_or(Path::new(".")).to_path_buf(),
    };
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = match forced_ext {
        Some(e) => format!(".{}", e),
        None => match src.extension().and_then(|e| e.to_str()) {
            Some(e) => format!(".{}", e.to_ascii_lowercase()),
            None => String::new(),
        },
    };
    unique_output(dir.join(format!("{}_compressed{}", stem, ext)))
}

fn probe_duration(bundled: &Option<PathBuf>, input: &Path) -> Option<f64> {
    let mut cmd = ffmpeg_cmd(bundled);
    cmd.arg("-hide_banner")
        .arg("-i")
        .arg(input)
        .stderr(Stdio::piped())
        .stdout(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let stderr = child.stderr.take()?;
    let duration = BufReader::new(stderr)
        .lines()
        .find_map(|line| parse_ffmpeg_duration(&line.ok()?));
    let _ = child.wait();
    duration
}

fn parse_ffmpeg_duration(line: &str) -> Option<f64> {
    let timestamp = line
        .trim_start()
        .strip_prefix("Duration:")?
        .split(',')
        .next()?
        .trim();
    let mut parts = timestamp.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn run_video(
    app: &AppHandle,
    bundled: &Option<PathBuf>,
    job_id: &str,
    input: &Path,
    output: &Path,
    mode: &ModeParams,
    token: &AtomicBool,
) -> Result<(), String> {
    let total = probe_duration(bundled, input);
    let mut cmd = ffmpeg_cmd(bundled);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args([
            "-c:v",
            "libx264",
            "-crf",
            mode.crf,
            "-preset",
            mode.preset,
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            mode.audio_kbps,
            "-movflags",
            "+faststart",
            "-progress",
            "pipe:1",
            "-nostats",
        ])
        .arg(output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {}", e))?;
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");

    let err_handle = thread::spawn(move || {
        let mut buf = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buf);
        buf
    });

    let app2 = app.clone();
    let id2 = job_id.to_string();
    let mut last_emit = Instant::now() - Duration::from_millis(1000);
    let mut last_pct = 0.0f64;
    for line in BufReader::new(stdout).lines() {
        if token.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(output);
            return Err("__CANCELLED__".into());
        }
        let line = line.map_err(|e| e.to_string())?;
        let cur_us = if let Some(v) = line.strip_prefix("out_time_us=") {
            v.trim().parse::<f64>().ok()
        } else if let Some(v) = line.strip_prefix("out_time_ms=") {
            v.trim().parse::<f64>().ok()
        } else {
            None
        };
        if let (Some(cur), Some(total)) = (cur_us, total) {
            if total > 0.0 {
                let pct = ((cur / 1_000_000.0) / total * 100.0).min(99.0);
                if last_emit.elapsed() >= Duration::from_millis(250) && pct > last_pct {
                    last_emit = Instant::now();
                    last_pct = pct;
                    let mut ev = JobEvent::new(&id2, 0);
                    ev.progress = pct;
                    emit(&app2, ev);
                }
            }
        }
    }

    let err_out = err_handle.join().unwrap_or_default();
    let status = child.wait().map_err(|e| e.to_string())?;

    if token.load(Ordering::Relaxed) {
        let _ = fs::remove_file(output);
        return Err("__CANCELLED__".into());
    }
    if !status.success() {
        let msg = extract_ffmpeg_error(&err_out);
        let _ = fs::remove_file(output);
        return Err(msg);
    }
    Ok(())
}

fn extract_ffmpeg_error(err: &str) -> String {
    for line in err.lines().rev() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        return l.chars().take(220).collect();
    }
    "ffmpeg failed".into()
}

fn compress_jpeg(input: &Path, output: &Path, quality: u8) -> Result<u64, String> {
    let mut reader = ImageReader::open(input).map_err(|e| format!("Cannot open file: {}", e))?;
    reader.no_limits();
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| format!("Decode failed: {}", e))?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| format!("Decode failed: {}", e))?;
    img.apply_orientation(orientation);
    let mut out = fs::File::create(output).map_err(|e| format!("Cannot write output: {}", e))?;
    let encoder =
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(30, 98));
    img.write_with_encoder(encoder)
        .map_err(|e| format!("Encode failed: {}", e))?;
    Ok(fs::metadata(output).map(|m| m.len()).unwrap_or(0))
}

fn compress_png(input: &Path, output: &Path) -> Result<u64, String> {
    let mut opts = oxipng::Options::from_preset(4);
    opts.strip = oxipng::StripChunks::Safe;
    let infile = oxipng::InFile::Path(input.to_path_buf());
    let outfile = oxipng::OutFile::Path {
        path: Some(output.to_path_buf()),
        preserve_attrs: false,
    };
    oxipng::optimize(&infile, &outfile, &opts)
        .map_err(|e| format!("PNG optimization failed: {}", e))?;
    Ok(fs::metadata(output).map(|m| m.len()).unwrap_or(0))
}

fn compress_webp(
    app: &AppHandle,
    bundled: &Option<PathBuf>,
    input: &Path,
    output: &Path,
    quality: &str,
    token: &AtomicBool,
) -> Result<u64, String> {
    let mut cmd = ffmpeg_cmd(bundled);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args([
            "-c:v",
            "libwebp",
            "-quality",
            quality,
            "-compression_level",
            "5",
            "-nostats",
        ])
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {}", e))?;
    let stderr = child.stderr.take().expect("stderr");
    let handle = thread::spawn(move || {
        let mut buf = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buf);
        buf
    });
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None => {
                if token.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_file(output);
                    return Err("__CANCELLED__".into());
                }
                thread::sleep(Duration::from_millis(80));
            }
        }
    }
    let err_out = handle.join().unwrap_or_default();
    let ok = child
        .try_wait()
        .ok()
        .flatten()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let msg = extract_ffmpeg_error(&err_out);
        let _ = fs::remove_file(output);
        let _ = &app;
        return Err(msg);
    }
    Ok(fs::metadata(output).map(|m| m.len()).unwrap_or(0))
}

fn process_one(
    app: &AppHandle,
    bundled: &Option<PathBuf>,
    item: &FileInput,
    mode: &str,
    out_dir: Option<&str>,
    token: &AtomicBool,
) {
    let input = PathBuf::from(&item.path);
    let meta = match fs::metadata(&input) {
        Ok(m) => m,
        Err(e) => {
            let mut ev = JobEvent::new(&item.id, 0);
            ev.status = "error".into();
            ev.message = Some(format!("Cannot read file: {}", e));
            emit(app, ev);
            return;
        }
    };
    let original_size = meta.len();
    let ext = ext_of(&input);

    let ev = JobEvent::new(&item.id, original_size);
    emit(app, ev);

    let params = mode_params(mode);
    let result: Result<(PathBuf, u64), String> = if VIDEO_EXTS.contains(&ext.as_str()) {
        let out = build_output_path(&input, out_dir, Some("mp4"));
        run_video(app, bundled, &item.id, &input, &out, &params, token).map(|_| {
            (
                out.clone(),
                fs::metadata(&out).map(|m| m.len()).unwrap_or(0),
            )
        })
    } else if IMAGE_EXTS.contains(&ext.as_str()) {
        let out_ext = if ext == "webp" {
            "webp"
        } else if ext == "png" {
            "png"
        } else {
            "jpg"
        };
        let out = build_output_path(&input, out_dir, Some(out_ext));
        let mut ev2 = JobEvent::new(&item.id, original_size);
        ev2.progress = 45.0;
        emit(app, ev2);
        let r = if token.load(Ordering::Relaxed) {
            Err("__CANCELLED__".to_string())
        } else {
            match ext.as_str() {
                "png" => compress_png(&input, &out),
                "webp" => compress_webp(app, bundled, &input, &out, params.webp_q, token),
                _ => compress_jpeg(&input, &out, params.jpeg_q),
            }
        };
        let r = match r {
            Ok(sz) if token.load(Ordering::Relaxed) => {
                let _ = fs::remove_file(&out);
                let _ = sz;
                Err("__CANCELLED__".to_string())
            }
            other => other,
        };
        r.map(|sz| (out.clone(), sz))
    } else {
        Err("Unsupported file type".into())
    };

    match result {
        Ok((out_path, final_size)) => {
            let saved = original_size as i64 - final_size as i64;
            let mut ev = JobEvent::new(&item.id, original_size);
            if saved <= 0 {
                let _ = fs::remove_file(&out_path);
                ev.status = "optimal".into();
                ev.progress = 100.0;
                ev.final_size = original_size;
                ev.saved = 0;
                ev.out_path = None;
                ev.message = Some("Already optimal".into());
            } else {
                ev.status = "done".into();
                ev.progress = 100.0;
                ev.final_size = final_size;
                ev.saved = saved;
                ev.out_path = Some(out_path.to_string_lossy().to_string());
            }
            emit(app, ev);
        }
        Err(e) => {
            let mut ev = JobEvent::new(&item.id, original_size);
            ev.progress = 100.0;
            if e == "__CANCELLED__" {
                ev.status = "cancelled".into();
            } else {
                ev.status = "error".into();
                ev.message = Some(e);
            }
            emit(app, ev);
        }
    }
}

#[tauri::command]
fn compress_files(
    app: AppHandle,
    state: State<'_, RunState>,
    files: Vec<FileInput>,
    mode: String,
    out_dir: Option<String>,
) {
    let token = Arc::new(AtomicBool::new(false));
    *state.cancel.lock().unwrap() = Some(token.clone());
    let bundled = find_ffmpeg(&app);
    thread::spawn(move || {
        for item in &files {
            if token.load(Ordering::Relaxed) {
                let mut ev = JobEvent::new(&item.id, 0);
                ev.status = "cancelled".into();
                ev.progress = 100.0;
                emit(&app, ev);
                continue;
            }
            process_one(&app, &bundled, item, &mode, out_dir.as_deref(), &token);
        }
        let _ = app.emit("queue-done", ());
    });
}

#[tauri::command]
fn cancel_all(state: State<'_, RunState>) {
    if let Some(t) = state.cancel.lock().unwrap().as_ref() {
        t.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
fn has_bundled_ffmpeg(app: AppHandle) -> bool {
    find_ffmpeg(&app).is_some()
}

#[tauri::command]
fn file_size(path: String) -> Option<u64> {
    fs::metadata(&path).ok().map(|m| m.len())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(RunState {
            cancel: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            compress_files,
            cancel_all,
            has_bundled_ffmpeg,
            file_size
        ])
        .run(tauri::generate_context!())
        .expect("error while running Squish");
}

#[cfg(test)]
mod tests {
    use super::parse_ffmpeg_duration;

    #[test]
    fn parses_indented_ffmpeg_duration() {
        let line = "  Duration: 01:02:03.50, start: 0.000000, bitrate: 800 kb/s";
        assert_eq!(parse_ffmpeg_duration(line), Some(3723.5));
    }
}
