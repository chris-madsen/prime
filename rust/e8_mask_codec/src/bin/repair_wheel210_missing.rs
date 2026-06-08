use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use zstd::stream::encode_all;

const WHEEL210_ZSTD_LEVEL: i32 = 1;

#[derive(Clone, Copy, Debug)]
struct ExactChunkMeta {
    chunk_id: u64,
    prime_count: u64,
    first_prime: u64,
    last_prime: u64,
}

#[derive(Clone, Copy, Debug)]
struct WheelChunkMeta {
    chunk_id: u64,
    first_prime: u64,
    last_prime: u64,
    cycle_start: u64,
    cycles: u64,
    raw_bytes: u64,
    compressed_bytes: u64,
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut archive_dir: Option<PathBuf> = None;
    let mut from_chunk: Option<u64> = None;
    let mut to_chunk: Option<u64> = None;
    let mut no_finalize = false;
    let mut finalize_only = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--archive-dir" => {
                archive_dir = Some(PathBuf::from(
                    args.next().ok_or("missing value after --archive-dir")?,
                ));
            }
            "--from-chunk" => {
                from_chunk = Some(
                    args.next()
                        .ok_or("missing value after --from-chunk")?
                        .parse()
                        .map_err(|e| format!("invalid --from-chunk: {e}"))?,
                );
            }
            "--to-chunk" => {
                to_chunk = Some(
                    args.next()
                        .ok_or("missing value after --to-chunk")?
                        .parse()
                        .map_err(|e| format!("invalid --to-chunk: {e}"))?,
                );
            }
            "--no-finalize" => no_finalize = true,
            "--finalize-only" => finalize_only = true,
            "--help" | "-h" => {
                println!(
                    "repair-wheel210-missing --archive-dir <path> [--from-chunk <id>] [--to-chunk <id>] [--no-finalize] [--finalize-only]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let archive_dir = archive_dir.ok_or("missing --archive-dir")?;
    if finalize_only {
        let wheel_dir = archive_dir.join("wheel210");
        return rewrite_wheel_index_and_manifest(&archive_dir, &wheel_dir)
            .map_err(|e| e.to_string());
    }
    run(&archive_dir, from_chunk, to_chunk, no_finalize).map_err(|e| e.to_string())
}

fn run(
    archive_dir: &Path,
    from_chunk: Option<u64>,
    to_chunk: Option<u64>,
    no_finalize: bool,
) -> Result<(), String> {
    let exact_index = archive_dir.join("index.tsv");
    let wheel_dir = archive_dir.join("wheel210");
    fs::create_dir_all(&wheel_dir).map_err(|e| format!("create {} failed: {e}", wheel_dir.display()))?;

    let exact_rows = load_exact_index(&exact_index)?;
    let existing = existing_wheel_chunk_ids(&wheel_dir)?;

    let selected: Vec<_> = exact_rows
        .into_iter()
        .filter(|row| from_chunk.is_none_or(|v| row.chunk_id >= v))
        .filter(|row| to_chunk.is_none_or(|v| row.chunk_id <= v))
        .filter(|row| !existing.contains(&row.chunk_id))
        .collect();

    if selected.is_empty() {
        println!("nothing to repair");
        if !no_finalize {
            rewrite_wheel_index_and_manifest(archive_dir, &wheel_dir)?;
        }
        return Ok(());
    }

    println!(
        "repairing {} missing wheel210 chunks in [{}..{}]",
        selected.len(),
        selected.first().map(|x| x.chunk_id).unwrap_or(0),
        selected.last().map(|x| x.chunk_id).unwrap_or(0)
    );

    for (i, row) in selected.iter().enumerate() {
        println!(
            "chunk {}/{} id={} range=[{}, {}] expected_primes={}",
            i + 1,
            selected.len(),
            row.chunk_id,
            row.first_prime,
            row.last_prime,
            row.prime_count
        );
        let meta = rebuild_one_wheel_chunk(&wheel_dir, *row)?;
        println!(
            "ok chunk_id={} first={} last={} cycles={} compressed_bytes={}",
            meta.chunk_id, meta.first_prime, meta.last_prime, meta.cycles, meta.compressed_bytes
        );
    }

    if !no_finalize {
        rewrite_wheel_index_and_manifest(archive_dir, &wheel_dir)?;
        println!("repair complete wheel_dir={}", wheel_dir.display());
    } else {
        println!("repair chunk pass complete without finalization");
    }
    Ok(())
}

fn load_exact_index(path: &Path) -> Result<Vec<ExactChunkMeta>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {} failed: {e}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<_> = line.split('\t').collect();
        if cols.len() != 5 {
            continue;
        }
        out.push(ExactChunkMeta {
            chunk_id: cols[0].parse().map_err(|e| format!("invalid chunk_id: {e}"))?,
            prime_count: cols[1].parse().map_err(|e| format!("invalid prime_count: {e}"))?,
            first_prime: cols[2].parse().map_err(|e| format!("invalid first_prime: {e}"))?,
            last_prime: cols[3].parse().map_err(|e| format!("invalid last_prime: {e}"))?,
        });
    }
    Ok(out)
}

