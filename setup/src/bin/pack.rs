//! `pack <setup binary> <output> <version> <name>=<file>...`
//!
//! Writes the finished installer: the setup binary with the given files
//! appended as its payload (format in src/payload.rs).

#[allow(dead_code)]
#[path = "../payload.rs"]
mod payload;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: pack <setup binary> <output> <version> <name>=<file>...");
        return ExitCode::from(2);
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pack: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let stub = std::fs::read(&args[0]).map_err(|e| format!("{}: {e}", args[0]))?;
    if payload::locate(std::path::Path::new(&args[0])).map_err(|e| e.to_string())?.is_some() {
        return Err(format!("{} already carries a payload", args[0]));
    }
    let mut files = Vec::new();
    for spec in &args[3..] {
        let (name, path) = spec.split_once('=').ok_or(format!("expected <name>=<file>, got {spec}"))?;
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        files.push((name.to_string(), bytes));
    }
    let out = &args[1];
    let tmp = format!("{out}.part");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| format!("{tmp}: {e}"))?);
    payload::pack(&stub, &args[2], &files, &mut f).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, out).map_err(|e| format!("{out}: {e}"))?;
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    println!("{out}: {} files, {:.1} MB", files.len(), size as f64 / 1e6);
    Ok(())
}
