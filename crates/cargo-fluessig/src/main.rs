//! `cargo fluessig emit` — the exporter subcommand for the derive front end.
//!
//! It runs the target crate's exporter bin (a one-liner over the
//! `catalog!`-generated `to_json()`; `fluessig-emit` by convention) and writes
//! its stdout to a file. This replaces `(cd emitter && node emit.mjs …)` for a
//! crate whose schema is authored with `#[derive(Entity)]`.
//!
//! Slice 1 keeps this deliberately thin: it does not itself understand the
//! schema — it shells out to the crate's own exporter, exactly the "bin target"
//! path from `derive-front-end.md` §2.8. `api.json` emission is Slice 5.

use std::path::Path;
use std::process::{Command, Stdio};

fn main() {
    // Invoked as `cargo fluessig emit …` → argv is
    // ["cargo-fluessig", "fluessig", "emit", …]. Drop the injected subcommand
    // token so the arg parsing is the same whether run via cargo or directly.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("fluessig") {
        args.remove(0);
    }

    match args.first().map(String::as_str) {
        Some("emit") => emit(&args[1..]),
        Some("--help") | Some("-h") | None => {
            usage();
        }
        Some(other) => {
            eprintln!("cargo-fluessig: unknown subcommand `{other}`\n");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!(
        "cargo fluessig emit — write a derive-authored crate's catalog.json\n\
         \n\
         USAGE:\n\
         \x20   cargo fluessig emit [--bin <name>] [-o <path>] [--manifest-path <path>]\n\
         \n\
         OPTIONS:\n\
         \x20   --bin <name>            exporter bin to run (default: fluessig-emit)\n\
         \x20   -o, --out <path>        output file (default: catalog.json)\n\
         \x20   --manifest-path <path>  Cargo.toml of the crate to run\n"
    );
}

fn emit(args: &[String]) {
    let mut bin = "fluessig-emit".to_string();
    let mut out = "catalog.json".to_string();
    let mut manifest_path: Option<String> = None;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--bin" => bin = expect_value(&mut it, "--bin"),
            "-o" | "--out" => out = expect_value(&mut it, "--out"),
            "--manifest-path" => manifest_path = Some(expect_value(&mut it, "--manifest-path")),
            other => {
                eprintln!("cargo-fluessig emit: unexpected argument `{other}`");
                std::process::exit(2);
            }
        }
    }

    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["run", "--quiet", "--bin", &bin]);
    if let Some(mp) = &manifest_path {
        cmd.args(["--manifest-path", mp]);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());

    let output = cmd.output().unwrap_or_else(|e| {
        eprintln!("cargo-fluessig: failed to run cargo: {e}");
        std::process::exit(1);
    });
    if !output.status.success() {
        eprintln!("cargo-fluessig: exporter bin `{bin}` failed");
        std::process::exit(output.status.code().unwrap_or(1));
    }

    std::fs::write(&out, &output.stdout).unwrap_or_else(|e| {
        eprintln!("cargo-fluessig: write {out}: {e}");
        std::process::exit(1);
    });
    let path = Path::new(&out);
    println!("wrote {} ({} bytes)", path.display(), output.stdout.len());
}

fn expect_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> String {
    it.next().cloned().unwrap_or_else(|| {
        eprintln!("cargo-fluessig emit: {flag} needs a value");
        std::process::exit(2);
    })
}
