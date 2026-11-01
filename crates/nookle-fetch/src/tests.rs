use super::*;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const FIXTURE: &str = include_str!("../../../samples/fixtures/fxtwitter/1000000000000000001.json");

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples")
}

async fn server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let b = base.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let base = b.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let req = String::from_utf8_lossy(&buf);
                let path = req.split(' ').nth(1).unwrap_or("/").to_string();
                let (head, body) = route(&base, &path);
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
            });
        }
    });
    base
}

fn ok(body: Vec<u8>, with_len: bool) -> (String, Vec<u8>) {
    let len = if with_len {
        format!("Content-Length: {}\r\n", body.len())
    } else {
        String::new()
    };
    (
        format!("HTTP/1.1 200 OK\r\n{len}Connection: close\r\n\r\n"),
        body,
    )
}

fn redirect(to: &str) -> (String, Vec<u8>) {
    (
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
        vec![],
    )
}

fn route(base: &str, path: &str) -> (String, Vec<u8>) {
    let port = base.rsplit(':').next().unwrap();
    match path {
        p if p.ends_with("/status/1000000000000000001") => {
            ok(FIXTURE.replace("sample://", &format!("{base}/")).into(), true)
        }
        "/u/status/2" => ok(
            serde_json::json!({"code": 200, "tweet": {"url": "https://x.com/u/status/2", "author": {"screen_name": "u"},
                "media": {"all": [{"type": "video", "url": "https://v/fallback.mp4", "variants": [
                    {"content_type": "application/x-mpegURL", "url": "https://v/a.m3u8"},
                    {"content_type": "video/mp4", "bitrate": 632000, "url": "https://v/low.mp4"},
                    {"content_type": "video/mp4", "bitrate": 2176000, "url": "https://v/high.mp4"},
                    {"content_type": "video/mp4", "bitrate": 950000, "url": "https://v/mid.mp4"}]}]}}})
            .to_string()
            .into(),
            true,
        ),
        "/u/status/3" => ok(
            br#"{"code":200,"tweet":{"url":"https://x.com/u/status/3","author":{"screen_name":"u"}}}"#.to_vec(),
            true,
        ),
        "/u/status/4" => ok(br#"{"code":404,"message":"NOT_FOUND"}"#.to_vec(), true),
        "/redirect" => redirect("/inputs/x_photo.jpg"),
        "/loop" => redirect("/loop"),
        "/evil" => redirect(&format!("http://127.0.0.2:{port}/inputs/x_photo.jpg")),
        "/ftp" => redirect("ftp://example.com/x"),
        p if p.starts_with("/inputs/") || p.starts_with("/nocl/inputs/") => {
            let file = samples().join(p.trim_start_matches("/nocl").trim_start_matches('/'));
            match std::fs::read(file) {
                Ok(b) => ok(b, !p.starts_with("/nocl")),
                Err(_) => not_found(),
            }
        }
        _ => not_found(),
    }
}

fn not_found() -> (String, Vec<u8>) {
    (
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        vec![],
    )
}

/// Guard on, but loopback (except 127.0.0.2) counts as public so the test server is reachable.
fn guarded(base: &str) -> Fetcher {
    Fetcher {
        cfg: Config {
            fxtwitter_base: base.into(),
            ..Config::default()
        },
        allow: |ip| ip.is_loopback() && ip != IpAddr::from([127, 0, 0, 2]),
    }
}

fn open(base: &str) -> Fetcher {
    Fetcher::new(Config {
        fxtwitter_base: base.into(),
        allow_private: true,
        ..Config::default()
    })
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("nookle-fetch-{}-{name}", std::process::id()))
}

#[tokio::test]
async fn resolves_every_x_url_variant() {
    let base = server().await;
    let f = open(&base);
    for url in [
        "https://x.com/nachtalb/status/1000000000000000001",
        "https://twitter.com/nachtalb/status/1000000000000000001",
        "https://mobile.twitter.com/nachtalb/status/1000000000000000001",
        "https://www.x.com/nachtalb/status/1000000000000000001",
        "https://fxtwitter.com/nachtalb/status/1000000000000000001",
        "https://vxtwitter.com/nachtalb/status/1000000000000000001",
        "https://fixupx.com/nachtalb/status/1000000000000000001",
        "https://X.com/i/status/1000000000000000001",
        "https://x.com/nachtalb/status/1000000000000000001?s=20&t=abc",
        "https://x.com/nachtalb/status/1000000000000000001/photo/1",
    ] {
        let r = f
            .resolve(url)
            .await
            .unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!(
            r.source,
            Source {
                url: "https://x.com/nachtalb/status/1000000000000000001".into(),
                site: Site::X,
                author: Some("nachtalb".into()),
            }
        );
        let got: Vec<_> = r.media.iter().map(|m| (m.url.as_str(), m.kind)).collect();
        assert_eq!(
            got,
            [
                (&*format!("{base}/inputs/x_photo.jpg"), Some(Kind::Image)),
                (&*format!("{base}/inputs/x_video.mp4"), Some(Kind::Video)),
                (&*format!("{base}/inputs/dance.gif"), Some(Kind::Gif)),
            ]
        );
    }
}

#[tokio::test]
async fn picks_best_mp4_and_maps_errors() {
    let base = server().await;
    let f = open(&base);
    let r = f.resolve("https://x.com/u/status/2").await.unwrap();
    assert_eq!(r.media[0].url, "https://v/high.mp4");
    assert!(matches!(
        f.resolve("https://x.com/u/status/3").await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        f.resolve("https://x.com/u/status/4").await,
        Err(Error::Upstream(_))
    ));
    assert!(matches!(
        f.resolve("https://x.com/u/status/5").await,
        Err(Error::Upstream(_))
    ));
    assert!(matches!(
        f.resolve("https://x.com/nachtalb").await,
        Err(Error::Unsupported(_))
    ));
}

