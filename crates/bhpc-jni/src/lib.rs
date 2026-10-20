//! Fake `JavaVM`/`JNIEnv` for the Blockheads guest libraries (PLAN 1.4).
//!
//! The Android game is a thin Java shell around native code. This crate provides that shell on
//! the PC: a small Java object model ([`Vm`]), the JNI function tables as host thunks
//! ([`Vm::install`]) and a built-in fake class library (`java.*`, `android.*`, `com.apportable.*`)
//! with PC values (screen, density, locale, directories, assets through [`Assets`]).
//!
//! See `README.md` for the integration steps.

pub mod build;
pub mod host;
pub mod invoke;
mod jlib;
pub mod jni;
pub mod looper;
pub mod mutf8;
pub mod platform;
mod render;
pub mod startup;
mod ui;
pub mod value;
pub mod vm;

pub use build::ClassBuilder;
pub use host::{install, Jni, JniError, Usage};
pub use invoke::Recv;
pub use jlib::{
    file_uri, uri_path, ActivityRequest, BILLING_UNAVAILABLE, GAMES_CHECK_MS, GAMES_NOTICE, RESULT_CANCELED, RESULT_OK,
    SERVICE_MISSING, STORE_NOTICE,
};
pub use jni::{Layout, JNI_VERSION, TABLE_SLOTS};
pub use platform::{
    Assets, AudioHooks, GfxHooks, MemAssets, MusicOp, NoAssets, NoAudio, NoGfx, NoSystem, Platform, SystemHooks,
    TrackOp,
};
pub use render::dialog::DialogNav;
pub use render::UiFrame;
pub use startup::{native_env, Stage};
pub use ui::{Hit, TouchPhase};
pub use value::{ClassId, Obj, Sig, Ty, Value};
pub use vm::{Alert, Call, Data, GuestAlloc, JResult, Kind, MemberReport, Report, Throw, Vm, VmConfig};

use bhpc_mem::GuestMemory;
use std::sync::Arc;

/// A VM with the built-in fake Java library installed.
pub fn new_vm(mem: Arc<GuestMemory>, alloc: Box<dyn GuestAlloc>, cfg: VmConfig) -> Vm {
    let vm = Vm::bare(mem, alloc, cfg);
    jlib::install(&vm);
    vm
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_desktop;
#[cfg(test)]
mod tests_host;
#[cfg(test)]
mod tests_jni;
#[cfg(test)]
mod tests_lib;
#[cfg(test)]
mod tests_looper;
#[cfg(test)]
mod tests_music;
#[cfg(test)]
mod tests_net;
#[cfg(test)]
mod tests_render;
#[cfg(test)]
mod tests_services;
#[cfg(test)]
mod tests_startup;
#[cfg(test)]
mod tests_ui;
#[cfg(test)]
mod tests_vm;
