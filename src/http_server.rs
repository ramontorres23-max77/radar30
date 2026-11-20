//! Minimal HTTP transport for remote MCP: `GET /health`, `POST /mcp` (single JSON-RPC
//! request/response), `OPTIONS` preflight. TLS is terminated by the Easypanel proxy.
//!
//! Deliberately dependency-free: std `TcpListener` with one thread per connection,
//! bounded request bodies and socket timeouts. `mcp::handle()` remains the single
//! authoritative protocol implementation shared by stdio and HTTP.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write as IoWrite};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

/// Start the HTTP server on `host:port`, blocking until the process is stopped.
pub fn serve(host: &str, port: u16) -> Result<()> {
    let listener =
        TcpListener::bind((host, port)).with_context(|| format!("binding {host}:{port}"))?;
    eprintln!("radar30 HTTP MCP listening on http://{host}:{port}/mcp (health: /health)");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(|| {
                    if let Err(err) = handle_connection(stream) {
                        eprintln!("[http] connection error: {err:#}");
                    }
                });
            }
            Err(err) => eprintln!("[http] accept error: {err}"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    let _ = reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("content-length")
        {
            content_length = value
                .trim()
                .parse::<usize>()
                .unwrap_or(0)
                .min(MAX_BODY_BYTES);
        }
        if request_line.len() + line.len() > MAX_HEADER_BYTES {
            return write_response(&mut stream, 431, "text/plain", b"headers too large");
        }
    }

    let path = target.split('?').next().unwrap_or("");
    match (method.as_str(), path) {
        ("GET", "/health" | "/") => {
            let body = json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
                "tools": crate::mcp::tools().len(),
            })
            .to_string();
            write_response(&mut stream, 200, "application/json", body.as_bytes())
        }
        ("GET", "/mcp") => write_response(
            &mut stream,
            405,
            "application/json",
            br#"{"error":"use POST /mcp for JSON-RPC calls"}"#,
        ),
        ("POST", "/mcp") => {
            if content_length == 0 {
                return write_response(
                    &mut stream,
                    400,
                    "application/json",
                    br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"empty request body"}}"#,
                );
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body)?;
            let request: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(err) => {
                    let msg = json!({
                        "jsonrpc": "2.0",
                        "id": null,
                        "error": { "code": -32700, "message": format!("parse error: {err}") },
                    })
                    .to_string();
                    return write_response(&mut stream, 400, "application/json", msg.as_bytes());
                }
            };
            match crate::mcp::handle(&request) {
                Some(response) => {
                    let payload = serde_json::to_vec(&response)?;
                    write_response(&mut stream, 200, "application/json", &payload)
                }
                None => write_response(&mut stream, 202, "application/json", b"{}"),
            }
        }
        ("OPTIONS", "/mcp" | "/health") => write_response(&mut stream, 204, "text/plain", b""),
        _ => write_response(
            &mut stream,
            404,
            "application/json",
            br#"{"error":"not found"}"#,
        ),
    }
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let mut header = String::with_capacity(256);
    let _ = writeln!(header, "HTTP/1.1 {status} {reason}");
    let _ = writeln!(header, "Content-Type: {content_type}");
    let _ = writeln!(header, "Content-Length: {}", body.len());
    let _ = writeln!(header, "Access-Control-Allow-Origin: *");
    let _ = writeln!(header, "Access-Control-Allow-Methods: GET, POST, OPTIONS");
    let _ = writeln!(
        header,
        "Access-Control-Allow-Headers: Content-Type, Mcp-Session-Id"
    );
    let _ = writeln!(header, "Connection: close");
    let _ = writeln!(header);
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(raw: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream).unwrap();
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.write_all(raw.as_bytes()).unwrap();
        client.flush().unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        server.join().unwrap();
        response
    }

    #[test]
    fn health_returns_json_status() {
        let raw = request("GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(raw.starts_with("HTTP/1.1 200 OK"));
        assert!(raw.contains("\"status\":\"ok\""));
        assert!(raw.contains("\"tools\":8"));
    }

    #[test]
    fn post_mcp_answers_initialize_over_http() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
        let raw = format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        );
        let response = request(&raw);
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"serverInfo\""));
        assert!(response.contains("radar30"));
    }

    #[test]
    fn unknown_path_is_404() {
        let raw = request("GET /nope HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(raw.starts_with("HTTP/1.1 404 Not Found"));
    }
}
