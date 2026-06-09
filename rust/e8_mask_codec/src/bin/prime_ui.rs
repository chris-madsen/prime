use e8_mask_codec::runtime::{
    is_prime_big, next_prime_big,
    is_prime_query_with_source, next_prime_query_with_source,
    next_prime_fast,
    is_prime_proven, next_prime_proven,
    detect_prover,
    ProvenPrimeResult,
    PrimeQueryConfig,
    Backend,
};
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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
    ecpp_available: bool,
}

#[derive(Deserialize)]
struct QueryRequest {
    n: String,
}

#[derive(Deserialize)]
struct QueryRequestBig {
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

#[derive(Serialize)]
struct IsPrimeBigResponse {
    n_preview: String,
    n_digits: usize,
    is_prime: bool,
    status: &'static str,
    method: &'static str,
    elapsed_ms: f64,
}

#[derive(Serialize)]
struct NextPrimeBigResponse {
    n_preview: String,
    n_digits: usize,
    next_prime: String,
    method: &'static str,
    elapsed_ms: f64,
}

#[derive(Serialize)]
struct IsPrimeBigProvenResponse {
    n_preview: String,
    n_digits: usize,
    is_prime: bool,
    status: String,
    method: String,
    certificate_available: bool,
    certificate_snippet: Option<String>,
    elapsed_ms: f64,
}

#[derive(Serialize)]
struct NextPrimeBigProvenResponse {
    n_preview: String,
    n_digits: usize,
    next_prime: String,
    status: String,
    method: String,
    certificate_available: bool,
    certificate_snippet: Option<String>,
    elapsed_ms: f64,
}

#[derive(Serialize)]
struct EcppUnavailableResponse {
    error: String,
    status: &'static str,
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

    Ok(Config {
        host,
        port,
        archive_dir,
        pid_file,
        log_file,
        ecpp_available: detect_prover().is_available(),
    })
}

fn print_help() {
    println!(
        "prime_ui [--host 127.0.0.1] [--port 43173] [--archive-dir data/runs/count_1e11/archive] [--pid-file <path>] [--log-file <path>]"
    );
}

fn read_full_request(stream: &mut TcpStream) -> Result<String, String> {
    // Read until we have the full headers + body based on Content-Length
    let mut raw: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 8192];
    // Read until we find end of headers
    loop {
        let n = stream.read(&mut tmp).map_err(|e| format!("read failed: {e}"))?;
        if n == 0 { break; }
        raw.extend_from_slice(&tmp[..n]);
        // Check if headers are complete
        if raw.windows(4).any(|w| w == b"\r\n\r\n") { break; }
        if raw.len() > 16 * 1024 * 1024 { return Err("request too large".into()); }
    }
    // Parse Content-Length and read remaining body if needed
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(raw.len());
    let headers = String::from_utf8_lossy(&raw[..header_end]);
    let content_length: usize = headers.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let body_so_far = raw.len().saturating_sub(header_end);
    if content_length > body_so_far {
        let remaining = content_length - body_so_far;
        if remaining > 16 * 1024 * 1024 { return Err("request body too large (max 16MB)".into()); }
        let old_len = raw.len();
        raw.resize(old_len + remaining, 0);
        stream.read_exact(&mut raw[old_len..]).map_err(|e| format!("read body failed: {e}"))?;
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

fn send_400(stream: &mut TcpStream, msg: &str) {
    // Escape the message for JSON string embedding
    let escaped = msg.replace('\\', "\\\\").replace('"', "\\\"");
    let payload = format!("{{\"error\":\"{escaped}\"}}");
    let _ = write_response(stream, 400, "application/json; charset=utf-8", payload.as_bytes());
}

fn handle_connection(mut stream: TcpStream, config: &Config) -> Result<(), String> {
    // Set read timeout so a slow/stalled client doesn't tie up the thread forever
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));

