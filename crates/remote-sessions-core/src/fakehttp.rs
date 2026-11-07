//! A tiny loopback HTTP server for tests: answers from a route table and records every request.
//! Not part of the product API (`#[doc(hidden)]`).
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// `(method, path prefix, status, response body)`: the first matching route answers; routes are not consumed.
pub type Route = (&'static str, String, u16, String);

pub struct Fake {
    pub base: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    routes: Arc<Mutex<Vec<Route>>>,
}

impl Fake {
    pub fn start(routes: Vec<Route>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let routes = Arc::new(Mutex::new(routes));
        let (s2, r2) = (seen.clone(), routes.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, length) = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 { break (buf.len(), 0); }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                        break (pos + 4, len);
                    }
                };
                while buf.len() < head_end + length {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end.min(buf.len())]).to_string();
                let mut lines = head.lines();
                let mut first = lines.next().unwrap_or("").split_whitespace();
                let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
                let body = String::from_utf8_lossy(&buf[head_end.min(buf.len())..]).to_string();
                s2.lock().unwrap().push(Seen { method: method.clone(), path: path.clone(), body, headers });
                let (status, reply) = r2.lock().unwrap().iter().find(|(m, p, _, _)| *m == method && path.starts_with(p.as_str())).map(|(_, _, s, b)| (*s, b.clone())).unwrap_or((404, "{}".into()));
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            }
        });
        Fake { base, seen, routes }
    }
    /// Replace the response of the GET/POST route with this method and prefix.
    pub fn set(&self, method: &'static str, prefix: &str, status: u16, body: &str) {
        let mut routes = self.routes.lock().unwrap();
        routes.retain(|(m, p, _, _)| !(*m == method && p == prefix));
        routes.insert(0, (method, prefix.to_string(), status, body.to_string()));
    }
    pub fn requests(&self) -> Vec<Seen> { self.seen.lock().unwrap().clone() }
}
