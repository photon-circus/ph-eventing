//! Guards the one feature combination Cargo cannot express.
//!
//! `portable-atomic-unsafe-assume-single-core` and
//! `portable-atomic-critical-section` select different portable-atomic
//! backends and cannot both be enabled. Cargo features are additive, so the
//! manifest cannot exclude them.
//!
//! This has to live in a build script rather than a `compile_error!` in
//! `src/lib.rs`. Both features forward straight to `portable-atomic`, whose own
//! `compile_error!` fires while the dependency compiles — before this crate is
//! compiled at all, so a guard in `lib.rs` is unreachable and never prints. A
//! build script does not depend on `portable-atomic`, so it runs regardless and
//! its message is actually seen.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    let single_core =
        std::env::var_os("CARGO_FEATURE_PORTABLE_ATOMIC_UNSAFE_ASSUME_SINGLE_CORE").is_some();
    let critical_section =
        std::env::var_os("CARGO_FEATURE_PORTABLE_ATOMIC_CRITICAL_SECTION").is_some();

    if single_core && critical_section {
        println!(
            "cargo::error=features `portable-atomic-unsafe-assume-single-core` and \
             `portable-atomic-critical-section` select different portable-atomic backends \
             and cannot both be enabled -- pick one."
        );
        println!(
            "cargo::error=this is why `--all-features` does not work for ph-eventing. Check \
             feature combinations individually, as scripts/ci.* does."
        );
    }
}
