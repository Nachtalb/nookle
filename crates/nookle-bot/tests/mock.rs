//! API client against the Python mock (`mock/server.py`), one mock per test.

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use nookle_bot::api::{Api, Upload};
use nookle_core::model::{DupMatch, ImportRequest, ImportStatus, Status};

const RW: &str = "nk_rwtestrwtestrwtestrwtestrwtestrw";

struct Mock(Child, String);

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn mock() -> Mock {
    let mut child = Command::new("python3")
        .arg("-u")
        .arg(root().join("mock/server.py"))
        .args(["--port", "0"])
        .env("MOCK_QUIET", "1")
        .env("MOCK_PROCESS_DELAY", "0.3")
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line
        .split_whitespace()
        .find(|w| w.starts_with("http://"))
        .expect("mock banner")
        .to_string();
    Mock(child, url)
}

fn api(m: &Mock) -> Api {
    Api::new(&m.1, RW)
}

fn sample(name: &str) -> PathBuf {
    root().join("samples/inputs").join(name)
}

#[tokio::test]
async fn search_pages_through_cursor() {
    let m = mock();
    let api = api(&m);
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = api.search("", cursor.as_deref(), 3).await.unwrap();
        assert!(page.items.len() <= 3);
        ids.extend(page.items.iter().map(|i| i.id.clone()));
        let next = nookle_bot::text::next_offset(page.next_cursor);
        if next.is_empty() {
            break;
        }
        cursor = Some(next);
    }
    assert_eq!(ids.len(), 7);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 7);

    let page = api.search("dance", None, 50).await.unwrap();
    assert_eq!(page.items[0].id, "dAnCe123");
    let results: Vec<_> = page
        .items
        .iter()
        .filter_map(nookle_bot::text::inline_result)
        .collect();
    assert_eq!(results.len(), page.items.len());
}

#[tokio::test]
async fn upload_then_exact_duplicate() {
    let m = mock();
    let api = api(&m);
    let sources = vec!["https://t.me/memes/42".to_string()];
    let tags = vec!["cat".to_string(), "lol".to_string()];
    let Upload::Created(item) = api
        .upload(&sample("x_photo.jpg"), "photo.jpg", &sources, &tags, false)
        .await
        .unwrap()
    else {
        panic!("expected created")
    };
    assert_eq!(item.status, Status::Processing);
    assert_eq!(item.tags, tags);
    assert_eq!(item.sources[0].url, sources[0]);

    let mut ready = (*item).clone();
    for _ in 0..50 {
        ready = api.item(&item.id).await.unwrap();
        if ready.status == Status::Ready {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(ready.status, Status::Ready);
    assert!(ready.telegram.is_some());

    match api
        .upload(&sample("x_photo.jpg"), "photo.jpg", &[], &[], true)
        .await
        .unwrap()
    {
        Upload::Duplicate {
            dup_match: DupMatch::Exact,
            existing,
            ..
        } => assert_eq!(existing.id, item.id),
        other => panic!("expected exact duplicate, got {other:?}"),
    }
}

#[tokio::test]
async fn near_duplicate_needs_force() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipped: the mock needs ffmpeg for near duplicates");
        return;
    }
    let m = mock();
    let api = api(&m);
    let file = sample("build_green_near_dup.jpg");
    match api
        .upload(&file, "near.jpg", &[], &[], false)
        .await
        .unwrap()
    {
        Upload::Duplicate {
            dup_match: DupMatch::Near,
            distance,
            existing,
        } => {
            assert_eq!(existing.id, "bUiLdGrn");
            assert!(distance.is_some());
        }
        other => panic!("expected near duplicate, got {other:?}"),
    }
    assert!(matches!(
        api.upload(&file, "near.jpg", &[], &[], true).await.unwrap(),
        Upload::Created(_)
    ));
}

#[tokio::test]
async fn import_reports_created_and_duplicates() {
    let m = mock();
    let api = api(&m);
    let req = ImportRequest {
        url: "https://x.com/nachtalb/status/1000000000000000001".into(),
        tags: vec!["imported".into()],
        ..Default::default()
    };
    let res = api.import(&req).await.unwrap();
    let statuses: Vec<_> = res.results.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        [
            ImportStatus::Created,
            ImportStatus::Created,
            ImportStatus::Duplicate
        ]
    );
    let summary = nookle_bot::text::import_summary(&res);
    assert_eq!(summary.lines().count(), 3);
    assert!(summary.contains("already there"));

    let err = api
        .import(&ImportRequest {
            url: "https://example.com/nope".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("mock only resolves"), "{err}");
}

#[tokio::test]
async fn hit_counts_and_unknown_fails() {
    let m = mock();
    let api = api(&m);
    let before = api.item("pH0t0aaa").await.unwrap().hits;
    api.hit("pH0t0aaa").await.unwrap();
    let after = api.item("pH0t0aaa").await.unwrap();
    assert_eq!(after.hits, before + 1);
    assert!(after.last_used_at.is_some());
    assert!(api.hit("nOsUcHiD").await.is_err());
}
