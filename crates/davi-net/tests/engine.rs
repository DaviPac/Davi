//! End-to-end tests against a throwaway local HTTP/1.1 server (no internet).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use davi_core::bru::parse_request;
use davi_core::env::VarScope;
use davi_net::{EngineConfig, HttpEngine, NetError};

/// Serves `n` connections. Each response echoes the method, path, cookie
/// header and body as JSON, and sets a session cookie.
fn echo_server(n: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().take(n) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let (mut content_length, mut cookie) = (0usize, String::new());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap();
                match k.to_ascii_lowercase().as_str() {
                    "content-length" => content_length = v.trim().parse().unwrap(),
                    "cookie" => cookie = v.trim().to_owned(),
                    _ => {}
                }
            }
            let mut body = vec![0; content_length];
            reader.read_exact(&mut body).unwrap();
            let mut parts = request_line.split_whitespace();
            let json = serde_json_like(
                parts.next().unwrap(),
                parts.next().unwrap(),
                &cookie,
                &String::from_utf8(body).unwrap(),
            );
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nSet-Cookie: session=abc; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                json.len(),
                json
            )
            .unwrap();
        }
    });
    format!("http://{addr}")
}

fn serde_json_like(method: &str, path: &str, cookie: &str, body: &str) -> String {
    format!(
        "{{\"method\":{:?},\"path\":{:?},\"cookie\":{:?},\"body\":{:?}}}",
        method, path, cookie, body
    )
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    // A *different* runtime than the engine's, standing in for GPUI's executor.
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn executes_with_interpolation_metrics_and_cookies() {
    let base = echo_server(2);
    let engine = HttpEngine::new(EngineConfig::default()).unwrap();
    let vars = VarScope::new().with_layer([("baseUrl", base.as_str()), ("id", "7")]);
    let req = parse_request(
        "post {\n  url: {{baseUrl}}/users/:id?x=1\n  body: json\n  auth: none\n}\n\nparams:path {\n  id: {{id}}\n}\n\nbody:json {\n  {\"id\": {{id}}}\n}\n",
    )
    .unwrap();

    let handle = engine.send(&req, &vars);
    let progress = handle.progress();
    let res = block_on(handle.response()).unwrap();

    assert_eq!(res.status, 201);
    assert_eq!(res.reason, Some("Created"));
    assert!(res.is_json());
    assert_eq!(res.cookies[0].name, "session");
    assert_eq!(res.size.body, res.body.len() as u64);
    assert!(res.size.headers > 0);
    assert!(res.timings.total >= res.timings.ttfb);
    assert_eq!(*progress.borrow(), davi_net::Progress::Done);

    let echoed: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["path"], "/users/7?x=1");
    assert_eq!(echoed["body"], "{\"id\": 7}");
    assert!(res.pretty_body().contains("\n  \"method\": \"POST\""));

    // The shared cookie jar replays the session cookie on the next request.
    let res = block_on(engine.send(&req, &vars).response()).unwrap();
    let echoed: serde_json::Value = serde_json::from_slice(&res.body).unwrap();
    assert_eq!(echoed["cookie"], "session=abc");
}

#[test]
fn truncates_bodies_over_the_cap() {
    let base = echo_server(1);
    let engine = HttpEngine::new(EngineConfig {
        max_body_bytes: 10,
        ..EngineConfig::default()
    })
    .unwrap();
    let req = parse_request(&format!("get {{\n  url: {base}/big\n}}\n")).unwrap();
    let res = block_on(engine.send(&req, &VarScope::new()).response()).unwrap();
    assert!(res.truncated);
    assert_eq!(res.body.len(), 10);
}

#[test]
fn reports_connection_errors_and_cancellation() {
    let engine = HttpEngine::new(EngineConfig::default()).unwrap();
    // Port 9 (discard) on localhost is closed in practice.
    let req = parse_request("get {\n  url: http://127.0.0.1:9/\n}\n").unwrap();
    let err = block_on(engine.send(&req, &VarScope::new()).response()).unwrap_err();
    assert!(matches!(err, NetError::Http(_)), "{err}");

    // A listener that never answers: cancel instead of waiting for a timeout.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let req = parse_request(&format!(
        "get {{\n  url: http://{}/\n}}\n",
        silent.local_addr().unwrap()
    ))
    .unwrap();
    let handle = engine.send(&req, &VarScope::new());
    handle.canceller().cancel();
    assert!(matches!(
        block_on(handle.response()),
        Err(NetError::Cancelled)
    ));
}
