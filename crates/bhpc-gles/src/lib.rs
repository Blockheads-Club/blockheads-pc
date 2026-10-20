//! OpenGL ES 2.0 and EGL for the Blockheads guest (PLAN section 1.5).
//!
//! The guest libraries (`libgles_apportable`, `libApplication`) import 380 `gl*` functions and
//! `eglGetCurrentContext`. This crate binds all of them as host thunks:
//!
//! * the ES 2.0 core, the OES framebuffer/blend/depth aliases, vertex array objects, mapped buffers (a guest
//!   shadow flushed on unmap), 3D textures, multi-draw and the fixed-point entry points that map onto ES2 are
//!   marshalled to a host GLES driver through the [`GlApi`] trait. Guest pointers become host pointers
//!   (`window base + address`) after a mapped-range check, so client-side vertex arrays, `glTexImage2D` pixels and
//!   output parameters are zero-copy; strings and shader sources are copied;
//! * the ES 1.x fixed pipeline and vendor extensions are accepted, counted and ignored ([`stubs`]);
//! * a complete EGL 1.4 subset is provided on top of a [`GlWindow`] (implemented by `bhpc-app` with SDL2).
//!
//! Integration (see `README.md`): build a [`Gles`], then bind guest imports with [`resolve`] / [`register_all`].
//!
//! Every thunk has the shape `fn(&mut Cpu, &Gles)`; the glue registers it as a re-entrant [`SharedThunkFn`].

pub mod api;
pub mod consts;
pub mod egl;
pub mod ext;
pub mod glsl;
pub mod imgsize;
pub mod marshal;
pub mod state;
pub mod stubs;
pub mod window;

#[cfg(test)]
mod tests_api;
#[cfg(test)]
mod tests_egl;
#[cfg(test)]
mod tests_gl;
#[cfg(test)]
mod tests_registry;
#[cfg(test)]
mod tests_support;

pub use api::{GlApi, HostGl};
pub use state::{es2_version, filter_extensions, Gles, GlesConfig, Stats};
pub use window::{GlWindow, HostContext, Overlay};

use api::{gl_list, FromGuest, ToGuest};
use bhpc_cpu::{Cpu, SharedThunkFn};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// A guest thunk body.
pub type Handler = fn(&mut Cpu, &Gles);

/// Thunks of the `direct` functions of the master list (scalar arguments only).
#[allow(non_snake_case, unused_mut, unused_variables, clippy::let_unit_value)]
mod direct {
    use super::*;

    macro_rules! direct_thunk {
        (direct $name:ident ($($an:ident: $at:ty),*) $(-> $rt:ty)?) => {
            pub fn $name(c: &mut Cpu, g: &Gles) {
                let mut a = c.args();
                $(let $an = <$at as FromGuest>::take(&mut a, c);)*
                let r = g.lock().api.$name($($an),*);
                ToGuest::put(r, c);
            }
        };
        (host $name:ident ($($tt:tt)*) $(-> $rt:ty)?) => {};
    }
    macro_rules! direct_entry {
        (direct $name:ident) => {
            Some((stringify!($name), $name as Handler))
        };
        (host $name:ident) => {
            None
        };
    }
    macro_rules! gen_direct {
        ($($kind:ident fn $name:ident($($an:ident: $at:ty),*) $(-> $rt:ty)?;)*) => {
            $(direct_thunk!($kind $name ($($an: $at),*) $(-> $rt)?);)*
            pub const TABLE: &[Option<(&str, Handler)>] = &[$(direct_entry!($kind $name)),*];
        };
    }
    gl_list!(gen_direct);
}

enum Kind {
    Fn(Handler),
    Ignored,
    Failing,
}

fn registry() -> &'static HashMap<&'static str, Kind> {
    static REG: OnceLock<HashMap<&'static str, Kind>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut m = HashMap::new();
        let tables: [&[(&str, Handler)]; 3] = [marshal::TABLE, ext::TABLE, egl::TABLE];
        for (n, h) in direct::TABLE.iter().flatten().chain(tables.iter().flat_map(|t| t.iter())) {
            m.insert(*n, Kind::Fn(*h));
        }
        for (alias, target) in ext::ALIASES {
            if let Some(Kind::Fn(h)) = m.get(target) {
                let h = *h;
                m.insert(alias, Kind::Fn(h));
            }
        }
        for n in stubs::IGNORED {
            m.insert(n, Kind::Ignored);
        }
        for n in stubs::FAILING {
            m.insert(n, Kind::Failing);
        }
        m
    })
}

/// Every `gl*`/`egl*` symbol this crate binds, sorted.
pub fn names() -> Vec<&'static str> {
    let mut v: Vec<_> = registry().keys().copied().collect();
    v.sort_unstable();
    v
}

/// True if `name` is a GL/EGL function this crate provides (including ignored stubs).
pub fn provides(name: &str) -> bool {
    registry().contains_key(name)
}

/// The thunk for `name`: counts the call, runs the handler and (with `check_errors`) drains the host error queue.
pub fn builtin(name: &str, gles: &Gles) -> Option<SharedThunkFn> {
    let (key, kind) = registry().get_key_value(name)?;
    let (key, g) = (*key, gles.clone());
    Some(match kind {
        Kind::Fn(h) => {
            let h = *h;
            Arc::new(move |c: &mut Cpu| {
                h(c, &g);
                g.lock().after_call(key);
            })
        }
        Kind::Ignored | Kind::Failing => {
            let failing = matches!(kind, Kind::Failing);
            Arc::new(move |c: &mut Cpu| {
                let mut s = g.lock();
                s.note_unsupported(key);
                if failing {
                    s.raise(consts::GL_INVALID_OPERATION);
                }
                s.stats.calls += 1;
                drop(s);
                c.ret_u32(0);
            })
        }
    })
}

/// Bind `name` to a guest address, creating the thunk once. `None` if `name` is not a GL/EGL function or the thunk
/// page is full.
pub fn resolve(cpu: &mut Cpu, gles: &Gles, name: &str) -> Option<u32> {
    if let Some(a) = cpu.thunk_addr(name) {
        return Some(a);
    }
    let f = builtin(name, gles)?;
    cpu.register_shared_thunk(name, f).ok()
}

/// Pre-bind every function. Returns the count.
pub fn register_all(cpu: &mut Cpu, gles: &Gles) -> usize {
    names().into_iter().filter(|n| resolve(cpu, gles, n).is_some()).count()
}
