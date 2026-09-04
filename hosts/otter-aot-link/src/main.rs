//! otter-aot-link CLI — the b″ link driver's bin half (O-16c′ design §3.1; L-1a + L-1b +
//! L-4 surface).
//!
//! Subcommands (deterministic, network-free; every gate failure is a nonzero exit):
//!
//! ```text
//! otter-aot-link rename --mode import|export --input IN.wasm --output OUT.wasm
//!                       [--map-module OLD=NEW]... [--map-name OLD=NEW]... [--kebab]
//! otter-aot-link check --imports MANIFEST.json [--require-self-contained] ARTIFACT.wasm
//! otter-aot-link check --rename-invariance ORIGINAL.wasm --mode import|export ARTIFACT.wasm
//! otter-aot-link check --scratch --manifest M.json --lib-manifest L.json
//!                      --lib-artifact BPRIME.wasm ARTIFACT.wasm
//! otter-aot-link check --linked --manifest M.json ARTIFACT.wasm
//! otter-aot-link check --component --manifest M.json COMPONENT.wasm
//! otter-aot-link inject-marker --input BPRIME.wasm --output OUT.wasm
//!                              --manifest M.json --lib-manifest L.json
//! otter-aot-link shrink-exports --input IN.wasm --output OUT.wasm --keep NAME...
//! otter-aot-link lib-manifest --extract UNSTRIPPED.wasm --output SIDECAR.json
//!                             --otter-version V --rustc-version V
//! otter-aot-link lib-manifest --finalize STRIPPED.wasm --sidecar SIDECAR.json
//! ```
//!
//! Argument parsing is hand-rolled on purpose: the crate's dependency list is the §3.1
//! contract (+ the recorded sha2 deviation, Cargo.toml) — a CLI framework is outside it.
//! Exit codes: 0 = green, 1 = gate/validation red, 2 = usage or I/O error.

use std::process::ExitCode;

use otter_aot_link::marker::RegionShape;
use otter_aot_link::{
    RenameMaps, RenameMode, component, imports, invariance, lib_manifest, linked, marker, rename, schema,
    scratch, shrink,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("rename") => run_rename(&args[1..]),
        Some("check") => run_check(&args[1..]),
        Some("inject-marker") => run_inject_marker(&args[1..]),
        Some("shrink-exports") => run_shrink_exports(&args[1..]),
        Some("lib-manifest") => run_lib_manifest(&args[1..]),
        Some("--help" | "-h" | "help") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => usage_error(&format!("unknown subcommand `{other}`")),
        None => usage_error("missing subcommand"),
    }
}

const USAGE: &str = "\
otter-aot-link — O-16c' b\" link driver (L-1a + L-1b + L-4 surface)

USAGE:
  otter-aot-link rename --mode import|export --input IN.wasm --output OUT.wasm
                        [--map-module OLD=NEW]... [--map-name OLD=NEW]... [--kebab]
  otter-aot-link check --imports MANIFEST.json [--require-self-contained] ARTIFACT.wasm
  otter-aot-link check --rename-invariance ORIGINAL.wasm --mode import|export ARTIFACT.wasm
  otter-aot-link check --scratch --manifest M.json --lib-manifest L.json
                       --lib-artifact BPRIME.wasm ARTIFACT.wasm
  otter-aot-link check --linked --manifest M.json ARTIFACT.wasm
  otter-aot-link check --component --manifest M.json COMPONENT.wasm
  otter-aot-link inject-marker --input BPRIME.wasm --output OUT.wasm
                               --manifest M.json --lib-manifest L.json
  otter-aot-link shrink-exports --input IN.wasm --output OUT.wasm --keep NAME...
  otter-aot-link lib-manifest --extract UNSTRIPPED.wasm --output SIDECAR.json
                              --otter-version V --rustc-version V
  otter-aot-link lib-manifest --finalize STRIPPED.wasm --sidecar SIDECAR.json
";

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("otter-aot-link: usage error: {msg}");
    eprint!("{USAGE}");
    ExitCode::from(2)
}

fn split_pair(arg: &str, flag: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((old, new)) if !old.is_empty() && !new.is_empty() => Ok((old.to_string(), new.to_string())),
        _ => Err(format!("{flag} expects OLD=NEW, got `{arg}`")),
    }
}

