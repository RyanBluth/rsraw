use std::{
    env, fs,
    path::{Path, PathBuf},
};

const LIBRAW_DIR: &str = "LibRaw";

fn main() {
    let dir = env::var_os("OUT_DIR").unwrap();
    // Any change to LibRaw's sources, private headers or public headers must
    // trigger a rebuild. (bindgen's CargoCallbacks only track headers it sees,
    // which disables Cargo's default whole-package tracking.)
    for dir in ["src", "internal", "libraw"] {
        println!("cargo:rerun-if-changed={LIBRAW_DIR}/{dir}");
    }
    println!("cargo:rerun-if-changed=src/bindings.rs");
    println!("cargo:rerun-if-changed=build.rs");

    bindings(&dir);
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
    let compiler = libraw.get_compiler();
    if compiler.is_like_msvc() {
        panic!("MSVC is not supported");
    }

    libraw.cpp(true);
    libraw.include(LIBRAW_DIR);
    libraw.files(libraw_sources());

    libraw.warnings(false);
    libraw.extra_warnings(false);
    // do I really have to supress all of these?
    libraw.flag_if_supported("-Wno-format-truncation");
    libraw.flag_if_supported("-Wno-unused-result");
    libraw.flag_if_supported("-Wno-format-overflow");

    // thread safety
    libraw.flag("-pthread");
    // cc emits the rustc-link-lib/search directives itself.
    libraw.compile("raw");
}

fn bindings(out_dir: impl AsRef<Path>) {
    let path = out_dir.as_ref().join("bindings.rs");
    let host = env::var("HOST").expect("HOST is not set");
    let target = env::var("TARGET").expect("TARGET is not set");

    // Cross builds (e.g. Android in CI) may not have a usable target sysroot
    // for clang/bindgen. Reuse the checked-in bindings in that case.
    if host != target {
        fs::copy("src/bindings.rs", &path).expect("Couldn't copy pregenerated bindings");
        return;
    }

    let bindings = bindgen::Builder::default()
        .header(format!("{LIBRAW_DIR}/libraw/libraw.h"))
        .use_core()
        .ctypes_prefix("libc")
        .generate_comments(true)
        .layout_tests(false)
        // API improvements
        .derive_eq(true)
        .no_partialeq("__darwin_pthread_handler_rec")
        .no_partialeq("_IO_FILE") // This is the unnamed struct with _close, _read, etc.
        .no_partialeq("_IO_cookie_io_functions_t")
        .no_partialeq("sigvec")
        .no_partialeq("libraw_callbacks_t")
        .no_partialeq("__sFILE")
        .size_t_is_usize(true)
        // these are never part of the API
        .blocklist_function("_.*")
        // consts creating duplications
        .blocklist_item("FP_NAN")
        .blocklist_item("FP_INFINITE")
        .blocklist_item("FP_ZERO")
        .blocklist_item("FP_SUBNORMAL")
        .blocklist_item("FP_NORMAL")
        // Rust doesn't support long double, and bindgen can't skip it
        // https://github.com/rust-lang/rust-bindgen/issues/1549
        .blocklist_function("acoshl")
        .blocklist_function("acosl")
        .blocklist_function("asinhl")
        .blocklist_function("asinl")
        .blocklist_function("atan2l")
        .blocklist_function("atanhl")
        .blocklist_function("atanl")
        .blocklist_function("cbrtl")
        .blocklist_function("ceill")
        .blocklist_function("copysignl")
        .blocklist_function("coshl")
        .blocklist_function("cosl")
        .blocklist_function("dreml")
        .blocklist_function("ecvt_r")
        .blocklist_function("erfcl")
        .blocklist_function("erfl")
        .blocklist_function("exp2l")
        .blocklist_function("expl")
        .blocklist_function("expm1l")
        .blocklist_function("fabsl")
        .blocklist_function("fcvt_r")
        .blocklist_function("fdiml")
        .blocklist_function("finitel")
        .blocklist_function("floorl")
        .blocklist_function("fmal")
        .blocklist_function("fmaxl")
        .blocklist_function("fminl")
        .blocklist_function("fmodl")
        .blocklist_function("frexpl")
        .blocklist_function("gammal")
        .blocklist_function("hypotl")
        .blocklist_function("ilogbl")
        .blocklist_function("isinfl")
        .blocklist_function("isnanl")
        .blocklist_function("j0l")
        .blocklist_function("j1l")
        .blocklist_function("jnl")
        .blocklist_function("ldexpl")
        .blocklist_function("lgammal")
        .blocklist_function("lgammal_r")
        .blocklist_function("llrintl")
        .blocklist_function("llroundl")
        .blocklist_function("log10l")
        .blocklist_function("log1pl")
        .blocklist_function("log2l")
        .blocklist_function("logbl")
        .blocklist_function("logl")
        .blocklist_function("lrintl")
        .blocklist_function("lroundl")
        .blocklist_function("modfl")
        .blocklist_function("nanl")
        .blocklist_function("nearbyintl")
        .blocklist_function("nextafterl")
        .blocklist_function("nexttoward")
        .blocklist_function("nexttowardf")
        .blocklist_function("nexttowardl")
        .blocklist_function("powl")
        .blocklist_function("qecvt")
        .blocklist_function("qecvt_r")
        .blocklist_function("qfcvt")
        .blocklist_function("qfcvt_r")
        .blocklist_function("qgcvt")
        .blocklist_function("remainderl")
        .blocklist_function("remquol")
        .blocklist_function("rintl")
        .blocklist_function("roundl")
        .blocklist_function("scalbl")
        .blocklist_function("scalblnl")
        .blocklist_function("scalbnl")
        .blocklist_function("significandl")
        .blocklist_function("sinhl")
        .blocklist_function("sinl")
        .blocklist_function("sqrtl")
        .blocklist_function("strtold")
        .blocklist_function("tanhl")
        .blocklist_function("tanl")
        .blocklist_function("tgammal")
        .blocklist_function("truncl")
        .blocklist_function("y0l")
        .blocklist_function("y1l")
        .blocklist_function("ynl")
        .generate()
        .expect("Unable to generate bindings");

    bindings
        .write_to_file(path)
        .expect("Couldn't write bindings!");
}