    let request = match read_full_request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            send_400(&mut stream, &e);
            return Ok(());
        }
    };
    if request.is_empty() {
        return Ok(());
    }
    let (method, path, body) = match parse_http_request(&request) {
        Ok(t) => t,
        Err(e) => {
            send_400(&mut stream, &e);
            return Ok(());
        }
    };

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
            let req: QueryRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(_) => { send_400(&mut stream, "invalid JSON request"); return Ok(()); }
            };
            let n = match parse_n(&req.n) {
                Ok(v) => v,
                Err(e) => { send_400(&mut stream, &e); return Ok(()); }
            };
            let started = Instant::now();
            let result = match is_prime_query_with_source(Some(&config.archive_dir), n) {
                Ok(r) => r,
                Err(e) => { send_400(&mut stream, &e.to_string()); return Ok(()); }
            };
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
            let req: QueryRequest = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(_) => { send_400(&mut stream, "invalid JSON request"); return Ok(()); }
            };
            let n = match parse_n(&req.n) {
                Ok(v) => v,
                Err(e) => { send_400(&mut stream, &e); return Ok(()); }
            };
            let started = Instant::now();
            let result = match next_prime_query_with_source(Some(&config.archive_dir), n) {
                Ok(r) => r,
                Err(e) => { send_400(&mut stream, &e.to_string()); return Ok(()); }
            };
            let payload = serde_json::to_vec(&NextPrimeResponse {
                n: req.n,
                next_prime: result.next_prime.to_string(),
                source: result.source.as_str(),
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            })
            .map_err(|e| format!("encode next-prime response failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        ("POST", "/api/is-prime-big") => {
            let req: QueryRequestBig = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(_) => { send_400(&mut stream, "invalid JSON request"); return Ok(()); }
            };
            let n_str = match validate_big_n(&req.n) {
                Ok(s) => s,
                Err(e) => { send_400(&mut stream, &e); return Ok(()); }
            };
            let n = match BigUint::parse_bytes(n_str.as_bytes(), 10) {
                Some(v) => v,
                None => { send_400(&mut stream, "failed to parse big integer"); return Ok(()); }
            };
            let n_digits = n_str.len();
            let n_preview = preview_digits(&n_str);
            let started = Instant::now();
            let is_prime = is_prime_big(&n, Backend::Hybrid, None);
            let method = if n_digits > 19 { "mask+miller-rabin" } else { "e8_mask" };
            let status = if is_prime { "probable_prime" } else { "composite" };
            let payload = serde_json::to_vec(&IsPrimeBigResponse {
                n_preview,
                n_digits,
                is_prime,
                status,
                method,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            })
            .map_err(|e| format!("encode is-prime-big response failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        ("POST", "/api/next-prime-big") => {
            let req: QueryRequestBig = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(_) => { send_400(&mut stream, "invalid JSON request"); return Ok(()); }
            };
            let n_str = match validate_big_n(&req.n) {
                Ok(s) => s,
                Err(e) => { send_400(&mut stream, &e); return Ok(()); }
            };
            let n = match BigUint::parse_bytes(n_str.as_bytes(), 10) {
                Some(v) => v,
                None => { send_400(&mut stream, "failed to parse big integer"); return Ok(()); }
            };
            let n_digits = n_str.len();
            let n_preview = preview_digits(&n_str);
            let started = Instant::now();
            let next = next_prime_big(&n, Backend::Hybrid, None);
            let next_str = next.to_string();
            let method = if n_digits > 19 { "mask+miller-rabin" } else { "e8_mask" };
            let payload = serde_json::to_vec(&NextPrimeBigResponse {
                n_preview,
                n_digits,
                next_prime: next_str,
                method,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            })
            .map_err(|e| format!("encode next-prime-big response failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        ("POST", "/api/is-prime-big-proven") => {
            let req: QueryRequestBig = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(_) => { send_400(&mut stream, "invalid JSON request"); return Ok(()); }
            };
            let n_str = match validate_big_n(&req.n) {
                Ok(s) => s,
                Err(e) => { send_400(&mut stream, &e); return Ok(()); }
            };
            let n = match BigUint::parse_bytes(n_str.as_bytes(), 10) {
                Some(v) => v,
                None => { send_400(&mut stream, "failed to parse big integer"); return Ok(()); }
            };
            let n_digits = n_str.len();
            let n_preview = preview_digits(&n_str);
            let started = Instant::now();

            if !config.ecpp_available {
                let payload = serde_json::to_vec(&EcppUnavailableResponse {
                    error: "ECPP backend не подключён. Установите PARI/GP (pari-gp).".to_string(),
                    status: "ecpp_unavailable",
                })
                .unwrap_or_else(|_| b"{\"error\":\"ecpp unavailable\"}".to_vec());
                return write_response(&mut stream, 200, "application/json; charset=utf-8", &payload);
            }

            let cfg = PrimeQueryConfig::default();
            let prover = detect_prover();
            let result = is_prime_proven(&n, &cfg, prover.as_ref(), None);
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

            let payload = match result {
                ProvenPrimeResult::Composite { .. } => {
                    serde_json::to_vec(&IsPrimeBigProvenResponse {
                        n_preview, n_digits, is_prime: false,
                        status: "composite".to_string(),
                        method: "mask+miller-rabin".to_string(),
                        certificate_available: false,
                        certificate_snippet: None,
                        elapsed_ms,
                    })
                }
                ProvenPrimeResult::ProvenPrime { certificate } => {
                    let snippet = if certificate.raw.len() > 200 {
                        Some(format!("{}…", &certificate.raw[..200]))
                    } else {
                        Some(certificate.raw.clone())
                    };
                    serde_json::to_vec(&IsPrimeBigProvenResponse {
                        n_preview, n_digits, is_prime: true,
                        status: "proven_prime".to_string(),
                        method: "mask+miller-rabin+ecpp".to_string(),
                        certificate_available: true,
                        certificate_snippet: snippet,
                        elapsed_ms,
                    })
                }
                ProvenPrimeResult::ProbablePrime { rounds, prover_error } => {
                    serde_json::to_vec(&IsPrimeBigProvenResponse {
                        n_preview, n_digits, is_prime: true,
                        status: "probable_prime".to_string(),
                        method: format!("mask+miller-rabin ({rounds} rounds); ecpp failed: {prover_error}"),
                        certificate_available: false,
                        certificate_snippet: None,
                        elapsed_ms,
                    })
                }
            }.map_err(|e| format!("encode is-prime-big-proven response failed: {e}"))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
        }
        ("POST", "/api/next-prime-big-proven") => {
            let req: QueryRequestBig = match serde_json::from_str(body) {
                Ok(r) => r,
                Err(_) => { send_400(&mut stream, "invalid JSON request"); return Ok(()); }
            };
            let n_str = match validate_big_n(&req.n) {
                Ok(s) => s,
                Err(e) => { send_400(&mut stream, &e); return Ok(()); }
            };
            let n = match BigUint::parse_bytes(n_str.as_bytes(), 10) {
                Some(v) => v,
                None => { send_400(&mut stream, "failed to parse big integer"); return Ok(()); }
            };
            let n_digits = n_str.len();
            let n_preview = preview_digits(&n_str);
            let started = Instant::now();

            if !config.ecpp_available {
                let payload = serde_json::to_vec(&EcppUnavailableResponse {
                    error: "ECPP backend не подключён. Установите PARI/GP (pari-gp).".to_string(),
                    status: "ecpp_unavailable",
                })
                .unwrap_or_else(|_| b"{\"error\":\"ecpp unavailable\"}".to_vec());
                return write_response(&mut stream, 200, "application/json; charset=utf-8", &payload);
            }

            let cfg = PrimeQueryConfig::default();
            let prover = detect_prover();
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

            match next_prime_proven(&n, &cfg, prover.as_ref(), None) {
                Ok(result) => {
                    let next_str = result.p.to_string();
                    let snippet = if result.certificate.raw.len() > 200 {
                        Some(format!("{}…", &result.certificate.raw[..200]))
                    } else {
                        Some(result.certificate.raw.clone())
                    };
                    let payload = serde_json::to_vec(&NextPrimeBigProvenResponse {
                        n_preview, n_digits,
                        next_prime: next_str,
                        status: "proven_prime".to_string(),
                        method: "mask+miller-rabin+ecpp".to_string(),
                        certificate_available: true,
                        certificate_snippet: snippet,
                        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                    })
                    .map_err(|e| format!("encode next-prime-big-proven response failed: {e}"))?;
                    write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
                }
                Err(e) => {
                    // Prover failed: fall back to fast result
                    let fast = next_prime_fast(&n, &cfg, None);
                    let next_str = fast.p.to_string();
                    let payload = serde_json::to_vec(&NextPrimeBigProvenResponse {
                        n_preview, n_digits,
                        next_prime: next_str,
                        status: "probable_prime".to_string(),
                        method: format!("mask+miller-rabin; ecpp failed: {e}"),
                        certificate_available: false,
                        certificate_snippet: None,
                        elapsed_ms,
                    })
                    .map_err(|e2| format!("encode next-prime-big-proven fallback failed: {e2}"))?;
                    write_response(&mut stream, 200, "application/json; charset=utf-8", &payload)
                }
            }
        }
        _ => {
            let payload = serde_json::to_vec(&ErrorResponse { error: "not found" })
                .unwrap_or_else(|_| b"{\"error\":\"not found\"}".to_vec());
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

fn validate_big_n(raw: &str) -> Result<String, String> {
    let s: String = raw.split_whitespace().collect();
    if s.is_empty() {
        return Err("input is empty".into());
    }
    if s.len() > 100_000 {
        return Err(format!("input too large: {} digits (max 100000)", s.len()));
    }
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "invalid input: expected decimal digits only (got non-digit near: {}…)",
            &s[..s.len().min(40)]
        ));
    }
    Ok(s)
}

fn preview_digits(s: &str) -> String {
    if s.len() <= 40 {
        s.to_string()
    } else {
        format!("{}…{} ({} digits total)", &s[..20], &s[s.len()-10..], s.len())
    }
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
