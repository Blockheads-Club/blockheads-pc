// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows gives a program's main thread 1 MiB of stack (Linux and macOS: 8 MiB). `bhpc-run --init-only` runs the
//! game's constructors in the JIT on the main thread, so the runner asks for the same 8 MiB there.
fn main() {
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|e| e == "msvc");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|o| o == "windows");
    if windows {
        let arg = if msvc { "/STACK:8388608" } else { "-Wl,--stack,8388608" };
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
