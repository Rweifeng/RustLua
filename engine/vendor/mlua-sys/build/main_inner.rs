use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

cfg_if::cfg_if! {
    if #[cfg(any(feature = "luau", feature = "vendored"))] {
        #[path = "find_vendored.rs"]
        mod find;
    } else {
        #[path = "find_normal.rs"]
        mod find;
    }
}

fn main() {
    #[cfg(all(feature = "luau", feature = "module", windows))]
    compile_error!("Luau does not support `module` mode on Windows");

    #[cfg(any(
        all(feature = "vendored", any(feature = "external", feature = "module")),
        all(feature = "external", any(feature = "vendored", feature = "module")),
        all(feature = "module", any(feature = "vendored", feature = "external"))
    ))]
    compile_error!("`vendored`, `external` and `module` features are mutually exclusive");

    println!("cargo:rerun-if-changed=build");
    println!("cargo:rerun-if-changed=wasm-shim-c");

    // NB: use the *runtime* env vars — compile-time `cfg(target_arch = ...)`
    // describes the HOST, since this build script binary runs on the host.
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();

    if target_arch == "wasm32" && target_os == "unknown" {
        build_wasm_shim(&manifest);
    }

    // Check if compilation and linking is handled by external crate
    if cfg!(not(feature = "external")) {
        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
        if target_os == "windows" && cfg!(feature = "module") {
            if !std::env::var("LUA_LIB_NAME").unwrap_or_default().is_empty() {
                // Don't use raw-dylib linking
                find::probe_lua();
                return;
            }

            println!("cargo:rustc-cfg=raw_dylib");
        }

        #[cfg(not(feature = "module"))]
        find::probe_lua();
    }
}

/// wasm32-unknown-unknown: build the C half of the libc shim.
///
/// The C sources (setjmp/longjmp over the wasm exception-handling proposal,
/// the stdio/printf family, abort/exit) are compiled with
/// `zig cc --target=wasm32-freestanding` and archived into `liblua_shim.a`.
/// The link directives are emitted from THIS crate's build script: mlua-sys
/// has no `links` key, so cargo forwards `cargo:rustc-link-lib` to every
/// linker run that reaches mlua-sys (unlike a `links` crate, whose rlib is
/// dropped from the final link and whose -l directives are not forwarded).
///
/// lua-src's build code (which runs IN-PROCESS, see find_vendored.rs) reads
/// the shim include dir from `DEP_WASM_LUA_SHIM_INCLUDE_DIR`; set it here.
fn build_wasm_shim(manifest: &Path) {
    let include_dir = manifest.join("wasm-shim-c").join("include");
    let csrc_dir = manifest.join("wasm-shim-c").join("csrc");

    // SAFETY: single-threaded build script; setting an env var before
    // lua-src's build code (same process) reads it.
    unsafe {
        env::set_var("DEP_WASM_LUA_SHIM_INCLUDE_DIR", &include_dir);
    }

    let zig = find_zig();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let obj_dir = out_dir.join("obj");
    std::fs::create_dir_all(&obj_dir).expect("create obj dir");

    let mut objs = Vec::new();
    for c in ["sjlj.c", "stdio.c"] {
        let src = csrc_dir.join(c);
        let obj = obj_dir.join(format!("{}.o", c));
        run(
            &zig,
            &[
                "cc",
                "--target=wasm32-freestanding",
                "-mexception-handling",
                "-fwasm-exceptions",
                "-mllvm",
                "-wasm-enable-sjlj",
                "-fno-sanitize=undefined",
                "-O2",
                "-fno-common",
                "-I",
                &include_dir.display().to_string(),
                "-c",
                &src.display().to_string(),
                "-o",
                &obj.display().to_string(),
            ],
        );
        objs.push(obj);
    }

    let lib_dir = out_dir.join("lib");
    std::fs::create_dir_all(&lib_dir).expect("create lib dir");
    let archive = lib_dir.join("liblua_shim.a");
    let mut ar = Command::new(&zig);
    ar.arg("ar").arg("crs").arg(&archive);
    for o in &objs {
        ar.arg(o);
    }
    let status = ar.status().expect("run zig ar");
    assert!(status.success(), "zig ar failed");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static:-bundle=lua_shim");
}

fn find_zig() -> String {
    if let Ok(z) = env::var("ZIG") {
        if !z.trim().is_empty() {
            return z;
        }
    }
    // Fall back to PATH lookup of `zig`.
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            for name in ["zig.exe", "zig"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate.display().to_string();
                }
            }
        }
    }
    panic!(
        "mlua-sys: cannot locate `zig` (needed to compile the wasm32-unknown-unknown \
         libc shim C stubs). Set the `ZIG` env var, e.g. in engine/.cargo/config.toml: \
         [env] ZIG = \"<path-to-zig.exe>\""
    );
}

fn run(zig: &str, args: &[&str]) {
    let status = Command::new(zig).args(args).status().expect("run zig cc");
    assert!(status.success(), "zig cc failed: {args:?}");
}
