//! Resolves third-party URLs (x.com via fxtwitter, direct links) and downloads their media safely.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use nookle_core::model::{Kind, Site, Source};
use reqwest::{Response, redirect::Policy};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use url::{Host, Url};

#[cfg(test)]
mod tests;

const MAX_REDIRECTS: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(60);
const X_HOSTS: &[&str] = &[
    "x.com",
    "twitter.com",
    "fxtwitter.com",
    "vxtwitter.com",
    "fixupx.com",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported url: {0}")]
    Unsupported(String),
    #[error("blocked: {0}")]
    Blocked(String),
    #[error("upstream: {0}")]
    Upstream(String),
    #[error("file too large")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Config {
    pub fxtwitter_base: String,
    /// Disables the SSRF guard. Tests only.
    pub allow_private: bool,
    pub user_agent: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fxtwitter_base: "https://api.fxtwitter.com".into(),
            allow_private: false,
            user_agent: "Mozilla/5.0 (compatible; nookle)".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub source: Source,
    pub media: Vec<RemoteMedia>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMedia {
    pub url: String,
    pub kind: Option<Kind>,
}

pub struct Fetcher {
    cfg: Config,
    allow: fn(IpAddr) -> bool,
}

impl Fetcher {
    pub fn new(cfg: Config) -> Self {
        let allow = if cfg.allow_private {
            |_| true
        } else {
            is_public
        };
        Self { cfg, allow }
    }

    pub async fn resolve(&self, url: &str) -> Result<Resolved> {
        let parsed = parse(url)?;
        let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
        let bare = host
            .strip_prefix("www.")
            .or_else(|| host.strip_prefix("mobile."))
            .unwrap_or(&host);
        if X_HOSTS.contains(&bare) {
            let segs: Vec<&str> = parsed
                .path_segments()
                .map(|s| s.collect())
                .unwrap_or_default();
            return match segs.as_slice() {
                [user, "status", id, ..]
                    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    self.resolve_x(user, id).await
                }
                _ => Err(Error::Unsupported(format!("not a post url: {url}"))),
            };
        }
        let site = if bare == "t.me" {
            Site::Telegram
        } else {
            Site::Web
        };
        Ok(Resolved {
            source: Source {
                url: url.into(),
                site,
                author: None,
            },
            media: vec![RemoteMedia {
                url: url.into(),
                kind: None,
            }],
        })
    }

    async fn resolve_x(&self, user: &str, id: &str) -> Result<Resolved> {
        let api = format!(
            "{}/{user}/status/{id}",
            self.cfg.fxtwitter_base.trim_end_matches('/')
        );
        let body = tokio::time::timeout(TIMEOUT, async {
            let resp = self.get(&api).await?;
            resp.bytes()
                .await
                .map_err(|e| Error::Upstream(format!("fxtwitter: {e}")))
        })
        .await
        .map_err(|_| Error::Upstream("fxtwitter: timeout".into()))??;
        let fx: FxResponse = serde_json::from_slice(&body)
            .map_err(|e| Error::Upstream(format!("fxtwitter: bad response: {e}")))?;
        let tweet = match (fx.code, fx.tweet) {
            (200, Some(t)) => t,
            (code, _) => {
                return Err(Error::Upstream(format!(
                    "fxtwitter {code}: {}",
                    fx.message.unwrap_or_default()
                )));
            }
        };
        let media: Vec<RemoteMedia> = tweet
            .media
            .map(|m| m.all)
            .unwrap_or_default()
            .into_iter()
            .map(FxMedia::into_remote)
            .collect();
        if media.is_empty() {
            return Err(Error::Unsupported(format!(
                "post has no media: {}",
                tweet.url
            )));
        }
        Ok(Resolved {
            source: Source {
                url: tweet.url,
                site: Site::X,
                author: Some(tweet.author.screen_name),
            },
            media,
        })
    }

    pub async fn download(&self, url: &str, dest: &Path, max_bytes: u64) -> Result<u64> {
        let res = match tokio::time::timeout(TIMEOUT, self.stream_to(url, dest, max_bytes)).await {
            Ok(r) => r,
            Err(_) => Err(Error::Upstream(format!("{url}: timeout"))),
        };
        if res.is_err() {
            let _ = tokio::fs::remove_file(dest).await;
        }
        res
    }

    async fn stream_to(&self, url: &str, dest: &Path, max_bytes: u64) -> Result<u64> {
        let mut resp = self.get(url).await?;
        if !resp.status().is_success() {
            return Err(Error::Upstream(format!("{url}: HTTP {}", resp.status())));
        }
        if resp.content_length().is_some_and(|n| n > max_bytes) {
            return Err(Error::TooLarge);
        }
        let mut file = tokio::fs::File::create(dest).await?;
        let mut size = 0u64;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| Error::Upstream(format!("{url}: {e}")))?
        {
            size += chunk.len() as u64;
            if size > max_bytes {
                return Err(Error::TooLarge);
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(size)
    }

    /// GET with manual redirects; every hop's host is resolved, checked and pinned.
    async fn get(&self, url: &str) -> Result<Response> {
        let mut url = parse(url)?;
        for _ in 0..=MAX_REDIRECTS {
            let mut builder = reqwest::Client::builder()
                .redirect(Policy::none())
                .user_agent(&self.cfg.user_agent)
                .connect_timeout(Duration::from_secs(10));
            match url.host() {
                Some(Host::Ipv4(ip)) => self.check(ip.into())?,
                Some(Host::Ipv6(ip)) => self.check(ip.into())?,
                Some(Host::Domain(d)) => {
                    let port = url.port_or_known_default().unwrap_or(80);
                    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((d, port))
                        .await
                        .map_err(|e| Error::Upstream(format!("{d}: {e}")))?
                        .collect();
                    let first = *addrs
                        .first()
                        .ok_or_else(|| Error::Upstream(format!("{d}: no address")))?;
                    for a in &addrs {
                        self.check(a.ip())?;
                    }
                    builder = builder.resolve(d, first);
                }
                None => return Err(Error::Unsupported(url.to_string())),
            }
            let client = builder
                .build()
                .map_err(|e| Error::Upstream(e.to_string()))?;
            let resp = client
                .get(url.clone())
                .send()
                .await
                .map_err(|e| Error::Upstream(format!("{url}: {e}")))?;
            if !resp.status().is_redirection() {
                return Ok(resp);
            }
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| Error::Upstream(format!("{url}: redirect without location")))?;
            let next = url
                .join(loc)
                .map_err(|e| Error::Upstream(format!("{url}: bad redirect: {e}")))?;
            url = parse(next.as_str())?;
        }
        Err(Error::Upstream("too many redirects".into()))
    }

    fn check(&self, ip: IpAddr) -> Result<()> {
        if (self.allow)(ip) {
            Ok(())
        } else {
            Err(Error::Blocked(format!("non-public address {ip}")))
        }
    }
}

