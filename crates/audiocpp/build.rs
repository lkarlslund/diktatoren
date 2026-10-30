//! Builds the audio.cpp C API from the vendored submodule with CMake and links it dynamically.
//!
//! Only the model families listed in `DEFAULT_MODELS` are compiled in, which keeps the build
//! short and the library small. Override with `AUDIOCPP_MODELS` (comma separated CMake model
//! targets) and pass extra CMake arguments with `AUDIOCPP_CMAKE_ARGS` (space separated).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The speech recognition, voice activity and wake-word families Diktatoren offers.
const DEFAULT_MODELS: &str = "parakeet_tdt,hviske_asr_v6,nemotron_asr,moonshine_asr,qwen3_asr,\
crisperwhisper,canary_asr,cohere_asr,micro_wake_word,open_wake_word,firered_vad";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let source = env::var_os("AUDIOCPP_SOURCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../third_party/audio.cpp"));
    let source = source.canonicalize().unwrap_or(source);
    if !source.join("include/audiocpp.h").exists() {
        panic!(
            "audio.cpp sources not found at {}; run `git submodule update --init`",
            source.display()
        );
    }

    for var in ["AUDIOCPP_SOURCE_DIR", "AUDIOCPP_MODELS", "AUDIOCPP_CMAKE_ARGS"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    for path in ["include/audiocpp.h", "CMakeLists.txt", "src", "include", "model_specs"] {
        println!("cargo:rerun-if-changed={}", source.join(path).display());
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let models = env::var("AUDIOCPP_MODELS").unwrap_or_else(|_| DEFAULT_MODELS.to_string());
    let on = |b: bool| if b { "ON" } else { "OFF" };
    let feature = |name: &str| env::var_os(format!("CARGO_FEATURE_{}", name.to_uppercase())).is_some();

    let mut config = cmake::Config::new(&source);
    config
        .profile("Release")
        .define("AUDIOCPP_BUILD_C_API", "ON")
        .define("AUDIOCPP_MODEL_SET", "custom")
        .define("AUDIOCPP_MODELS", &models)
        .define("ENGINE_ENABLE_NATIVE_CPU", "OFF")
        .define("ENGINE_ENABLE_CUDA", on(feature("cuda")))
        .define("ENGINE_ENABLE_HIP", on(feature("hip")))
        .define("ENGINE_ENABLE_VULKAN", on(feature("vulkan")))
        .define("ENGINE_ENABLE_METAL", on(feature("metal") || target_os == "macos"))
        // Apple clang ships no OpenMP, and on Windows it would add a runtime DLL dependency.
        .define("ENGINE_ENABLE_OPENMP", on(target_os == "linux"))
        .build_target("audiocpp");
    if let Ok(extra) = env::var("AUDIOCPP_CMAKE_ARGS") {
        for arg in extra.split_whitespace() {
            config.configure_arg(arg);
        }
    }
    let dst = config.build();

    let bin = dst.join("build/bin");
    let lib_dir = [bin.join("Release"), bin.clone()]
        .into_iter()
        .find(|dir| has_library(dir, &target_os))
        .unwrap_or_else(|| panic!("libaudiocpp not found under {}", bin.display()));

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=audiocpp");
    // Exported to dependents as DEP_AUDIOCPP_LIB_DIR / DEP_AUDIOCPP_SOURCE_DIR.
    println!("cargo:lib_dir={}", lib_dir.display());
    println!("cargo:source_dir={}", source.display());
    if target_os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    } else if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    }

    // Tests and examples of this crate run from target/<profile>/deps; on Windows the DLL must
    // sit beside them.
    if target_os == "windows" {
        let out = PathBuf::from(env::var("OUT_DIR").unwrap());
        if let Some(profile_dir) = out.ancestors().nth(3) {
            copy_libraries(&lib_dir, &profile_dir.join("deps"), &target_os);
        }
    }
}

fn is_library(name: &str, target_os: &str) -> bool {
    match target_os {
        "windows" => name.ends_with(".dll"),
        "macos" => name.ends_with(".dylib"),
        _ => name.contains(".so"),
    }
}

fn has_library(dir: &Path, target_os: &str) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| is_library(&e.file_name().to_string_lossy(), target_os))
    })
}

fn copy_libraries(from: &Path, to: &Path, target_os: &str) {
    let _ = fs::create_dir_all(to);
    for entry in fs::read_dir(from).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if is_library(&name.to_string_lossy(), target_os) {
            let _ = fs::copy(entry.path(), to.join(&name));
        }
    }
}
