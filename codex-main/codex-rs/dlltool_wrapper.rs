use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Replacement for GNU `dlltool` that uses LLVM's `rust-lld` (lld-link mode)
/// to create import libraries from .def files — no `as.exe` required.
fn main() {
    let args: Vec<String> = env::args().collect();
    let mut def_file = String::new();
    let mut output_lib = String::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-d" | "--def" => {
                if i + 1 < args.len() {
                    def_file = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            "-l" | "--output-lib" => {
                if i + 1 < args.len() {
                    output_lib = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    if def_file.is_empty() || output_lib.is_empty() {
        eprintln!("dlltool-ll: missing -d or -l argument");
        std::process::exit(1);
    }

    let exe_dir = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    // rust-lld.exe lives in the parent bin/ directory, not self-contained/
    let lld = exe_dir
        .parent()
        .map(|p| p.join("rust-lld.exe"))
        .unwrap_or_else(|| exe_dir.join("rust-lld.exe"));

    let status = Command::new(&lld)
        .arg("-flavor")
        .arg("link")
        .arg("/LIB")
        .arg("/MACHINE:X64")
        .arg(format!("/DEF:{}", def_file))
        .arg(format!("/OUT:{}", output_lib))
        .status();

    match status {
        Ok(s) if s.success() => std::process::exit(0),
        Ok(s) => {
            eprintln!("dlltool-ll: rust-lld failed with status {}", s);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("dlltool-ll: failed to run rust-lld: {}", e);
            std::process::exit(1);
        }
    }
}
