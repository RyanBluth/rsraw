use std::{
    env,
    path::{Path, PathBuf},
};

const LIBRAW_DIR: &str = "LibRaw";

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // Any change to LibRaw's sources, private headers or public headers must
    // trigger a rebuild. (bindgen's CargoCallbacks only track headers it sees,
    // which disables Cargo's default whole-package tracking.)
    for dir in ["src", "internal", "libraw"] {
        println!("cargo:rerun-if-changed={LIBRAW_DIR}/{dir}");
    }
    println!("cargo:rerun-if-changed=src/bindings.rs");
    println!("cargo:rerun-if-changed=build.rs");

    bindings(&out_dir);
    build();
}

/// Every LibRaw translation unit, i.e. all `src/**/*.cpp` except the `*_ph.cpp`
/// placeholders, which are `#include`d by other files. This matches
/// `lib_libraw_a_SOURCES` in LibRaw's Makefile.am, so upgrading LibRaw does not
/// require editing a hard-coded list.
fn libraw_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("LibRaw source directory is missing") {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "cpp")
                && !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with("_ph.cpp"))
            {
                out.push(path);
            }
        }
    }
    let mut sources = Vec::new();
    walk(&Path::new(LIBRAW_DIR).join("src"), &mut sources);
    sources.sort();
    sources
}

fn build() {
    let mut libraw = cc::Build::new();
    let msvc = libraw.get_compiler().is_like_msvc();

    libraw.cpp(true);
    libraw.include(LIBRAW_DIR);
    libraw.files(libraw_sources());
    // Static library: no __declspec(dllimport/dllexport) on Windows.
    libraw.define("LIBRAW_NODLL", None);

    libraw.warnings(false);
    libraw.extra_warnings(false);
    if msvc {
        // LibRaw uses C++ exceptions for error handling.
        libraw.flag("/EHsc");
        libraw.define("_CRT_SECURE_NO_WARNINGS", None);
    } else {
        libraw.flag_if_supported("-Wno-format-truncation");
        libraw.flag_if_supported("-Wno-unused-result");
        libraw.flag_if_supported("-Wno-format-overflow");
        // Thread-safe (libraw_r) build.
        libraw.flag_if_supported("-pthread");
    }
    // cc emits the rustc-link-lib/search directives itself.
    libraw.compile("raw");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // ntohs/htonl; MSVC also honours LibRaw's #pragma comment(lib), GCC does not.
        println!("cargo:rustc-link-lib=ws2_32");
    }
}

/// The checked-in `src/bindings.rs` was generated for an LP64 Unix target
/// (64-bit `long` and `time_t`). Its struct layouts are wrong anywhere else,
/// e.g. on Windows where `long` is 32-bit but `time_t` is 64-bit.
fn pregenerated_bindings_match_target() -> bool {
    let family = env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let width = env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap_or_default();
    family.split(',').any(|f| f == "unix") && width == "64"
}

fn bindings(out_dir: &Path) {
    let path = out_dir.join("bindings.rs");
    let host = env::var("HOST").expect("HOST is not set");
    let target = env::var("TARGET").expect("TARGET is not set");
    let pregenerated_ok = pregenerated_bindings_match_target();

    // Cross builds (e.g. Android in CI) may not have a usable target sysroot
    // for clang, so reuse the checked-in bindings when their layout matches.
    let use_pregenerated = !cfg!(feature = "bindgen") || (host != target && pregenerated_ok);

    if use_pregenerated {
        if !pregenerated_ok {
            panic!(
                "rsraw-sys: the checked-in bindings only match 64-bit Unix targets; \
                 enable the `bindgen` feature (requires libclang) to build for {target}"
            );
        }
        std::fs::copy("src/bindings.rs", &path).expect("Couldn't copy pregenerated bindings");
        return;
    }

    #[cfg(feature = "bindgen")]
    generate_bindings(&path);
}

/// Regenerate `src/bindings.rs` on a 64-bit Linux host with:
/// `cargo build -p rsraw-sys && cp target/debug/build/rsraw-sys-*/out/bindings.rs rsraw-sys/src/`
#[cfg(feature = "bindgen")]
fn generate_bindings(path: &Path) {
    // bindgen passes `--target` from $TARGET to clang automatically.
    bindgen::Builder::default()
        .header(format!("{LIBRAW_DIR}/libraw/libraw.h"))
        .clang_arg("-DLIBRAW_NODLL")
        .use_core()
        .ctypes_prefix("libc")
        .generate_comments(true)
        // Only LibRaw's own API; libc/stdio/math re-exports clash with std.
        .allowlist_function("libraw_.*")
        .allowlist_type("libraw_.*|LibRaw_.*")
        .allowlist_var("LIBRAW_.*")
        // `FILE` only appears behind a pointer; keep glibc's layout out of the
        // checked-in bindings so they are valid on every LP64 Unix.
        .blocklist_type("FILE|_IO_.*|__off_t|__off64_t|__uint64_t")
        .raw_line("pub type FILE = libc::FILE;")
        // Check every struct's size/alignment/offsets against the C compiler.
        .layout_tests(true)
        .size_t_is_usize(true)
        .generate()
        .expect("Unable to generate bindings")
        .write_to_file(path)
        .expect("Couldn't write bindings!");
}
