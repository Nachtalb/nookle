use nookle_core::model::Kind;
use nookle_media::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn has_ffmpeg() -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-demuxers"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("webp_anim"))
}

fn input(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples/inputs")
        .join(name)
}

fn codecs(path: &Path) -> String {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(",")
}

fn first_frame_rgba(path: &Path) -> Vec<u8> {
    std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
        .output()
        .unwrap()
        .stdout
}

#[tokio::test]
async fn samples() {
    if !has_ffmpeg() {
        eprintln!("skip: ffmpeg >= 8 with animated WebP support not found");
        return;
    }
    let out = std::env::temp_dir().join(format!("nookle-media-test-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let cases = [
        ("build_green.png", Kind::Image, "webp"),
        ("build_green_near_dup.jpg", Kind::Image, "webp"),
        ("photo.jpg", Kind::Image, "webp"),
        ("x_photo.jpg", Kind::Image, "webp"),
        ("transparent.png", Kind::Image, "webp"),
        ("dance.gif", Kind::Gif, "webp_anim"),
        ("silent_short.mp4", Kind::Gif, "webp_anim"),
        ("silent_long.mp4", Kind::Video, "av1"),
        ("with_sound.mp4", Kind::Video, "av1,opus"),
        ("x_video.mp4", Kind::Video, "av1,opus"),
    ];
    let mut stored = std::collections::HashMap::new();
    for (i, (name, kind, codec)) in cases.into_iter().enumerate() {
        let t = Instant::now();
        let s = normalize(&input(name), &out, &format!("id{i}"))
            .await
            .unwrap();
        eprintln!(
            "{name}: {:?} {} bytes in {:?} {:?}",
            s.probe.kind,
            s.size,
            t.elapsed(),
            s.probe
        );
        assert_eq!(s.probe.kind, kind, "{name}");
        assert_eq!(probe(&input(name)).await.unwrap().kind, kind, "{name}");
        assert_eq!(codecs(&s.path), codec, "{name}");
        assert_eq!(s.path, out.join(format!("id{i}.{}", s.ext)));
        let thumb = out.join(format!("id{i}.thumb.webp"));
        thumbnail(&s.path, &thumb).await.unwrap();
        assert_eq!(codecs(&thumb), "webp");
        assert!(frame_png(&s.path).await.unwrap().starts_with(b"\x89PNG"));
        stored.insert(name, s);
    }

    let dance = &stored["dance.gif"].probe;
    assert_eq!((dance.width, dance.height), (320, 240));
    assert!(dance.duration_ms.unwrap().abs_diff(2000) < 50);
    assert!(stored["with_sound.mp4"].probe.has_audio);
    assert!(!stored["silent_long.mp4"].probe.has_audio);

    let t = &stored["transparent.png"].path;
    assert_eq!(first_frame_rgba(t)[3], 0, "corner stays transparent");
    let png = out.join("t.png");
    std::fs::write(&png, frame_png(t).await.unwrap()).unwrap();
    assert_eq!(
        &first_frame_rgba(&png)[..4],
        &[255, 255, 255, 255],
        "flattened onto white"
    );

    let h = |n: &'static str| {
        let p = stored[n].path.clone();
        async move { phash(&p).await.unwrap() }
    };
    let green = h("build_green.png").await;
    let near = distance(&green, &h("build_green_near_dup.jpg").await);
    let other = distance(&green, &h("x_photo.jpg").await);
    let input_vs_stored = distance(
        &phash(&input("dance.gif")).await.unwrap(),
        &h("dance.gif").await,
    );
    let gif = h("silent_short.mp4").await;
    eprintln!(
        "near_dup={near} x_photo={other} dance={input_vs_stored} silent_short hashes={}",
        gif.len()
    );
    assert!(near <= 6 && other > 6 && input_vs_stored <= 6);
    assert_eq!(gif.len(), 3);
    assert!(distance(&gif, &h("silent_long.mp4").await) > 6);

    let intro = out.join("intro.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("color=c=black:s=320x240:r=10:d=3[b];testsrc=s=320x240:r=10:d=3[t];[b][t]concat=n=2:v=1")
        .args(["-c:v", "libx264"])
        .arg(&intro)
        .status()
        .unwrap();
    assert!(ok.success());
    let s = normalize(&intro, &out, "intro").await.unwrap();
    assert_eq!(
        phash(&s.path).await.unwrap().len(),
        2,
        "black frame skipped"
    );
    // stills skip a black first frame: 3 s in here, halfway in clips under 3 s
    let probe_png = out.join("still.png");
    let bright = |png: Vec<u8>| {
        std::fs::write(&probe_png, png).unwrap();
        let img = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&probe_png)
            .args(["-vf", "scale=8:8,format=gray", "-f", "rawvideo", "-"])
            .output()
            .unwrap()
            .stdout;
        img.iter().map(|&v| v as u32).sum::<u32>() / img.len().max(1) as u32
    };
    assert!(
        bright(frame_png(&s.path).await.unwrap()) > 30,
        "frame after the fade"
    );
    let th = out.join("intro_thumb.webp");
    thumbnail(&s.path, &th).await.unwrap();
    assert!(
        bright(std::fs::read(&th).unwrap()) > 30,
        "thumb after the fade"
    );
    let short = out.join("short_intro.mp4");
    assert!(std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("color=c=black:s=320x240:r=10:d=0.5[b];testsrc=s=320x240:r=10:d=1.5[t];[b][t]concat=n=2:v=1")
        .args(["-c:v", "libx264"])
        .arg(&short)
        .status()
        .unwrap()
        .success());
    assert!(
        bright(frame_png(&short).await.unwrap()) > 30,
        "halfway in a 2 s clip"
    );

    // bytes after the end of a PNG or JPEG are a still image, not a second frame
    for name in ["photo.jpg", "trailing.png"] {
        let src = out.join(format!("junk_{name}"));
        let mut bytes = match name {
            "photo.jpg" => std::fs::read(input(name)).unwrap(),
            _ => {
                std::process::Command::new("ffmpeg")
                    .args([
                        "-v",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        "testsrc=s=64x48:d=1",
                        "-frames:v",
                        "1",
                    ])
                    .args(["-f", "image2pipe", "-c:v", "png", "-"])
                    .output()
                    .unwrap()
                    .stdout
            }
        };
        bytes.extend_from_slice(&[0x13, 0x37, 0, 0, 0, 0, 0xff, 0xfe]);
        std::fs::write(&src, bytes).unwrap();
        assert_eq!(probe(&src).await.unwrap().kind, Kind::Image, "{name}");
        assert_eq!(phash(&src).await.unwrap().len(), 1, "{name}");
    }

    for (name, want) in [
        ("dance.gif", Some("h264")),
        ("silent_short.mp4", Some("h264")),
        ("silent_long.mp4", Some("h264")),
        ("with_sound.mp4", Some("h264,aac")),
        ("photo.jpg", None),
    ] {
        let t = Instant::now();
        let tg = telegram_copy(&stored[name], &out).await.unwrap();
        eprintln!(
            "telegram {name}: {:?} in {:?}",
            tg.as_ref().map(|p| std::fs::metadata(p).unwrap().len()),
            t.elapsed()
        );
        assert_eq!(tg.as_deref().map(codecs).as_deref(), want, "{name}");
    }
    std::fs::remove_dir_all(&out).unwrap();
}
