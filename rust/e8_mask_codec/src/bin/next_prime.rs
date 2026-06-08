use e8_mask_codec::runtime::next_prime_query;
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut archive_dir: Option<PathBuf> = None;
    let mut n: Option<u64> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--archive-dir" => archive_dir = Some(PathBuf::from(args.next().ok_or("missing value after --archive-dir")?)),
            "--n" => n = Some(args.next().ok_or("missing value after --n")?.parse().map_err(|e| format!("invalid --n: {e}"))?),
            "--help" | "-h" => { println!("next-prime --n <N> [--archive-dir <path>]  # first prime > N"); return Ok(()); }
            value if n.is_none() => n = Some(value.parse().map_err(|e| format!("invalid N: {e}"))?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let n = n.ok_or("missing --n <N>")?;
    let result = next_prime_query(archive_dir.as_deref(), n).map_err(|e| e.to_string())?;
    println!("n={n}");
    println!("next_prime={result}");
    Ok(())
}
