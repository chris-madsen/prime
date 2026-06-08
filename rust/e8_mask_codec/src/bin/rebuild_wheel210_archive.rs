use e8_mask_codec::runtime::rebuild_wheel210_archive;
use std::path::PathBuf;
use std::process;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut archive_dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--archive-dir" => {
                let value = args.next().ok_or("missing value after --archive-dir")?;
                archive_dir = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!("rebuild-wheel210-archive --archive-dir <path>");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let archive_dir = archive_dir.ok_or("missing --archive-dir")?;
    rebuild_wheel210_archive(&archive_dir).map_err(|error| error.to_string())?;
    println!("ok archive_dir={}", archive_dir.display());
    Ok(())
}