fn read_file(path: &str) -> Result<Vec<u8>, ExitCode> {
    std::fs::read(path).map_err(|e| {
        eprintln!("otter-aot-link: cannot read `{path}`: {e}");
        ExitCode::from(2)
    })
}

fn run_rename(args: &[String]) -> ExitCode {
    let mut mode: Option<RenameMode> = None;
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut maps = RenameMaps::default();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        macro_rules! value {
            () => {
                match it.next() {
                    Some(v) => v,
                    None => return usage_error(&format!("{arg} expects a value")),
                }
            };
        }
        match arg.as_str() {
            "--mode" => {
                let v = value!();
                match RenameMode::parse(v) {
                    Some(m) => mode = Some(m),
                    None => return usage_error(&format!("--mode must be import|export, got `{v}`")),
                }
            }
            "--input" => input = Some(value!().clone()),
            "--output" => output = Some(value!().clone()),
            "--map-module" => match split_pair(value!(), "--map-module") {
                Ok((old, new)) => {
                    if maps.modules.insert(old.clone(), new).is_some() {
                        return usage_error(&format!("duplicate --map-module key `{old}`"));
                    }
                }
                Err(e) => return usage_error(&e),
            },
            "--map-name" => match split_pair(value!(), "--map-name") {
                Ok((old, new)) => {
                    if maps.names.insert(old.clone(), new).is_some() {
                        return usage_error(&format!("duplicate --map-name key `{old}`"));
                    }
                }
                Err(e) => return usage_error(&e),
            },
            "--kebab" => maps.kebab = true,
            other => return usage_error(&format!("unknown rename flag `{other}`")),
        }
    }
    let (Some(mode), Some(input), Some(output)) = (mode, input, output) else {
        return usage_error("rename requires --mode, --input and --output");
    };

    let bytes = match read_file(&input) {
        Ok(b) => b,
        Err(code) => return code,
    };
    match rename::rename(&bytes, mode, &maps) {
        Ok(out) => {
            if let Err(e) = std::fs::write(&output, &out) {
                eprintln!("otter-aot-link: cannot write `{output}`: {e}");
                return ExitCode::from(2);
            }
            println!(
                "otter-aot-link: rename OK ({mode:?} mode, {} module key(s), {} name key(s), \
                 kebab={}) — invariance + Validator green, {} bytes → `{output}`",
                maps.modules.len(),
                maps.names.len(),
                maps.kebab,
                out.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: rename FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_check(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--imports") => run_check_imports(&args[1..]),
        Some("--rename-invariance") => run_check_invariance(&args[1..]),
        Some("--scratch") => run_check_scratch(&args[1..]),
        Some("--linked") => run_check_linked(&args[1..]),
        Some("--component") => run_check_component(&args[1..]),
        _ => usage_error(
            "check requires --imports, --rename-invariance, --scratch, --linked or --component \
             as its first flag",
        ),
    }
}

fn run_check_imports(args: &[String]) -> ExitCode {
    let mut manifest_path: Option<String> = None;
    let mut artifact_path: Option<String> = None;
    let mut require_self_contained = false;
    for arg in args {
        match arg.as_str() {
            "--require-self-contained" => require_self_contained = true,
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown check --imports flag `{other}`"));
            }
            positional => {
                if manifest_path.is_none() {
                    manifest_path = Some(positional.to_string());
                } else if artifact_path.is_none() {
                    artifact_path = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(manifest_path), Some(artifact_path)) = (manifest_path, artifact_path) else {
        return usage_error("check --imports requires MANIFEST.json and ARTIFACT.wasm");
    };
    let manifest_bytes = match read_file(&manifest_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let artifact = match read_file(&artifact_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let manifest = match schema::manifest_from_json(&manifest_bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("otter-aot-link: check --imports FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    match imports::check_imports(&artifact, &manifest, require_self_contained) {
        Ok(report) => {
            println!(
                "otter-aot-link: check --imports OK — residual set == manifest otter_host set \
                 ({} host import(s){})",
                report.matched_hosts,
                if report.self_contained {
                    ", self-contained"
                } else {
                    ""
                }
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: check --imports FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_check_invariance(args: &[String]) -> ExitCode {
    let mut original: Option<String> = None;
    let mut mode: Option<RenameMode> = None;
    let mut artifact: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--mode" => {
                let Some(v) = it.next() else {
                    return usage_error("--mode expects a value");
                };
                match RenameMode::parse(v) {
                    Some(m) => mode = Some(m),
                    None => return usage_error(&format!("--mode must be import|export, got `{v}`")),
                }
            }
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown check --rename-invariance flag `{other}`"));
            }
            positional => {
                if original.is_none() {
                    original = Some(positional.to_string());
                } else if artifact.is_none() {
                    artifact = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(original), Some(mode), Some(artifact)) = (original, mode, artifact) else {
        return usage_error("check --rename-invariance requires ORIGINAL.wasm, --mode and ARTIFACT.wasm");
    };
    let orig_bytes = match read_file(&original) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let art_bytes = match read_file(&artifact) {
        Ok(b) => b,
        Err(code) => return code,
    };
    match invariance::assert_invariance(&orig_bytes, &art_bytes, mode) {
        Ok(()) => {
            println!(
                "otter-aot-link: check --rename-invariance OK ({mode:?} mode — only the \
                 allowed section differs)"
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: check --rename-invariance FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Take the flag's value or return a usage error.
macro_rules! flag_value {
    ($it:ident, $flag:expr) => {
        match $it.next() {
            Some(v) => v.clone(),
            None => return usage_error(&format!("{} expects a value", $flag)),
        }
    };
}

fn run_check_scratch(args: &[String]) -> ExitCode {
    let mut manifest_path: Option<String> = None;
    let mut lib_manifest_path: Option<String> = None;
    let mut lib_artifact_path: Option<String> = None;
    let mut artifact_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--manifest" => manifest_path = Some(flag_value!(it, arg)),
            "--lib-manifest" => lib_manifest_path = Some(flag_value!(it, arg)),
            "--lib-artifact" => lib_artifact_path = Some(flag_value!(it, arg)),
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown check --scratch flag `{other}`"));
            }
            positional => {
                if artifact_path.is_none() {
                    artifact_path = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(manifest_path), Some(lib_manifest_path), Some(lib_artifact_path), Some(artifact_path)) =
        (manifest_path, lib_manifest_path, lib_artifact_path, artifact_path)
    else {
        return usage_error(
            "check --scratch requires --manifest, --lib-manifest, --lib-artifact and ARTIFACT.wasm",
        );
    };
    let (manifest, sidecar) = match load_sidecars(&manifest_path, &lib_manifest_path) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let (artifact, lib_artifact) = match (read_file(&artifact_path), read_file(&lib_artifact_path)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    match scratch::check_scratch(&artifact, &manifest, &sidecar, &lib_artifact) {
        Ok(report) => {
            // §2-1: the empty-region branch is EXPLICITLY logged, never a silent pass.
            println!(
                "otter-aot-link: check --scratch OK — {}; markers {}, memory min {} page(s) \
                 (required {}), base 0x{:x} >= heap_floor 0x{:x}",
                report.shape.describe(),
                report.markers,
                report.min_pages,
                report.required_pages,
                report.base,
                report.heap_floor
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: check --scratch FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_check_linked(args: &[String]) -> ExitCode {
    let mut manifest_path: Option<String> = None;
    let mut artifact_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--manifest" => manifest_path = Some(flag_value!(it, arg)),
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown check --linked flag `{other}`"));
            }
            positional => {
                if artifact_path.is_none() {
                    artifact_path = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(manifest_path), Some(artifact_path)) = (manifest_path, artifact_path) else {
        return usage_error("check --linked requires --manifest and ARTIFACT.wasm");
    };
    let manifest_bytes = match read_file(&manifest_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let artifact = match read_file(&artifact_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let manifest = match schema::manifest_from_json(&manifest_bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("otter-aot-link: check --linked FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    match linked::check_linked(&artifact, &manifest) {
        Ok(report) => {
            // §5.3: the start disposition is logged EXPLICITLY (the start check's first live
            // run is an L-4 gate item — never a silent pass).
            println!(
                "otter-aot-link: check --linked OK — single owned memory (shared={} == \
                 manifest, memory64={} == manifest), otter_entry+otter_table exported \
                 ({} export(s)), table min==max=={}, element items {}, EH census 0, {}",
                report.shared_memory,
                report.memory64,
                report.exports,
                report.table_size,
                report.element_items,
                report.start.describe()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: check --linked FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_check_component(args: &[String]) -> ExitCode {
    let mut manifest_path: Option<String> = None;
    let mut artifact_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--manifest" => manifest_path = Some(flag_value!(it, arg)),
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown check --component flag `{other}`"));
            }
            positional => {
                if artifact_path.is_none() {
                    artifact_path = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(manifest_path), Some(artifact_path)) = (manifest_path, artifact_path) else {
        return usage_error("check --component requires --manifest and COMPONENT.wasm");
    };
    let manifest_bytes = match read_file(&manifest_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let artifact = match read_file(&artifact_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let manifest = match schema::manifest_from_json(&manifest_bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("otter-aot-link: check --component FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    match component::check_component(&artifact, &manifest) {
        Ok(report) => {
            println!(
                "otter-aot-link: check --component OK — world exports [{}], imports [{}] \
                 (== manifest otter_host kebab image), {} nested core module(s), EH census 0, \
                 native EH-free validation green",
                report.exports.join(", "),
                report.imports.join(", "),
                report.nested_modules
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: check --component FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_shrink_exports(args: &[String]) -> ExitCode {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut keep: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--input" => input = Some(flag_value!(it, arg)),
            "--output" => output = Some(flag_value!(it, arg)),
            "--keep" => {
                let v = flag_value!(it, arg);
                if !keep.insert(v.clone()) {
                    return usage_error(&format!("duplicate --keep name `{v}`"));
                }
            }
            other => return usage_error(&format!("unknown shrink-exports flag `{other}`")),
        }
    }
    let (Some(input), Some(output)) = (input, output) else {
        return usage_error("shrink-exports requires --input, --output and at least one --keep");
    };
    let bytes = match read_file(&input) {
        Ok(b) => b,
        Err(code) => return code,
    };
    match shrink::shrink_exports(&bytes, &keep) {
        Ok((out, report)) => {
            if let Err(e) = std::fs::write(&output, &out) {
                eprintln!("otter-aot-link: cannot write `{output}`: {e}");
                return ExitCode::from(2);
            }
            println!(
                "otter-aot-link: shrink-exports OK — kept [{}], dropped [{}] — export-mode \
                 invariance + Validator green, {} bytes -> `{output}`",
                report.kept.join(", "),
                report.dropped.join(", "),
                out.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: shrink-exports FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_inject_marker(args: &[String]) -> ExitCode {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut lib_manifest_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--input" => input = Some(flag_value!(it, arg)),
            "--output" => output = Some(flag_value!(it, arg)),
            "--manifest" => manifest_path = Some(flag_value!(it, arg)),
            "--lib-manifest" => lib_manifest_path = Some(flag_value!(it, arg)),
            other => return usage_error(&format!("unknown inject-marker flag `{other}`")),
        }
    }
    let (Some(input), Some(output), Some(manifest_path), Some(lib_manifest_path)) =
        (input, output, manifest_path, lib_manifest_path)
    else {
        return usage_error("inject-marker requires --input, --output, --manifest and --lib-manifest");
    };
    let (manifest, sidecar) = match load_sidecars(&manifest_path, &lib_manifest_path) {
        Ok(pair) => pair,
        Err(code) => return code,
    };
    let bytes = match read_file(&input) {
        Ok(b) => b,
        Err(code) => return code,
    };
    match marker::inject_markers(&bytes, &sidecar, &manifest) {
        Ok((out, report)) => {
            if let Err(e) = std::fs::write(&output, &out) {
                eprintln!("otter-aot-link: cannot write `{output}`: {e}");
                return ExitCode::from(2);
            }
            // §2-1: every branch is logged — the W==0 no-op explicitly so (not silent).
            println!(
                "otter-aot-link: inject-marker OK — {}; {} marker(s) injected, memory min \
                 {} -> {} page(s), {} bytes -> `{output}`",
                report.shape.describe(),
                report.injected,
                report.old_min,
                report.new_min,
                out.len()
            );
            if report.shape == RegionShape::Empty {
                println!(
                    "otter-aot-link: inject-marker NOTE — empty region (W==0): output is the \
                     input byte-for-byte (call-free program shape, §2-1)"
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("otter-aot-link: inject-marker FAILED: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_lib_manifest(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--extract") => run_lib_extract(&args[1..]),
        Some("--finalize") => run_lib_finalize(&args[1..]),
        _ => usage_error("lib-manifest requires --extract or --finalize as its first flag"),
    }
}

fn run_lib_extract(args: &[String]) -> ExitCode {
    let mut artifact_path: Option<String> = None;
    let mut output: Option<String> = None;
    let mut otter_version: Option<String> = None;
    let mut rustc_version: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--output" => output = Some(flag_value!(it, arg)),
            "--otter-version" => otter_version = Some(flag_value!(it, arg)),
            "--rustc-version" => rustc_version = Some(flag_value!(it, arg)),
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown lib-manifest --extract flag `{other}`"));
            }
            positional => {
                if artifact_path.is_none() {
                    artifact_path = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(artifact_path), Some(output), Some(otter_version), Some(rustc_version)) =
        (artifact_path, output, otter_version, rustc_version)
    else {
        // Version fields are record-only (§4.3) but never fabricated: the recipe must say them.
        return usage_error(
            "lib-manifest --extract requires UNSTRIPPED.wasm, --output, --otter-version and \
             --rustc-version",
        );
    };
    let bytes = match read_file(&artifact_path) {
        Ok(b) => b,
        Err(code) => return code,
    };
    let sidecar = match lib_manifest::extract(&bytes, &otter_version, &rustc_version) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("otter-aot-link: lib-manifest --extract FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    let json = match schema::library_to_json(&sidecar) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("otter-aot-link: lib-manifest --extract FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(&output, &json) {
        eprintln!("otter-aot-link: cannot write `{output}`: {e}");
        return ExitCode::from(2);
    }
    println!(
        "otter-aot-link: lib-manifest --extract OK — heap_floor 0x{:x} (shared={}, min {} \
         page(s)), {} export(s), artifact_sha256 UNBOUND (run --finalize on the strip-final \
         bytes) -> `{output}`",
        sidecar.heap_floor,
        sidecar.shared,
        sidecar.memory_min,
        sidecar.exports.len()
    );
    ExitCode::SUCCESS
}

fn run_lib_finalize(args: &[String]) -> ExitCode {
    let mut artifact_path: Option<String> = None;
    let mut sidecar_path: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--sidecar" => sidecar_path = Some(flag_value!(it, arg)),
            other if other.starts_with('-') => {
                return usage_error(&format!("unknown lib-manifest --finalize flag `{other}`"));
            }
            positional => {
                if artifact_path.is_none() {
                    artifact_path = Some(positional.to_string());
                } else {
                    return usage_error(&format!("unexpected extra argument `{positional}`"));
                }
            }
        }
    }
    let (Some(artifact_path), Some(sidecar_path)) = (artifact_path, sidecar_path) else {
        return usage_error("lib-manifest --finalize requires STRIPPED.wasm and --sidecar");
    };
    let (bytes, sidecar_bytes) = match (read_file(&artifact_path), read_file(&sidecar_path)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(code), _) | (_, Err(code)) => return code,
    };
    let sidecar = match schema::library_from_json(&sidecar_bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("otter-aot-link: lib-manifest --finalize FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    let finalized = match lib_manifest::finalize(&sidecar, &bytes) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("otter-aot-link: lib-manifest --finalize FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    let json = match schema::library_to_json(&finalized) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("otter-aot-link: lib-manifest --finalize FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = std::fs::write(&sidecar_path, &json) {
        eprintln!("otter-aot-link: cannot write `{sidecar_path}`: {e}");
        return ExitCode::from(2);
    }
    println!(
        "otter-aot-link: lib-manifest --finalize OK — artifact_sha256 {} bound over {} bytes \
         -> `{sidecar_path}`",
        finalized.artifact_sha256.as_deref().unwrap_or("?"),
        bytes.len()
    );
    ExitCode::SUCCESS
}

/// Load the (product manifest, library sidecar) JSON pair; usage-style exit on I/O errors,
/// gate-red exit on schema refusals.
fn load_sidecars(
    manifest_path: &str,
    lib_manifest_path: &str,
) -> Result<(schema::ManifestSidecar, schema::LibrarySidecar), ExitCode> {
    let manifest_bytes = read_file(manifest_path)?;
    let lib_bytes = read_file(lib_manifest_path)?;
    let manifest = schema::manifest_from_json(&manifest_bytes).map_err(|e| {
        eprintln!("otter-aot-link: manifest sidecar `{manifest_path}`: {e}");
        ExitCode::FAILURE
    })?;
    let sidecar = schema::library_from_json(&lib_bytes).map_err(|e| {
        eprintln!("otter-aot-link: library sidecar `{lib_manifest_path}`: {e}");
        ExitCode::FAILURE
    })?;
    Ok((manifest, sidecar))
}
