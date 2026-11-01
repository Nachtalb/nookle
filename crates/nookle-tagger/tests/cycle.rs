//! One claim → analyse → submit cycle against the Python mock and a fake VLM.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nookle_core::model::{Analysis, Item, Style};

const KEY: &str = "nk_rwtestrwtestrwtestrwtestrwtestrw";

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Answers `/health` and every chat completion with a fixed analysis; records the request bodies.
fn fake_vlm(bodies: Arc<Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut line, mut len) = (String::new(), 0);
            reader.read_line(&mut line).unwrap();
            let request = line.clone();
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let reply = if request.starts_with("GET /health") {
                r#"{"status":"ok"}"#.to_string()
            } else {
                bodies
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(body).unwrap());
                let answer = r#"{"text":"HELLO","style":"photo","tags":[" Cat","KEYBOARD","cat","funny","office"]}"#;
                serde_json::json!({"choices": [{"message": {"content": answer}}]}).to_string()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    url
}

#[tokio::test]
async fn claim_analyse_submit() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        eprintln!("skipped: the mock needs ffmpeg for analysis frames");
        return;
    }
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let mut mock = Command::new("python3")
        .args(["-u", "mock/server.py", "--port", "0"])
        .current_dir(root)
        .env("MOCK_QUIET", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(mock.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let _mock = Kill(mock);
    let api = first.split_whitespace().nth(3).unwrap().to_string();

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let vlm = fake_vlm(bodies.clone());
    let _tagger = Kill(
        Command::new(env!("CARGO_BIN_EXE_nookle-tagger"))
            .env("NOOKLE_API", &api)
            .env("NOOKLE_KEY", KEY)
            .env("VLM_URL", &vlm)
            .env("POLL_SECS", "1")
            .spawn()
            .unwrap(),
    );

    let http = reqwest::Client::new();
    let started = Instant::now();
    let item = loop {
        let item: Item = http
            .get(format!("{api}/api/items/sIlEnt25"))
            .bearer_auth(KEY)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if item.analysis != Analysis::Pending {
            break item;
        }
        assert!(started.elapsed() < Duration::from_secs(30), "still pending");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(item.analysis, Analysis::Done);
    assert_eq!(item.style, Some(Style::Photo));
    assert_eq!(item.text, "HELLO");
    for tag in ["cat", "keyboard", "funny", "office"] {
        assert!(
            item.tags.contains(&tag.to_string()),
            "{tag} in {:?}",
            item.tags
        );
    }
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("data:image/png;base64,"));
    assert!(bodies[0].contains("json_schema"));
}