fn parse(url: &str) -> Result<Url> {
    let u = Url::parse(url).map_err(|e| Error::Unsupported(format!("{url}: {e}")))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(Error::Unsupported(url.into()));
    }
    Ok(u)
}

pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (18..20).contains(&b)))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    // IPv4-compatible ::a.b.c.d (incl. :: and ::1) and NAT64 64:ff9b::/96 embed an IPv4 address.
    if s[..6] == [0; 6] || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let [.., a, b, c, d] = ip.octets();
        return is_public_v4(Ipv4Addr::new(a, b, c, d));
    }
    !(ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] == 0x2001 && s[1] == 0x0db8))
}

#[derive(Deserialize)]
struct FxResponse {
    code: u16,
    message: Option<String>,
    tweet: Option<FxTweet>,
}

#[derive(Deserialize)]
struct FxTweet {
    url: String,
    author: FxAuthor,
    media: Option<FxMediaList>,
}

#[derive(Deserialize)]
struct FxAuthor {
    screen_name: String,
}

#[derive(Deserialize)]
struct FxMediaList {
    #[serde(default)]
    all: Vec<FxMedia>,
}

#[derive(Deserialize)]
struct FxMedia {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    #[serde(default)]
    variants: Vec<FxVariant>,
}

#[derive(Deserialize)]
struct FxVariant {
    url: String,
    content_type: Option<String>,
    bitrate: Option<u64>,
}

impl FxMedia {
    fn into_remote(self) -> RemoteMedia {
        let kind = match self.kind.as_str() {
            "photo" => Some(Kind::Image),
            "video" => Some(Kind::Video),
            "gif" => Some(Kind::Gif),
            _ => None,
        };
        let best_mp4 = self
            .variants
            .into_iter()
            .filter(|v| v.content_type.as_deref() == Some("video/mp4"))
            .max_by_key(|v| v.bitrate.unwrap_or(0))
            .map(|v| v.url);
        let url = match kind {
            Some(Kind::Video | Kind::Gif) => best_mp4.unwrap_or(self.url),
            _ => self.url,
        };
        RemoteMedia { url, kind }
    }
}
