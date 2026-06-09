use e8_mask_codec::runtime::{
    Backend, next_prime_big,
    next_prime_fast, next_prime_proven,
    detect_prover,
    PrimeQueryConfig,
    validate_decimal_input,
};
use serde::Serialize;
use std::str::FromStr;
use std::time::Instant;

#[derive(Serialize)]
struct JsonResult {
    n_preview: String,
    n_digits: usize,
    next_prime: String,
    status: String,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    certificate_snippet: Option<String>,
    elapsed_ms: f64,
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut n_str: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut backend = Backend::Hybrid;
    let mut prove = false;
    let mut json = false;
    let mut rounds: Option<u32> = None;
    let mut strong_rounds: Option<u32> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--n" => {
                n_str = Some(args.next().ok_or("missing value after --n")?);
            }
            "--file" | "-f" => {
                file_path = Some(args.next().ok_or("missing value after --file")?);
            }
            "--backend" => {
                let val = args.next().ok_or("missing value after --backend")?;
                backend = Backend::from_str(&val).map_err(|e| e.to_string())?;
            }
            "--prove" => prove = true,
            "--json" => json = true,
            "--progress" => { /* progress to stderr always on for long computations */ }
            "--rounds" => {
                rounds = Some(
                    args.next()
                        .ok_or("missing value after --rounds")?
                        .parse()
                        .map_err(|e| format!("invalid --rounds: {e}"))?,
                );
            }
            "--strong-rounds" => {
                strong_rounds = Some(
                    args.next()
                        .ok_or("missing value after --strong-rounds")?
                        .parse()
                        .map_err(|e| format!("invalid --strong-rounds: {e}"))?,
                );
            }
            "--help" | "-h" => {
                println!("next-prime-big --n <N> [options]");
                println!("               --file <path> [options]");
                println!();
                println!("Options:");
                println!("  --backend cpu|gpu|hybrid   (default: hybrid)");
                println!("  --prove                    run ECPP proof via PARI/GP");
                println!("  --json                     output as JSON");
                println!("  --rounds <N>               Miller-Rabin fast rounds (default: 32)");
                println!("  --strong-rounds <N>        Miller-Rabin strong rounds (default: 64)");
                println!("  --progress                 show progress to stderr");
                return Ok(());
            }
            value if n_str.is_none() && file_path.is_none() => {
                n_str = Some(value.to_string());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let raw_str = match (n_str, file_path) {
        (Some(s), None) => s,
        (None, Some(path)) => {
            std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read file {path}: {e}"))?
                .split_whitespace()
                .collect::<String>()
        }
        (Some(_), Some(_)) => return Err("specify either --n or --file, not both".into()),
        (None, None) => return Err("missing input: use --n <N> or --file <path>".into()),
    };

    let n = validate_decimal_input(&raw_str)
        .map_err(|e| format!("input error: {e}"))?;
    let n_str = raw_str.trim().trim_start_matches('+').to_string();
    let n_digits = n_str.len();
    let n_preview = if n_digits <= 40 {
        n_str.clone()
    } else {
        format!("{}…{} ({} digits)", &n_str[..20], &n_str[n_digits - 10..], n_digits)
    };

    let started = Instant::now();

    // Build config with optional overrides
    let mut cfg = PrimeQueryConfig::default();
    if let Some(r) = rounds { cfg.miller_rabin_rounds_fast = r; }
    if let Some(r) = strong_rounds { cfg.miller_rabin_rounds_strong = r; }

    if prove {
        let prover = detect_prover();
        if !prover.is_available() {
            if json {
                println!("{{\"error\":\"ECPP backend не подключён. Установите PARI/GP (pari-gp).\"}}");
            } else {
                eprintln!("ECPP unavailable: install PARI/GP (pari-gp)");
                eprintln!("Falling back to fast (probabilistic) nextPrime.");
            }
        }
        let elapsed_ms_ref = started.elapsed().as_secs_f64() * 1000.0;
        match next_prime_proven(&n, &cfg, prover.as_ref(), None) {
            Ok(result) => {
                let next_str = result.p.to_string();
                let snippet = if result.certificate.raw.len() > 200 {
                    format!("{}…", &result.certificate.raw[..200])
                } else {
                    result.certificate.raw.clone()
                };
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                if json {
                    let out = serde_json::to_string(&JsonResult {
                        n_preview, n_digits,
                        next_prime: next_str,
                        status: "proven_prime".into(),
                        method: "mask+miller-rabin+ecpp".into(),
                        certificate_snippet: Some(snippet),
                        elapsed_ms,
                    }).unwrap();
                    println!("{out}");
                } else {
                    println!("n={n_preview}");
                    println!("next_prime={next_str}");
                    println!("status=proven_prime");
                    println!("method=mask+miller-rabin+ecpp");
                    println!("certificate={}", &result.certificate.raw[..result.certificate.raw.len().min(200)]);
                    println!("elapsed_ms={elapsed_ms:.1}");
                }
            }
            Err(e) => {
                // Prover failed — fall back to fast
                let fast = next_prime_fast(&n, &cfg, None);
                let next_str = fast.p.to_string();
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                if json {
                    let out = serde_json::to_string(&JsonResult {
                        n_preview, n_digits,
                        next_prime: next_str,
                        status: "probable_prime".into(),
                        method: format!("mask+miller-rabin; ecpp failed: {e}"),
                        certificate_snippet: None,
                        elapsed_ms,
                    }).unwrap();
                    println!("{out}");
                } else {
                    println!("n={n_preview}");
                    println!("next_prime={next_str}");
                    println!("status=probable_prime");
                    println!("ecpp_error={e}");
                    println!("elapsed_ms={elapsed_ms:.1}");
                }
                let _ = elapsed_ms_ref;
            }
        }
    } else {
        // Fast (probabilistic) mode
        let (next_str, status, method) = if n_digits > 19 {
            let fast = next_prime_fast(&n, &cfg, None);
            (fast.p.to_string(), "probable_prime", "mask+miller-rabin")
        } else {
            let next = next_prime_big(&n, backend, None);
            (next.to_string(), "probable_prime", "e8_mask")
        };
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        if json {
            let out = serde_json::to_string(&JsonResult {
                n_preview, n_digits,
                next_prime: next_str,
                status: status.into(),
                method: method.into(),
                certificate_snippet: None,
                elapsed_ms,
            }).unwrap();
            println!("{out}");
        } else {
            println!("n={n_preview}");
            println!("next_prime={next_str}");
            println!("status={status}");
            println!("method={method}");
            println!("elapsed_ms={elapsed_ms:.1}");
        }
    }

    Ok(())
}
