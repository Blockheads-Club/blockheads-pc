//! The built-in fake Java library: just enough of `java.*`, `android.*` and `com.apportable.*` for
//! the Blockheads libraries to boot. Anything else is invented on demand (see [`crate::Vm`]).

use crate::value::{Obj, Value};
use crate::vm::{Call, JResult, Vm};

mod android;
mod apportable;
pub(crate) mod desktop;
mod graphics;
mod intent;
mod io;
mod lang;
mod net;
mod services;
mod util;

pub(crate) fn void() -> JResult {
    Ok(Value::Void)
}

pub(crate) fn int(v: i32) -> JResult {
    Ok(Value::Int(v))
}

pub(crate) fn boolean(v: bool) -> JResult {
    Ok(Value::Int(v as i32))
}

pub(crate) fn float(v: f32) -> JResult {
    Ok(Value::Float(v))
}

pub(crate) fn long(v: i64) -> JResult {
    Ok(Value::Long(v))
}

pub(crate) fn object(o: Option<Obj>) -> JResult {
    Ok(Value::Ref(o))
}

pub(crate) fn string(c: &Call, s: &str) -> JResult {
    Ok(Value::Ref(Some(c.vm.new_string(s))))
}

/// Install every built-in class into `vm`.
pub(crate) fn install(vm: &Vm) {
    lang::install(vm);
    util::install(vm);
    io::install(vm);
    android::install(vm);
    graphics::install(vm);
    apportable::install(vm);
    net::install(vm);
    desktop::install(vm);
    intent::install(vm);
    services::install(vm);
    crate::looper::install(vm);
}

#[cfg(test)]
pub(crate) use apportable::WIDGETS;
pub(crate) use apportable::{activity, surface_view};
pub(crate) use graphics::{bitmap_info, bitmap_info_in, face_in, face_of, Face, BOLD, ITALIC};
pub use intent::{file_uri, uri_path, ActivityRequest, RESULT_CANCELED, RESULT_OK};
pub(crate) use lang::thread_object;
pub(crate) use services::Services;
pub use services::{BILLING_UNAVAILABLE, GAMES_CHECK_MS, GAMES_NOTICE, SERVICE_MISSING, STORE_NOTICE};