fn existing_wheel_chunk_ids(wheel_dir: &Path) -> Result<HashSet<u64>, String> {
    let mut out = HashSet::new();
    for entry in fs::read_dir(wheel_dir).map_err(|e| format!("read_dir {} failed: {e}", wheel_dir.display()))? {
        let entry = entry.map_err(|e| format!("read_dir entry failed: {e}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if let Some(id) = name
            .strip_prefix("chunk_")
            .and_then(|s| s.strip_suffix(".zst"))
            .and_then(|s| s.parse::<u64>().ok())
        {
            out.insert(id);
        }
    }
    Ok(out)
}

fn rebuild_one_wheel_chunk(wheel_dir: &Path, row: ExactChunkMeta) -> Result<WheelChunkMeta, String> {
    let primes = primes_in_range(row.first_prime, row.last_prime);
    if primes.len() as u64 != row.prime_count {
        return Err(format!(
            "prime_count mismatch for chunk_id={}: got {}, expected {}",
            row.chunk_id,
            primes.len(),
            row.prime_count
        ));
    }
    if primes.first().copied() != Some(row.first_prime) || primes.last().copied() != Some(row.last_prime) {
        return Err(format!(
            "boundary mismatch for chunk_id={}: got [{:?}, {:?}] expected [{}, {}]",
            row.chunk_id,
            primes.first(),
            primes.last(),
            row.first_prime,
            row.last_prime
        ));
    }

    let lookup = wheel210_lookup();
    let cycle_start = row.first_prime / 210;
    let cycle_end = row.last_prime / 210;
    let cycles = cycle_end - cycle_start + 1;
    let mut raw = vec![0_u8; cycles as usize * 6];
    for &prime in &primes {
        let bit_index = lookup[(prime % 210) as usize];
        if bit_index < 0 {
            continue;
        }
        let cycle = prime / 210;
        let local_cycle = (cycle - cycle_start) as usize;
        let byte_base = local_cycle * 6;
        let bit = bit_index as usize;
        raw[byte_base + bit / 8] |= 1_u8 << (bit % 8);
    }

    let compressed = encode_all(&raw[..], WHEEL210_ZSTD_LEVEL)
        .map_err(|e| format!("zstd encode failed for chunk_id={}: {e}", row.chunk_id))?;
    let temp_path = wheel_dir.join(format!("chunk.{}.tmp", std::process::id()));
    let final_path = wheel_dir.join(format!("chunk_{:020}.zst", row.chunk_id));
    let mut bytes = Vec::with_capacity(64 + compressed.len());
    bytes.extend_from_slice(b"W210A1\0");
    bytes.extend_from_slice(&row.first_prime.to_le_bytes());
    bytes.extend_from_slice(&row.last_prime.to_le_bytes());
    bytes.extend_from_slice(&cycle_start.to_le_bytes());
    bytes.extend_from_slice(&cycles.to_le_bytes());
    bytes.extend_from_slice(&(raw.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&compressed);
    let mut file = File::create(&temp_path)
        .map_err(|e| format!("create {} failed: {e}", temp_path.display()))?;
    file.write_all(&bytes)
        .map_err(|e| format!("write {} failed: {e}", temp_path.display()))?;
    file.sync_all()
        .map_err(|e| format!("sync {} failed: {e}", temp_path.display()))?;
    fs::rename(&temp_path, &final_path)
        .map_err(|e| format!("rename {} -> {} failed: {e}", temp_path.display(), final_path.display()))?;

    Ok(WheelChunkMeta {
        chunk_id: row.chunk_id,
        first_prime: row.first_prime,
        last_prime: row.last_prime,
        cycle_start,
        cycles,
        raw_bytes: raw.len() as u64,
        compressed_bytes: compressed.len() as u64,
    })
}

fn primes_in_range(low: u64, high: u64) -> Vec<u64> {
    if high < 2 || low > high {
        return Vec::new();
    }
    let mut out = Vec::new();
    if low <= 2 && 2 <= high {
        out.push(2);
    }
    let start = if low <= 3 { 3 } else if low % 2 == 0 { low + 1 } else { low };
    if start > high {
        return out;
    }
    let len = ((high - start) / 2 + 1) as usize;
    let mut sieve = vec![true; len];
    let limit = integer_sqrt(high);
    let base = simple_primes_up_to(limit);
    for &p in &base {
        if p == 2 {
            continue;
        }
        let p64 = u64::from(p);
        let mut multiple = p64.saturating_mul(p64);
        if multiple < start {
            multiple = start.div_ceil(p64) * p64;
        }
        if multiple % 2 == 0 {
            multiple += p64;
        }
        let step = p64 as usize;
        let mut idx = ((multiple - start) / 2) as usize;
        while idx < sieve.len() {
            sieve[idx] = false;
            idx += step;
        }
    }
    for (i, is_prime) in sieve.into_iter().enumerate() {
        if is_prime {
            out.push(start + 2 * i as u64);
        }
    }
    out
}

fn integer_sqrt(value: u64) -> u64 {
    (value as f64).sqrt().floor() as u64
}

fn simple_primes_up_to(limit: u64) -> Vec<u32> {
    if limit < 2 {
        return Vec::new();
    }
    let size = limit as usize + 1;
    let mut sieve = vec![true; size];
    sieve[0] = false;
    sieve[1] = false;
    let root = integer_sqrt(limit) as usize;
    for p in 2..=root {
        if sieve[p] {
            let mut m = p * p;
            while m < size {
                sieve[m] = false;
                m += p;
            }
        }
    }
    sieve
        .into_iter()
        .enumerate()
        .filter_map(|(value, is_prime)| is_prime.then_some(value as u32))
        .collect()
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

fn wheel210_lookup() -> [i16; 210] {
    let mut lookup = [-1_i16; 210];
    let mut bit = 0_i16;
    for residue in 0..210 {
        if gcd_u64(residue as u64, 210) == 1 {
            lookup[residue] = bit;
            bit += 1;
        }
    }
    lookup
}

fn inspect_wheel210_chunk(path: &Path, chunk_id: u64) -> Result<WheelChunkMeta, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {} failed: {e}", path.display()))?;
    if bytes.len() < 55 || &bytes[..7] != b"W210A1\0" {
        return Err(format!("invalid wheel210 chunk header: {}", path.display()));
    }
    Ok(WheelChunkMeta {
        chunk_id,
        first_prime: u64::from_le_bytes(bytes[7..15].try_into().unwrap()),
        last_prime: u64::from_le_bytes(bytes[15..23].try_into().unwrap()),
        cycle_start: u64::from_le_bytes(bytes[23..31].try_into().unwrap()),
        cycles: u64::from_le_bytes(bytes[31..39].try_into().unwrap()),
        raw_bytes: u64::from_le_bytes(bytes[39..47].try_into().unwrap()),
        compressed_bytes: u64::from_le_bytes(bytes[47..55].try_into().unwrap()),
    })
}

fn rewrite_wheel_index_and_manifest(archive_dir: &Path, wheel_dir: &Path) -> Result<(), String> {
    let mut metas = Vec::new();
    for entry in fs::read_dir(wheel_dir).map_err(|e| format!("read_dir {} failed: {e}", wheel_dir.display()))? {
        let entry = entry.map_err(|e| format!("read_dir entry failed: {e}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let Some(chunk_id) = name
            .strip_prefix("chunk_")
            .and_then(|s| s.strip_suffix(".zst"))
            .and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        metas.push(inspect_wheel210_chunk(&path, chunk_id)?);
    }
    metas.sort_by_key(|meta| meta.chunk_id);

    let index_path = wheel_dir.join("index.tsv");
    let index_tmp = wheel_dir.join("index.tsv.tmp");
    let mut index = File::create(&index_tmp)
        .map_err(|e| format!("create {} failed: {e}", index_tmp.display()))?;
    writeln!(
        index,
        "chunk_id\tfirst_prime\tlast_prime\tcycle_start\tcycles\traw_bytes\tcompressed_bytes"
    )
    .map_err(|e| format!("write {} failed: {e}", index_tmp.display()))?;
    for meta in &metas {
        writeln!(
            index,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            meta.chunk_id,
            meta.first_prime,
            meta.last_prime,
            meta.cycle_start,
            meta.cycles,
            meta.raw_bytes,
            meta.compressed_bytes
        )
        .map_err(|e| format!("write {} failed: {e}", index_tmp.display()))?;
    }
    index.sync_all()
        .map_err(|e| format!("sync {} failed: {e}", index_tmp.display()))?;
    fs::rename(&index_tmp, &index_path)
        .map_err(|e| format!("rename {} -> {} failed: {e}", index_tmp.display(), index_path.display()))?;

    let exact_rows = load_exact_index(&wheel_dir.parent().unwrap().join("index.tsv"))?;
    let exact_counts: std::collections::HashMap<u64, u64> =
        exact_rows.into_iter().map(|row| (row.chunk_id, row.prime_count)).collect();
    let committed_primes = baseline_committed_primes(archive_dir, &metas)?
        .unwrap_or_else(|| {
            metas.iter()
                .map(|meta| exact_counts.get(&meta.chunk_id).copied().unwrap_or(0))
                .sum()
        });
    let manifest_path = wheel_dir.join("manifest.txt");
    let manifest_tmp = wheel_dir.join("manifest.txt.tmp");
    let mut manifest = File::create(&manifest_tmp)
        .map_err(|e| format!("create {} failed: {e}", manifest_tmp.display()))?;
    write!(
        manifest,
        concat!(
            "format=e8-wheel210-v1\n",
            "encoding=48-of-210-bitset+zstd\n",
            "special_primes=2,3,5,7\n",
            "chunk_file_pattern=chunk_%020u.zst\n",
            "index_file=index.tsv\n",
            "committed_chunks={}\n",
            "committed_primes={}\n",
        ),
        metas.len(),
        committed_primes
    )
    .map_err(|e| format!("write {} failed: {e}", manifest_tmp.display()))?;
    manifest
        .sync_all()
        .map_err(|e| format!("sync {} failed: {e}", manifest_tmp.display()))?;
    fs::rename(&manifest_tmp, &manifest_path).map_err(|e| {
        format!(
            "rename {} -> {} failed: {e}",
            manifest_tmp.display(),
            manifest_path.display()
        )
    })?;
    Ok(())
}

fn baseline_committed_primes(
    archive_dir: &Path,
    metas: &[WheelChunkMeta],
) -> Result<Option<u64>, String> {
    let run_dir = archive_dir
        .parent()
        .ok_or_else(|| format!("archive dir has no parent: {}", archive_dir.display()))?;
    let checkpoint = run_dir.join("count.checkpoint");
    if !checkpoint.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&checkpoint)
        .map_err(|e| format!("read {} failed: {e}", checkpoint.display()))?;
    let mut checkpoint_committed = None;
    let mut checkpoint_chunks_written = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("archive_committed_primes=") {
            checkpoint_committed = Some(v.parse::<u64>().map_err(|e| format!("invalid archive_committed_primes: {e}"))?);
        } else if let Some(v) = line.strip_prefix("archive_chunks_written=") {
            checkpoint_chunks_written = Some(v.parse::<u64>().map_err(|e| format!("invalid archive_chunks_written: {e}"))?);
        }
    }
    let Some(committed) = checkpoint_committed else {
        return Ok(None);
    };
    let Some(chunks_written) = checkpoint_chunks_written else {
        return Ok(None);
    };
    let extra_high_chunks = metas
        .iter()
        .filter(|meta| meta.chunk_id >= chunks_written)
        .count() as u64;
    Ok(Some(
        committed.saturating_add(extra_high_chunks.saturating_mul(16_000_000)),
    ))
}
