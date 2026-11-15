//! A scripted adb server on a local port, for tests without adb or a device.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use crate::adb::AdbClient;

/// What the fake server does after answering a request.
pub enum Next {
    /// Read another request on the same connection (after `host:transport:`).
    ReadAnother,
    /// Close the connection.
    Close,
}

type Handler = dyn Fn(&str, &mut TcpStream) -> Next + Send + Sync;

pub struct FakeAdbServer {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeAdbServer {
    /// Starts a server that answers each request with `handler`.
    pub fn start(handler: impl Fn(&str, &mut TcpStream) -> Next + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let thread_requests = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    return;
                };
                let handler = handler.clone();
                let requests = thread_requests.clone();
                std::thread::spawn(move || {
                    while let Some(request) = read_request(&mut stream) {
                        requests.lock().unwrap().push(request.clone());
                        match handler(&request, &mut stream) {
                            Next::ReadAnother => {}
                            Next::Close => return,
                        }
                    }
                });
            }
        });
        Self { port, requests }
    }

    pub fn client(&self) -> AdbClient {
        AdbClient::new(self.port, None)
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).ok()?;
    let length = usize::from_str_radix(std::str::from_utf8(&length).ok()?, 16).ok()?;
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload).ok()?;
    String::from_utf8(payload).ok()
}

pub fn okay(stream: &mut TcpStream) {
    let _ = stream.write_all(b"OKAY");
}

pub fn fail(stream: &mut TcpStream, message: &str) {
    let _ = stream.write_all(format!("FAIL{:04x}{message}", message.len()).as_bytes());
}

/// Writes a length-prefixed payload (after `OKAY` for queries, or as a tracking update).
pub fn length_prefixed(stream: &mut TcpStream, payload: &str) {
    let _ = stream.write_all(format!("{:04x}{payload}", payload.len()).as_bytes());
}

/// Answers `host:transport:<serial>` with `OKAY` for `serial` and `FAIL` for others.
pub fn answer_transport(request: &str, stream: &mut TcpStream, serial: &str) -> Option<Next> {
    let requested = request.strip_prefix("host:transport:")?;
    if requested == serial {
        okay(stream);
        Some(Next::ReadAnother)
    } else {
        fail(stream, &format!("device '{requested}' not found"));
        Some(Next::Close)
    }
}
