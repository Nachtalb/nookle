//! ffmpeg-based normalisation, Telegram copies, thumbnails, frames and perceptual hashes.
//!
//! Shells out to `ffmpeg`/`ffprobe` ≥ 8 (needed to decode animated WebP).
//!
//! - Still images: WebP q80, fit inside 4096×4096.
//! - Animated WebP: q75, max 720 px wide, resampled to 30 fps only when the source is faster.
//! - AV1: SVT-AV1 preset 8 CRF 38, fit inside 1920×1080 (or 1080×1920 portrait); Opus 64 kb/s stereo.
//! - Telegram copies: libx264 veryfast CRF 23, AAC 128 kb/s, even dimensions.
//! - `distance`: for each hash of the shorter list the Hamming distance to its nearest hash in the other list, worst case wins (64 if a list is empty).

use nookle_core::model::Kind;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported media: {0}")]
    Unsupported(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub kind: Kind,
    pub width: u32,
    pub height: u32,
    pub duration_ms: Option<u64>,
    pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub path: PathBuf,
    pub ext: &'static str,
    pub mime: &'static str,
    pub probe: Probe,
    pub size: u64,
}

// Composites alpha onto white: 1 - (1 - c) * a.
const FLATTEN: &str = "format=gbrap,negate,premultiply=inplace=1,negate,format=rgb24";

fn ffmpeg() -> Command {
    let mut c = Command::new("ffmpeg");
    c.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
    c
}

