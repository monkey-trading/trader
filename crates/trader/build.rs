//! Embeds the console's build, `console/dist`, in the binary: writes `console_assets.rs` into
//! `OUT_DIR`, listing each built file by its URL path with its bytes. Without a build the list is
//! empty, and the bot serves a page saying so.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Lists the console's built files.
fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let console = manifest.join("../../console");
    let dist = console.join("dist");
    let mut files = Vec::new();
    // Cargo counts a missing path as changed on every build, so without a build it watches the
    // console's directory, in which one appears.
    if dist.is_dir() {
        println!("cargo::rerun-if-changed={}", dist.display());
        collect(&dist, &dist, &mut files);
    } else {
        println!("cargo::rerun-if-changed={}", console.display());
    }
    files.sort();
    let mut code =
        String::from("/// The console's built files, by URL path.\npub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (url, path) in &files {
        writeln!(code, "    ({url:?}, include_bytes!({:?})),", path.display()).expect("writing to a String");
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR")).join("console_assets.rs");
    // Rewritten only when it changes, so a rerun that finds the same files rebuilds nothing.
    if std::fs::read_to_string(&out).ok().as_deref() != Some(code.as_str()) {
        std::fs::write(&out, code).expect("writing console_assets.rs");
    }
}
