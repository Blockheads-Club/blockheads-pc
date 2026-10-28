//! Client mod loader of Blockheads PC (milestone M6 of the mod system).
//!
//! - [`join`]: the 0xBC join flow against a server (mod-sdk `docs/protocol-0xbc.md`), as a state machine the
//!   `bhpc-run` glue drives from the game's networking hooks.
//! - [`session`]: the folder of mods one game session runs (the player's own, developer folders, a server's).
//! - [`voice`]: voice chat seams (HELLO features, push-to-talk events).
//! - [`host`]: the mod host of a session (bhmod-core: Luau, WASM) with the PC client's game bridge.
//!
//! Nothing here touches the guest: the glue (`bhpc-run`, play build) installs the ObjC hooks, runs everything on
//! the game's main thread and carries out the actions.

pub mod binds;
pub mod form;
pub mod host;
pub mod hud;
pub mod join;
pub mod overhead;
pub mod session;
pub mod ui;
pub mod view;
pub mod voice;

/// The loader version sent in `HELLO.loader`.
pub const LOADER_VERSION: &str = bhpc_launcher::mods::LOADER_VERSION;

/// `HELLO.platform` for this build.
pub fn platform() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        a => a,
    };
    format!("pc-{}-{arch}", std::env::consts::OS)
}

#[cfg(test)]
mod tests_form;
#[cfg(test)]
mod tests_host;
#[cfg(test)]
mod tests_hud;
#[cfg(test)]
mod tests_join;
#[cfg(all(test, feature = "jvm"))]
mod tests_jvm;
#[cfg(test)]
mod tests_session;
#[cfg(test)]
mod tests_voice;
#[cfg(test)]
mod tests_voicehost;

/// The `blockheads:api` version the client host implements.
pub const API_VERSION: &str = bhmod_core::API_VERSION;
#[cfg(test)]
mod tests_ui;
#[cfg(test)]
mod tests_view;
