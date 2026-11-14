//! Parse and validate every .wgsl given (files or folders) with naga 23.1 and naga 30.0.2,
//! and print where they disagree. A file that is a fragment (composed into a module at
//! runtime) fails both the same way; what matters is a file whose verdict changed.
use std::path::{Path, PathBuf};

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(p).unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if p.extension().is_some_and(|e| e == "wgsl") {
        out.push(p.to_path_buf());
    }
}

fn v23(src: &str) -> Result<(), String> {
    let m = naga23::front::wgsl::parse_str(src).map_err(|e| format!("parse: {}", e.message()))?;
    naga23::valid::Validator::new(naga23::valid::ValidationFlags::all(), naga23::valid::Capabilities::all())
        .validate(&m)
        .map(|_| ())
        .map_err(|e| format!("validate: {}", e.as_inner()))
}

fn v30(src: &str) -> Result<(), String> {
    let m = naga30::front::wgsl::parse_str(src).map_err(|e| format!("parse: {}", e.message()))?;
    naga30::valid::Validator::new(naga30::valid::ValidationFlags::all(), naga30::valid::Capabilities::all())
        .validate(&m)
        .map(|_| ())
        .map_err(|e| format!("validate: {}", e.as_inner()))
}

fn main() {
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        collect(Path::new(&a), &mut files);
    }
    let (mut both_ok, mut both_bad, mut changed) = (0, 0, 0);
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let (a, b) = (v23(&src), v30(&src));
        match (&a, &b) {
            (Ok(()), Ok(())) => both_ok += 1,
            (Err(x), Err(y)) => {
                both_bad += 1;
                if x != y {
                    println!("SAME-FAIL {}\n    23: {x}\n    30: {y}", f.display());
                } else {
                    println!("SAME-FAIL {}\n    {x}", f.display());
                }
            }
            _ => {
                changed += 1;
                println!("CHANGED {}\n    23: {a:?}\n    30: {b:?}", f.display());
            }
        }
    }
    println!("{} files: {both_ok} valid under both, {both_bad} invalid under both (fragments), {changed} changed verdict", files.len());
}
