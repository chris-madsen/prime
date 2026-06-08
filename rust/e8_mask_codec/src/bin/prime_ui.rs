use e8_mask_codec::runtime::{is_prime_query_with_source, next_prime_query_with_source};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

const INDEX_HTML: &str = include_str!("../../ui/prime_ui.html");
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 43173;
const DEFAULT_ARCHIVE_DIR: &str = "data/runs/count_1e11/archive";

#[derive(Clone, Debug)]
struct Config {
    host: String,
    port: u16,
    archive_dir: PathBuf,
    pid_file: Option<PathBuf>,
    log_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct QueryRequest {
    n: String,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    error: &'a str,
}

#[derive(Serialize)]
struct HealthResponse<'a> {
    ok: bool,
    host: &'a str,
    port: u16,
    archive_dir: String,
}

#[derive(Serialize)]
struct IsPrimeResponse {
    n: String,
    is_prime: bool,
    source: &'static str,
    elapsed_ms: f64,
}

#[derive(Serialize)]
struct NextPrimeResponse {
    n: String,
    next_prime: String,
    source: &'static str,
    elapsed_ms: f64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config = parse_args()?;
    let bind_addr = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&bind_addr)
        .map_err(|e| format!("bind {bind_addr} failed: {e}"))?;
    if let Some(parent) = config.pid_file.as_deref().and_then(Path::parent) {
        fs::create_dir_all(parent).map_err(|e| format!("create pid dir failed: {e}"))?;
    }
    if let Some(parent) = config.log_file.as_deref().and_then(Path::parent) {
        fs::create_dir_all(parent).map_err(|e| format!("create log dir failed: {e}"))?;
    }
    if let Some(pid_file) = &config.pid_file {
        fs::write(pid_file, format!("{}\n", process::id()))
            .map_err(|e| format!("write pid file failed: {e}"))?;
    }
    if let Some(log_file) = &config.log_file {
        let line = format!(
            "prime_ui started pid={} host={} port={} archive_dir={}\n",
            process::id(),
            config.host,
            config.port,
            config.archive_dir.display()
        );
        let _ = fs::OpenOptions::new().create(true).append(true).open(log_file).and_then(|mut f| f.write_all(line.as_bytes()));
    }
    let shared = Arc::new(config);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let cfg = Arc::clone(&shared);
                thread::spawn(move || {
                    let _ = handle_connection(stream, &cfg);
                });
            }
            Err(error) => eprintln!("accept error: {error}"),
        }
    }
    Ok(())
}

fn parse_args() -> Result<Config, String> {
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;
    let mut archive_dir = PathBuf::from(DEFAULT_ARCHIVE_DIR);
    let mut pid_file = None;
    let mut log_file = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => host = args.next().ok_or("missing value after --host")?,
            "--port" => {
                port = args
                    .next()
                    .ok_or("missing value after --port")?
                    .parse()
                    .map_err(|e| format!("invalid --port: {e}"))?
            }
            "--archive-dir" => {
                archive_dir = PathBuf::from(args.next().ok_or("missing value after --archive-dir")?)
            }
            "--pid-file" => pid_file = Some(PathBuf::from(args.next().ok_or("missing value after --pid-file")?)),
            "--log-file" => log_file = Some(PathBuf::from(args.next().ok_or("missing value after --log-file")?)),
            "--help" | "-h" => {
                print_help();
                process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if host != DEFAULT_HOST {
        return Err("prime_ui v1 only allows --host 127.0.0.1".into());
    }

    Ok(Config {
        host,
        port,
        archive_dir,
        pid_file,
        log_file,
    })
}

fn print_help() {
    println!(
        "prime_ui [--host 127.0.0.1] [--port 43173] [--archive-dir data/runs/count_1e11/archive] [--pid-file <path>] [--log-file <path>]"
    );
}

fn handle_connection(mut stream: TcpStream, config: &Config) -> Result<(), String> {
    let mut buffer = vec![0_u8; 1024 * 128];
    let read = stream.read(&mut buffer).map_err(|e| format!("read failed: {e}"))?;
    if read == 0 {
        return Ok(());
    }
    let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
    let (method, path, body) = parse_http_request(&request)?;

    match (method, path) {
        ("GET", "/") => write_response(&mut stream, 200, "text/html; charset=utf-8", INDEX_HTML.as_bytes()),
        ("GET", "/healthz") => {
            let payload = serde_json::to_vec(&HealthResponse {
                ok: true,
                host: &config.host,
                port: config.port,
                archive_dir: config.archive_dir.display().to_string(),
            })
            .map_err(|e| format!("health json failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        ("POST", "/api/is-prime") => {
            let req: QueryRequest = serde_json::from_str(body).map_err(|_| "invalid JSON request".to_string())?;
            let n = parse_n(&req.n)?;
            let started = Instant::now();
            let result = is_prime_query_with_source(Some(&config.archive_dir), n).map_err(|e| e.to_string())?;
            let payload = serde_json::to_vec(&IsPrimeResponse {
                n: req.n,
                is_prime: result.is_prime,
                source: result.source.as_str(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            })
            .map_err(|e| format!("encode is-prime response failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        ("POST", "/api/next-prime") => {
            let req: QueryRequest = serde_json::from_str(body).map_err(|_| "invalid JSON request".to_string())?;
            let n = parse_n(&req.n)?;
            let started = Instant::now();
            let result = next_prime_query_with_source(Some(&config.archive_dir), n).map_err(|e| e.to_string())?;
            let payload = serde_json::to_vec(&NextPrimeResponse {
                n: req.n,
                next_prime: result.next_prime.to_string(),
                source: result.source.as_str(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            })
            .map_err(|e| format!("encode next-prime response failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        _ => {
            let payload = serde_json::to_vec(&ErrorResponse { error: "not found" })
                .map_err(|e| format!("encode 404 failed: {e}"))?;
            write_response(&mut stream, 404, "application/json; charset=utf-8", &payload)
        }
    }
}

fn parse_http_request(request: &str) -> Result<(&str, &str, &str), String> {
    let (head, body) = request
        .split_once("\r\n\r\n")
        .or_else(|| request.split_once("\n\n"))
        .unwrap_or((request, ""));
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?;
    let path = parts.next().ok_or("missing path")?;
    Ok((method, path, body))
}

fn parse_n(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("missing numeric string field `n`".into());
    }
    trimmed
        .parse::<u64>()
        .map_err(|e| format!("invalid u64 string for n: {e}"))
}

fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<(), String> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        status_text,
        content_type,
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|e| format!("write response failed: {e}"))
}