async fn run(mut c: Command) -> Result<Vec<u8>> {
    let out = c.stdin(Stdio::null()).kill_on_drop(true).output().await?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    Err(Error::Ffmpeg(
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

async fn ffprobe(input: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut c = Command::new("ffprobe");
    c.args(["-v", "error"]).args(args).arg(input);
    run(c).await.map_err(|e| match e {
        Error::Ffmpeg(m) => Error::Unsupported(m),
        e => e,
    })
}

pub async fn probe(input: &Path) -> Result<Probe> {
    let out = ffprobe(
        input,
        &[
            "-count_packets",
            "-show_entries",
            "format=duration:stream=codec_type,codec_name,width,height,nb_read_packets:stream_disposition=attached_pic:stream_side_data=rotation",
            "-of",
            "json",
        ],
    )
    .await?;
    let j: Value = serde_json::from_slice(&out).map_err(|e| Error::Unsupported(e.to_string()))?;
    let streams = j["streams"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let v = streams
        .iter()
        .find(|s| {
            s["codec_type"] == "video"
                && s["width"].as_u64().unwrap_or(0) > 0
                && s["disposition"]["attached_pic"] != 1
        })
        .ok_or_else(|| Error::Unsupported("no video stream".into()))?;
    let frames: u64 = v["nb_read_packets"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let animated_image = matches!(v["codec_name"].as_str(), Some("gif" | "apng" | "webp_anim"));
    let mut duration = j["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok());
    if duration.is_none() && frames > 1 {
        // Animated WebP has no container duration.
        duration = packets(input)
            .await?
            .iter()
            .map(|(p, d)| p + d)
            .reduce(f64::max);
    }
    let kind = if has_audio {
        Kind::Video
    } else if frames <= 1 {
        Kind::Image
    } else if animated_image || duration.is_some_and(|d| d <= 20.0) {
        Kind::Gif
    } else {
        Kind::Video
    };
    let (mut width, mut height) = (
        v["width"].as_u64().unwrap_or(0) as u32,
        v["height"].as_u64().unwrap_or(0) as u32,
    );
    if v["side_data_list"][0]["rotation"]
        .as_i64()
        .unwrap_or(0)
        .abs()
        % 180
        == 90
    {
        (width, height) = (height, width);
    }
    Ok(Probe {
        kind,
        width,
        height,
        duration_ms: duration
            .filter(|_| kind != Kind::Image)
            .map(|d| (d * 1000.0).round() as u64),
        has_audio,
    })
}

// (pts, duration) of every video packet, in seconds.
async fn packets(input: &Path) -> Result<Vec<(f64, f64)>> {
    let out = ffprobe(
        input,
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,duration_time",
            "-of",
            "csv=p=0",
        ],
    )
    .await?;
    let mut p: Vec<_> = String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|l| {
            let (pts, dur) = l.split_once(',')?;
            Some((pts.trim().parse().ok()?, dur.trim().parse().unwrap_or(0.0)))
        })
        .collect();
    p.sort_by(|a: &(f64, f64), b| a.0.total_cmp(&b.0));
    Ok(p)
}

async fn frame_count(input: &Path) -> Result<u64> {
    let out = ffprobe(
        input,
        &[
            "-count_packets",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_packets",
            "-of",
            "csv=p=0",
        ],
    )
    .await?;
    Ok(String::from_utf8_lossy(&out).trim().parse().unwrap_or(0))
}

pub async fn normalize(input: &Path, out_dir: &Path, id: &str) -> Result<Stored> {
    let p = probe(input).await?;
    let (ext, mime) = match p.kind {
        Kind::Image | Kind::Gif => ("webp", "image/webp"),
        Kind::Video => ("mp4", "video/mp4"),
    };
    let path = out_dir.join(format!("{id}.{ext}"));
    let mut c = ffmpeg();
    c.arg("-i").arg(input).args(["-map_metadata", "-1"]);
    match p.kind {
        Kind::Image => c.args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(4096,iw)':'min(4096,ih)':force_original_aspect_ratio=decrease",
            "-c:v",
            "libwebp",
            "-quality",
            "80",
        ]),
        Kind::Gif => {
            let d = p.duration_ms.unwrap_or(0);
            let fast = d > 0 && frame_count(input).await? * 1000 > 31 * d;
            let vf = if fast {
                "scale='min(720,iw)':-1,fps=30"
            } else {
                "scale='min(720,iw)':-1"
            };
            c.args([
                "-an",
                "-vf",
                vf,
                "-c:v",
                "libwebp_anim",
                "-quality",
                "75",
                "-loop",
                "0",
            ])
        }
        Kind::Video => {
            c.args([
                "-map",
                "0:v:0",
                "-map",
                "0:a:0?",
                "-vf",
                "scale=w='if(gte(iw,ih),min(1920,iw),min(1080,iw))':h='if(gte(iw,ih),min(1080,ih),min(1920,ih))':force_original_aspect_ratio=decrease:force_divisible_by=2",
                "-c:v",
                "libsvtav1",
                "-preset",
                "8",
                "-crf",
                "38",
                "-pix_fmt",
                "yuv420p",
            ]);
            if p.has_audio {
                c.args(["-c:a", "libopus", "-b:a", "64k", "-ac", "2"]);
            }
            c.args(["-movflags", "+faststart"])
        }
    };
    c.arg(&path);
    if let Err(e) = run(c).await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(e);
    }
    let size = tokio::fs::metadata(&path).await?.len();
    let probe = probe(&path).await?;
    Ok(Stored {
        path,
        ext,
        mime,
        probe,
        size,
    })
}

/// Where to take a still from: the start, unless the first frame is black (a fade-in), then 3 s
/// in, or halfway through clips shorter than 3 s. Returns a seek time, or for animated WebP
/// (which can't seek) a frame index.
async fn still_at(path: &Path) -> (Option<u64>, Option<u64>) {
    let Ok(p) = probe(path).await else {
        return (None, None);
    };
    let Some(d) = p.duration_ms.filter(|d| *d > 0 && p.kind != Kind::Image) else {
        return (None, None);
    };
    let Ok(px) = gray32(path, None, None).await else {
        return (None, None);
    };
    let max = px.iter().copied().max().unwrap_or(0);
    let mean = px.iter().map(|&v| v as u32).sum::<u32>() / px.len().max(1) as u32;
    if mean >= 16 || max >= 40 {
        return (None, None);
    }
    let ms = if d > 3000 { 3000 } else { d / 2 };
    if p.kind == Kind::Gif {
        let starts = packets(path).await.unwrap_or_default();
        let n = starts.partition_point(|&(t, _)| t * 1000.0 <= ms as f64 + 5.0);
        return (None, Some(n.saturating_sub(1) as u64));
    }
    (Some(ms), None)
}

fn seek(c: &mut Command, at_ms: Option<u64>) {
    if let Some(ms) = at_ms {
        c.args(["-ss", &format!("{}.{:03}", ms / 1000, ms % 1000)]);
    }
}

fn select(frame: Option<u64>) -> String {
    frame
        .map(|n| format!("select='eq(n,{n})',"))
        .unwrap_or_default()
}

pub async fn thumbnail(stored: &Path, out: &Path) -> Result<()> {
    let mut c = ffmpeg();
    let (ms, frame) = still_at(stored).await;
    seek(&mut c, ms);
    c.arg("-i").arg(stored);
    let vf = format!("{}scale='min(320,iw)':-1", select(frame));
    c.args([
        "-frames:v",
        "1",
        "-vf",
        &vf,
        "-c:v",
        "libwebp",
        "-quality",
        "70",
    ]);
    c.arg(out);
    run(c).await.map(drop)
}

pub async fn frame_png(stored: &Path) -> Result<Vec<u8>> {
    let mut c = ffmpeg();
    let (ms, frame) = still_at(stored).await;
    seek(&mut c, ms);
    c.arg("-i").arg(stored);
    let vf = format!("{}{FLATTEN}", select(frame));
    c.args([
        "-frames:v",
        "1",
        "-vf",
        &vf,
        "-c:v",
        "png",
        "-f",
        "image2pipe",
        "-",
    ]);
    run(c).await
}

pub async fn telegram_copy(stored: &Stored, out_dir: &Path) -> Result<Option<PathBuf>> {
    if stored.probe.kind == Kind::Image {
        return Ok(None);
    }
    let stem = stored
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let out = out_dir.join(format!("{stem}.tg.mp4"));
    let gif = stored.probe.kind == Kind::Gif;
    let even = "scale=trunc(iw/2)*2:trunc(ih/2)*2";
    let vf = if gif {
        format!("{FLATTEN},{even}")
    } else {
        even.to_string()
    };
    let mut c = ffmpeg();
    c.arg("-i").arg(&stored.path);
    c.args(["-map_metadata", "-1", "-map", "0:v:0", "-vf", &vf]);
    c.args([
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p",
    ]);
    if !gif && stored.probe.has_audio {
        c.args(["-map", "0:a:0", "-c:a", "aac", "-b:a", "128k"]);
    } else {
        c.arg("-an");
    }
    c.args(["-movflags", "+faststart"]).arg(&out);
    if let Err(e) = run(c).await {
        let _ = tokio::fs::remove_file(&out).await;
        return Err(e);
    }
    Ok(Some(out))
}

pub async fn phash(path: &Path) -> Result<Vec<u64>> {
    let p = probe(path).await?;
    let Some(d) = p.duration_ms.filter(|_| p.kind != Kind::Image) else {
        return Ok(vec![dct_hash(&gray32(path, None, None).await?)]);
    };
    // Animated WebP can't seek: pick the frame shown at that time by index (5 ms slack for ms rounding).
    let starts: Vec<f64> = match p.kind {
        Kind::Gif => packets(path).await?.into_iter().map(|(p, _)| p).collect(),
        _ => vec![],
    };
    let at = |pct: u64| {
        let ms = d * pct / 100;
        match starts.is_empty() {
            true => (Some(ms), None),
            false => {
                let n = starts.partition_point(|&t| t * 1000.0 <= ms as f64 + 5.0);
                (None, Some(n.saturating_sub(1) as u64))
            }
        }
    };
    let mut hashes = vec![];
    for pct in [10, 50, 90] {
        let (ms, frame) = at(pct);
        let px = gray32(path, ms, frame).await?;
        if !uniform(&px) {
            hashes.push(dct_hash(&px));
        }
    }
    if hashes.is_empty() {
        let (ms, frame) = at(50);
        hashes.push(dct_hash(&gray32(path, ms, frame).await?));
    }
    Ok(hashes)
}

async fn gray32(path: &Path, at_ms: Option<u64>, frame: Option<u64>) -> Result<Vec<u8>> {
    let mut c = ffmpeg();
    if let Some(ms) = at_ms {
        c.args(["-ss", &format!("{}.{:03}", ms / 1000, ms % 1000)]);
    }
    c.arg("-i").arg(path);
    let vf = format!(
        "{}{FLATTEN},scale=32:32:flags=area,format=gray",
        select(frame)
    );
    c.args(["-frames:v", "1", "-vf", &vf, "-f", "rawvideo", "-"]);
    let px = run(c).await?;
    if px.len() != 1024 {
        return Err(Error::Ffmpeg(format!(
            "expected a 32x32 frame, got {} bytes",
            px.len()
        )));
    }
    Ok(px)
}

fn uniform(px: &[u8]) -> bool {
    let mean = px.iter().map(|&p| p as f64).sum::<f64>() / px.len() as f64;
    let var = px.iter().map(|&p| (p as f64 - mean).powi(2)).sum::<f64>() / px.len() as f64;
    var.sqrt() < 5.0
}

fn dct_hash(px: &[u8]) -> u64 {
    let cos =
        |k: usize, n: usize| ((2 * n + 1) as f64 * k as f64 * std::f64::consts::PI / 64.0).cos();
    let mut rows = [[0f64; 8]; 32];
    for (y, row) in rows.iter_mut().enumerate() {
        for (u, r) in row.iter_mut().enumerate() {
            *r = (0..32).map(|x| px[y * 32 + x] as f64 * cos(u, x)).sum();
        }
    }
    let mut d = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            d[v * 8 + u] = (0..32).map(|y| rows[y][u] * cos(v, y)).sum();
        }
    }
    let mut ac = d[1..].to_vec();
    ac.sort_by(f64::total_cmp);
    let median = ac[31];
    d.iter()
        .enumerate()
        .fold(0, |h, (i, &x)| if x > median { h | 1 << i } else { h })
}

pub fn distance(a: &[u64], b: &[u64]) -> u32 {
    if a.is_empty() || b.is_empty() {
        return 64;
    }
    let dir = |x: &[u64], y: &[u64]| {
        x.iter()
            .map(|h| y.iter().map(|g| (h ^ g).count_ones()).min().unwrap())
            .max()
            .unwrap()
    };
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Less => dir(a, b),
        std::cmp::Ordering::Greater => dir(b, a),
        std::cmp::Ordering::Equal => dir(a, b).max(dir(b, a)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_across_lengths() {
        assert_eq!(distance(&[0b1011], &[u64::MAX, 0b1001, 0]), 1);
        assert_eq!(distance(&[0, 0, 0], &[0, 0, u64::MAX]), 64);
        assert_eq!(distance(&[], &[0]), 64);
    }

    #[test]
    fn dct_hash_sees_structure() {
        let grad: Vec<u8> = (0..1024).map(|i| (i % 32 * 8) as u8).collect();
        let flipped: Vec<u8> = grad.iter().map(|p| 255 - p).collect();
        assert!((dct_hash(&grad) ^ dct_hash(&flipped)).count_ones() > 6);
        assert!(uniform(&[0; 1024]) && !uniform(&grad));
    }
}
