#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::disallowed_methods, reason = "These native-only tests bound their local HTTP servers with a monotonic clock.")]
use gridcraft_engine::Session;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

fn preview(s: &mut Session, url: &str) -> Result<serde_json::Value, String> {
    s.run("data.query.preview", json!({"formula":format!("Text.FromBinary(Web.Contents({}))",gridcraft_mquery::quote_text(url))}))
}
fn serve(listener: TcpListener, status: &'static str, extra: String, body: &'static str) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let started = std::time::Instant::now();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut buf = [0; 4096];
                    let mut request = Vec::new();
                    while !request.windows(4).any(|s| s == b"\r\n\r\n") && request.len() < 8192 {
                        let n = stream.read(&mut buf).unwrap();
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buf[..n]);
                    }
                    write!(stream, "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && started.elapsed() < Duration::from_secs(5) => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("test HTTP server: {e}"),
            }
        }
    })
}

#[test]
fn redirects_need_permission_before_contacting_another_site() {
    let first = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", first.local_addr().unwrap());
    let target = format!("http://{}/data", second.local_addr().unwrap());
    let server = serve(first, "302 Found", format!("Location: {target}\r\n"), "");
    let mut s = Session::new();
    s.new_workbook();
    s.run("data.web.approve", json!({"site":base})).unwrap();
    assert!(preview(&mut s, &base).unwrap_err().contains("needs your permission"));
    server.join().unwrap();
    second.set_nonblocking(true).unwrap();
    assert!(second.accept().is_err(), "no request reaches the unapproved target");
    let needs = s.run("data.database.needs", json!({})).unwrap();
    assert_eq!(needs, json!([{"need":"web","site":second.local_addr().unwrap().to_string()}]));
    s.run("data.web.approve", json!({"url":target})).unwrap();
    let server = serve(second, "200 OK", String::new(), "allowed");
    assert_eq!(preview(&mut s, &target).unwrap()["rows"], json!([["allowed"]]));
    server.join().unwrap();
}

#[test]
fn relative_redirects_work_and_timeout_is_honored() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let next = listener.try_clone().unwrap();
    let first = serve(listener, "302 Found", "Location: /final\r\n".into(), "");
    let mut s = Session::new();
    s.new_workbook();
    s.run("data.web.approve", json!({"site":base})).unwrap();
    // One sequential server handles both replies to avoid a race between acceptors.
    let second = thread::spawn(move || {
        first.join().unwrap();
        serve(next, "200 OK", String::new(), "ok").join().unwrap();
    });
    assert_eq!(preview(&mut s, &base).unwrap()["rows"], json!([["ok"]]));
    second.join().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    s.run("data.web.approve", json!({"site":url})).unwrap();
    let formula = format!("Web.Contents({}, [Timeout=#duration(0,0,0,0.05)])", gridcraft_mquery::quote_text(&url));
    let start = std::time::Instant::now();
    assert!(s.run("data.query.preview", json!({"formula":formula})).is_err());
    assert!(start.elapsed() < Duration::from_secs(3), "the requested short timeout applies");
}
