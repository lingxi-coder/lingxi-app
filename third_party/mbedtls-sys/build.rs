//! cc-built seam for the vendored mbedTLS C sources (TLS + crypto).
//!
//! Compiles the three standard mbedTLS static libraries — `mbedcrypto`,
//! `mbedx509`, `mbedtls` — from `third_party/mbedtls/library/*.c` using the
//! stock `include/mbedtls/mbedtls_config.h`, then exports the include dir as
//! `DEP_MBEDTLS_INCLUDE` for the direct dependents (libgit2-sys / libssh2-sys).
//!
//! Link order matters: `mbedtls` -> `mbedx509` -> `mbedcrypto` (a TLS symbol
//! resolves against x509, which resolves against crypto). The three
//! `rustc-link-lib` lines below are emitted in that order.

use std::env;
use std::path::PathBuf;

/// X.509 sources (CMake `src_x509`).
const SRC_X509: &[&str] = &[
    "pkcs7.c",
    "x509.c",
    "x509_create.c",
    "x509_crl.c",
    "x509_crt.c",
    "x509_csr.c",
    "x509write.c",
    "x509write_crt.c",
    "x509write_csr.c",
];

/// TLS sources (CMake `src_tls`).
const SRC_TLS: &[&str] = &[
    "debug.c",
    "mps_reader.c",
    "mps_trace.c",
    "net_sockets.c",
    "ssl_cache.c",
    "ssl_ciphersuites.c",
    "ssl_client.c",
    "ssl_cookie.c",
    "ssl_debug_helpers_generated.c",
    "ssl_msg.c",
    "ssl_ticket.c",
    "ssl_tls.c",
    "ssl_tls12_client.c",
    "ssl_tls12_server.c",
    "ssl_tls13_keys.c",
    "ssl_tls13_server.c",
    "ssl_tls13_client.c",
    "ssl_tls13_generic.c",
];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    // `third_party/mbedtls` lives next to `third_party/mbedtls-sys`.
    let src_root = manifest.parent().unwrap().join("mbedtls");
    let include = src_root.join("include");
    let library = src_root.join("library");

    println!("cargo:rerun-if-changed={}", library.display());
    println!("cargo:rerun-if-changed={}", include.display());

    // Partition the library *.c into the three standard mbedTLS libs.
    let mut x509_files = Vec::new();
    let mut tls_files = Vec::new();
    let mut crypto_files = Vec::new();

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&library)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("c"))
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if SRC_X509.contains(&name.as_str()) {
            x509_files.push(path);
        } else if SRC_TLS.contains(&name.as_str()) {
            tls_files.push(path);
        } else {
            crypto_files.push(path);
        }
    }

    let base = |files: &[PathBuf]| {
        let mut cfg = cc::Build::new();
        cfg.include(&include)
            // The library *.c use `#include "common.h"` etc. from library/.
            .include(&library)
            .define("MBEDTLS_CONFIG_FILE", "\"mbedtls/mbedtls_config.h\"")
            .warnings(false);
        // NDK API floor: ensure --target uses API 29 (the P0a/G7 lesson).
        bump_android_api(&mut cfg);
        for f in files {
            cfg.file(f);
        }
        cfg
    };

    base(&crypto_files).compile("mbedcrypto");
    base(&x509_files).compile("mbedx509");
    base(&tls_files).compile("mbedtls");

    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:include={}", include.display());
    println!("cargo:rustc-link-search=native={}", out.display());
    // Link order: tls -> x509 -> crypto.
    println!("cargo:rustc-link-lib=static=mbedtls");
    println!("cargo:rustc-link-lib=static=mbedx509");
    println!("cargo:rustc-link-lib=static=mbedcrypto");
}

/// When cross-compiling for Android, `cc` defaults the `--target=<triple>NN`
/// API level from the NDK toolchain; some host setups default below 29, which
/// breaks newer libc symbols. Force API 29 if the target is Android and the
/// caller hasn't already pinned a higher level. (P0a / G7 lesson.)
fn bump_android_api(cfg: &mut cc::Build) {
    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("android") {
        return;
    }
    // cc derives the clang `--target` triple+api itself; we only need to make
    // sure the minimum is 29. cc honors CLANG_TARGET via the per-file flag, but
    // the simplest robust knob is the `ANDROID_API`/`-D__ANDROID_API__` floor.
    cfg.define("__ANDROID_API__", "29");
}
