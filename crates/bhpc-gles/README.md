# bhpc-gles

OpenGL ES 2.0 and EGL for the guest (PLAN 1.5). GPL-3.0-only.

## What it provides

All 380 `gl*` imports of `libgles_apportable.so` / `libApplication.so` plus `eglGetCurrentContext` (and a full EGL 1.4
subset: display, one RGBA8888/D24S8/ES2 config, contexts with sharing, window surfaces, swap, swap interval,
`eglGetProcAddress`) are bound as guest thunks. `tests_registry` checks the list against
`docs/feasibility/A-native-imports-symbols.txt`.

| Class | Names | Handling |
|---|---|---|
| ES 2.0 core | about 150 | forwarded to the host driver through the `GlApi` trait |
| Aliases | `*OES` framebuffer/renderbuffer/blend/depth, `glGenVertexArraysOES`, ... | same thunk as the core name |
| Extensions | VAO, `glMapBufferOES` (guest shadow flushed on unmap), 3D textures (ES3 host), `glMultiDraw*EXT` (looped), IMG multisample | `ext.rs` |
| Fixed point | `glClearColorx`, `glTexParameterx`, ... | converted to the float call |
| ES 1.x fixed pipeline, QCOM/AMD/NV/IMG/APPLE vendor calls | 180 | accepted, counted in `Stats::unsupported`, logged once, return 0 (`stubs.rs`). A few that would hand back objects or binaries (`glShaderBinary`, `glProgramBinaryOES`, `glEGLImageTarget*`) also raise `GL_INVALID_OPERATION` |

Marshalling rules (`marshal.rs`, `ext.rs`):

* Guest pointer = `window base + address`. Every pointer is checked with `GuestMemory::is_mapped` for the exact byte
  count (uniform counts, `image_bytes` for pixel data with the tracked `GL_UNPACK/PACK_ALIGNMENT`, `size` for
  buffers). An unmapped range stops the guest with `Stop::Fault`, like a segfault. Then the host pointer is passed on,
  so client-side vertex arrays, `glTexImage2D`, `glReadPixels` and every output parameter are zero-copy.
* `glVertexAttribPointer`/`glDrawElements`: the pointer is an offset when an `ARRAY_BUFFER` / the current VAO's
  `ELEMENT_ARRAY_BUFFER` is bound (tracked), client memory otherwise. Vertex data ranges cannot be known before the
  draw, so only the first byte is checked.
* Strings (`glShaderSource`, names) are copied; `glGetString` copies into a guest string pool (`GlesConfig` arena).
* Policy overrides: `GL_VERSION` is "OpenGL ES 2.0 <driver suffix>", `GL_EXTENSIONS` is the intersection of the
  driver's list with an allow list (vendor extensions and PVRTC are never advertised; VAO and mapbuffer only if
  `expose_vao` / `expose_mapbuffer`, the game has software fallbacks for both), `GL_NUM_SHADER_BINARY_FORMATS` and
  `GL_NUM_PROGRAM_BINARY_FORMATS_OES` are 0 so the game uploads sources.
* Debug: `GlesConfig::check_errors` drains `glGetError` after each call and logs `GL error 0x... after glFoo`.

## Integration (what `bhpc-run` / `bhpc-jni` must do)

```rust
// host main thread
let (backend, gl_window) = bhpc_app::sdl::SdlBackend::new(&WindowConfig::default())?; // Arc<SdlGl>: GlWindow
// GL thread (or any thread) - a context must be current while the driver entry points are loaded:
let primary = gl_window.create_context(None).unwrap();
gl_window.make_current(Some(primary));
let gles = bhpc_gles::Gles::with_host(GlesConfig::new(ARENA_BASE, 8 << 20), gl_window.clone(), mem.clone())?;
gles.egl_initialize();
let ctx = gles.egl_adopt_context(primary);           // the context we just made current
let surface = gles.egl_create_surface();
gles.egl_make_current(surface, surface, ctx)?;
```

1. `ARENA_BASE` is a free 8 MiB guest range (it is mapped by `Gles::new`; pick one outside the bionic heap).
2. Binding imports: in the `bhpc_loader::HostSymbols` implementation (`bhpc-bionic::Hle` or the runner), before the
   trap fallback:
   `if bhpc_gles::provides(name) { return bhpc_gles::resolve(cpu, &gles, name); }` (the thunks are re-entrant
   `SharedThunkFn`s, usable from every guest thread; `Gles` is `Clone + Send + Sync`). `libEGL.so`, `libGLESv1_CM.so`
   and `libGLESv2.so` are host libs in the loader summary, so `dlopen` of them returns `HOST_HANDLE` and `dlsym` should
   go through the same `resolve`.
3. JNI (`VerdeActivity` natives) map to: `swapBuffers` -> `gles.egl_swap_buffers(surface)`, `makeCurrentContext` ->
   `egl_make_current`, `createSharedContext` -> `egl_create_context(primary)`, `destroyContext` ->
   `egl_destroy_context`, `getScreenWidth/Height` -> `gles.drawable_size()` divided by `Metrics::scale()` from
   `bhpc-app`.
4. After the first frames log `gles.stats()`: `unsupported` lists calls that reached an ignored stub (expected to be
   empty on the ES2 path; `glOrthof`/`glFrustumf` are imported by `libApplication` but should not run).

## Known limits

* Binding tracking (`ARRAY_BUFFER`, per-VAO `ELEMENT_ARRAY_BUFFER`, pixel-store alignment) is global, not per context.
  Fine for one render thread; shared contexts that rebind concurrently could confuse the offset/pointer decision.
* Driver entry points are loaded once and used for every context (true for Mesa, NVIDIA, ANGLE core functions).
* `eglCreatePbufferSurface` and `eglBindTexImage` are not supported.
* ES1 matrix/fixed pipeline is not emulated (risk R6 in the plan); calls are counted, not drawn.

## Tests

`cargo test -p bhpc-gles` needs no GPU: a recording `GlApi` mock and a fake `GlWindow` stand in, and the guest side
is real (JIT calls into thunks). `COVERAGE_PACKAGES=bhpc-gles scripts/coverage.sh` gates 100 % lines and functions.
The real-driver path is covered by the ignored test in `bhpc-app` (`--features sdl`).
