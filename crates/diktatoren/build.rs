//! Generates the model catalog from audio.cpp's model specs and places the audio.cpp shared
//! library and bundled Silero VAD beside the binary.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const TASKS: [&str; 3] = ["asr", "vad", "wake_word"];

fn main() {
    let source = PathBuf::from(env::var("DEP_AUDIOCPP_SOURCE_DIR").expect("audiocpp source dir"));
    let lib_dir = PathBuf::from(env::var("DEP_AUDIOCPP_LIB_DIR").expect("audiocpp lib dir"));
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let specs = source.join("model_specs");
    println!("cargo:rerun-if-changed={}", specs.display());

    fs::write(out.join("catalog.json"), catalog(&specs).to_string()).unwrap();

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    match target_os.as_str() {
        "linux" => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN"),
        "macos" => println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path"),
        _ => {}
    }
    // target/<profile>/build/<pkg>/out -> target/<profile>
    if let Some(profile_dir) = out.ancestors().nth(3) {
        copy_libraries(&lib_dir, profile_dir, &target_os);
        copy_dir(
            &source.join("assets/framework/models/silero_vad"),
            &profile_dir.join("models/silero_vad"),
        );
    }
}

/// One entry per family, with only the packages that can be downloaded as-is.
fn catalog(specs: &Path) -> Value {
    let mut paths: Vec<_> = fs::read_dir(specs)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    let mut families = Vec::new();
    for path in paths {
        let spec: Value = match serde_json::from_str(&fs::read_to_string(&path).unwrap()) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let tasks: Vec<&str> = spec["tasks"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !tasks.iter().any(|t| TASKS.contains(t)) {
            continue;
        }
        let default_download = &spec["package_defaults"]["download"];
        let packages: Vec<Value> = spec["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let download = if p["download"].is_object() { &p["download"] } else { default_download };
                if download["kind"] != "huggingface_snapshot" || download["gated"] == true {
                    return None;
                }
                if p["format"] != "gguf" {
                    return None;
                }
                let or = |v: &Value, fallback: &Value| if v.is_string() { v.clone() } else { fallback.clone() };
                Some(json!({
                    "id": p["id"],
                    "name": or(&p["display_name"], &p["id"]),
                    "default": p["default"].as_bool().unwrap_or(false),
                    "precision": p["precision"],
                    "directory": or(&p["target_directory"], &json!("")),
                    "files": p["files"],
                    "strip_prefix": p["strip_prefix"],
                    "repo": download["repo"],
                    "revision": or(&download["revision"], &json!("main")),
                }))
            })
            .collect();
        if packages.is_empty() {
            continue;
        }
        families.push(json!({
            "family": spec["family"],
            "name": if spec["display_name"].is_string() { &spec["display_name"] } else { &spec["family"] },
            "description": spec["description"].as_str().unwrap_or_default(),
            "tasks": tasks,
            "modes": spec["modes"],
            "languages": spec["languages"],
            "packages": packages,
        }));
    }
    Value::Array(families)
}

fn is_library(name: &str, target_os: &str) -> bool {
    match target_os {
        "windows" => name.ends_with(".dll"),
        "macos" => name.ends_with(".dylib"),
        _ => name.contains(".so"),
    }
}

fn copy_libraries(from: &Path, to: &Path, target_os: &str) {
    for entry in fs::read_dir(from).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if is_library(&name.to_string_lossy(), target_os) {
            let dest = to.join(&name);
            let _ = fs::remove_file(&dest);
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).into_iter().flatten().flatten() {
        let dest = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}