#[tokio::test]
async fn resolves_direct_urls() {
    let f = open("http://unused");
    let r = f.resolve("https://example.com/a.png?x=1").await.unwrap();
    assert_eq!(r.source.site, Site::Web);
    assert_eq!(r.source.author, None);
    assert_eq!(
        r.media,
        [RemoteMedia {
            url: "https://example.com/a.png?x=1".into(),
            kind: None
        }]
    );
    let r = f.resolve("https://t.me/somechannel/42").await.unwrap();
    assert_eq!(r.source.site, Site::Telegram);
    for bad in ["ftp://example.com/a.png", "file:///etc/passwd", "not a url"] {
        assert!(
            matches!(f.resolve(bad).await, Err(Error::Unsupported(_))),
            "{bad}"
        );
    }
}

#[tokio::test]
async fn downloads_and_enforces_limits() {
    let base = server().await;
    let f = guarded(&base);
    let want = std::fs::read(samples().join("inputs/x_video.mp4")).unwrap();
    let len = want.len() as u64;

    let dest = tmp("ok");
    let size = f
        .download(&format!("{base}/inputs/x_video.mp4"), &dest, len)
        .await
        .unwrap();
    assert_eq!(size, len);
    assert_eq!(std::fs::read(&dest).unwrap(), want);
    std::fs::remove_file(&dest).unwrap();

    for path in ["/inputs/x_video.mp4", "/nocl/inputs/x_video.mp4"] {
        let dest = tmp("big");
        let res = f.download(&format!("{base}{path}"), &dest, len - 1).await;
        assert!(matches!(res, Err(Error::TooLarge)), "{path}: {res:?}");
        assert!(!dest.exists(), "{path}: partial file left behind");
    }

    let dest = tmp("404");
    let res = f.download(&format!("{base}/inputs/nope"), &dest, len).await;
    assert!(matches!(res, Err(Error::Upstream(_))));
    assert!(!dest.exists());
}

#[tokio::test]
async fn redirects_are_followed_and_rechecked() {
    let base = server().await;
    let f = guarded(&base);
    let dest = tmp("redir");
    let size = f
        .download(&format!("{base}/redirect"), &dest, u64::MAX)
        .await
        .unwrap();
    assert_eq!(size, std::fs::metadata(&dest).unwrap().len());
    std::fs::remove_file(&dest).unwrap();

    let res = f.download(&format!("{base}/evil"), &dest, u64::MAX).await;
    assert!(matches!(res, Err(Error::Blocked(_))), "{res:?}");
    let res = f.download(&format!("{base}/loop"), &dest, u64::MAX).await;
    assert!(matches!(res, Err(Error::Upstream(_))), "{res:?}");
    let res = f.download(&format!("{base}/ftp"), &dest, u64::MAX).await;
    assert!(matches!(res, Err(Error::Unsupported(_))), "{res:?}");
    assert!(!dest.exists());
}

#[tokio::test]
async fn default_guard_blocks_private_hosts() {
    let base = server().await;
    let f = Fetcher::new(Config::default());
    let port = base.rsplit(':').next().unwrap();
    for url in [
        format!("{base}/inputs/x_photo.jpg"),
        format!("http://localhost:{port}/inputs/x_photo.jpg"),
        format!("http://[::1]:{port}/"),
        format!("http://[::ffff:127.0.0.1]:{port}/"),
    ] {
        let res = f.download(&url, &tmp("blocked"), u64::MAX).await;
        assert!(matches!(res, Err(Error::Blocked(_))), "{url}: {res:?}");
    }
}

#[test]
fn ip_classification() {
    for ip in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "100.64.0.1",
        "100.127.255.255",
        "169.254.169.254",
        "224.0.0.1",
        "0.0.0.0",
        "0.1.2.3",
        "255.255.255.255",
        "192.0.2.1",
        "198.51.100.1",
        "203.0.113.1",
        "198.18.0.1",
        "240.0.0.1",
        "::",
        "::1",
        "fc00::1",
        "fd12:3456::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::ffff:169.254.169.254",
        "::127.0.0.1",
        "::192.168.0.1",
        "64:ff9b::a00:1",
    ] {
        assert!(!is_public(ip.parse().unwrap()), "{ip} should be blocked");
    }
    for ip in [
        "1.1.1.1",
        "8.8.8.8",
        "100.63.255.255",
        "100.128.0.1",
        "172.32.0.1",
        "151.101.1.1",
        "2606:4700::1111",
        "2a00:1450::1",
        "::ffff:1.1.1.1",
        "64:ff9b::808:808",
    ] {
        assert!(is_public(ip.parse().unwrap()), "{ip} should be allowed");
    }
}

#[tokio::test]
#[ignore = "hits api.fxtwitter.com and pbs.twimg.com"]
async fn live_myka_rei() {
    let f = Fetcher::new(Config::default());
    let r = f
        .resolve("https://x.com/myka_rei/status/2114513008907161965")
        .await
        .unwrap();
    assert_eq!(r.source.site, Site::X);
    assert_eq!(r.source.author.as_deref(), Some("myka_rei"));
    assert!(!r.media.is_empty());
    assert_eq!(r.media[0].kind, Some(Kind::Image));
    assert!(r.media[0].url.contains("name=orig"), "{}", r.media[0].url);
    let dest = tmp("live");
    let size = f.download(&r.media[0].url, &dest, 50 << 20).await.unwrap();
    assert!(size > 1000);
    std::fs::remove_file(&dest).unwrap();
}
