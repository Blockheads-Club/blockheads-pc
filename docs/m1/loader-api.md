# bhpc-loader API (M1)

`bhpc-loader` reads the user's APK, maps all bundled ARM32 libraries into the guest window, relocates them and
exposes what the CPU and bionic layers need. It never executes guest code: execution goes through the small
`GuestCaller` trait below, implemented by `bhpc-cpu`.

## APK reader (`bhpc_loader::apk`)

- `Apk::open(path)` / `Apk::from_bytes(vec)`: read-only, in-memory zip (stored and deflate, CRC-32 checked, no
  zip64, no encryption). Nothing is written to disk.
- `apk.libs()` lists `lib/armeabi-v7a/*.so`; `apk.read_lib(name)` decompresses one.
- `apk.verify(force)` hashes `libApplication.so` (SHA-256) against `KNOWN_BUILDS` and returns the `Release`
  (`V1_7_4`, `V1_7_5`, `V1_7_6`). An unknown build is `ApkError::UnknownBuild { sha256 }` unless `force`, which
  returns `Ok(None)`. The hashes are identifiers only:

  | Release | SHA-256 of `lib/armeabi-v7a/libApplication.so` |
  |---|---|
  | 1.7.4 | `e40f53436fe3028aefaa678f2312aab9d2a9a024747aa797d6334494cf225861` |
  | 1.7.5 | `d09418e9c0865902054a71358dcff3264d47f7b24ede58667cb5ea0e6f269b96` |
  | 1.7.6 | `733d821027d69de329d0ba171df2e6013d612edf5a4d327badd001acc30b94c7` |

- `Apk` implements `LibSource`, so it plugs straight into `Linker::load_all`.

## Linking (`Linker`)

```rust
let mut apk = Apk::open(path)?;
apk.verify(force)?;
let mut linker = Linker::new(mem.clone());
linker.set_trap_unresolved(true);                       // unresolved imports -> counted trap thunks
let sum = linker.load_all(&["libApplication.so"], &mut apk, &mut host)?;
// sum.range      indices of the new modules, dependencies first
// sum.host_libs  DT_NEEDED names the APK does not bundle (libc.so, libm.so, libdl.so, liblog.so, ...)
```

- **Order.** `DT_NEEDED` is followed depth first from the roots; modules are registered post-order, so
  `linker.modules()` is topological (a cycle is tolerated). All modules are mapped before any is relocated, so
  imports see the complete set. `load` (single module) stays available for fixtures.
- **Binding.** Defined symbol of a `DF_SYMBOLIC` object: itself. Otherwise: global load order, then
  `HostSymbols::resolve`, then weak (`0`), then, in trap mode, `HostSymbols::trap(name)`. Without trap mode, or if
  `trap` returns `None`, the load fails with `LoadError::Unresolved` and registers nothing. `linker.unresolved()`
  maps each trapped name to the number of relocations that reference it. The trap thunk must record its own hits
  at run time; the loader only counts bindings.
- **Relocations.** `RELATIVE`, `ABS32`, `GLOB_DAT`, `JUMP_SLOT` (`NONE` skipped). Anything else is
  `LoadError::Unsupported`. Per module: `Module::relocs_applied`.
- **Module data.** `segments` (page-rounded `PT_LOAD` with `PF_*`), `exidx`, `init`, `init_array`, `fini_array`,
  `exports`, `symbols` (sorted by address), `symbolic`.
- **Protections.** Everything is mapped read/write. `linker.apply_protections()` applies the segment
  protections with `mprotect` (shared pages get the union). Call it after constructors have run.
- **Unwinding.** `linker.find_exidx(pc)` returns `(exidx_guest_addr, entry_count)` of the module containing `pc`;
  the `__gnu_Unwind_Find_exidx` / `dl_unwind_find_exidx` thunk writes the count to `*pcount` and returns the address.

## dl* (`bhpc_loader::dl`)

Plain methods; `bhpc-bionic` wraps each in a thunk.

| Guest call | Method |
|---|---|
| `dlopen(name, flags)` | `linker.dlopen(Option<&str>)`: handle (`index + 1`), `HOST_HANDLE` for a host lib, `GLOBAL_HANDLE` for NULL, or `0` + `dlerror` |
| `dlsym(h, name)` | `linker.dlsym(h, name, &mut host)`: `RTLD_DEFAULT` = global then host; module handle = module plus its `DT_NEEDED` tree |
| `dlsym(RTLD_NEXT, name)` | `linker.dlsym_from(caller_pc, name)` |
| `dladdr(addr, info)` | `linker.dladdr(addr) -> Option<DlInfo { module, base, symbol }>` (nearest preceding symbol that covers `addr`; Thumb bit kept in `symbol.1`) |
| `dlerror()` | `linker.dlerror()`: message, cleared on read |

## Constructor runner (`bhpc_loader::init`)

`DT_INIT` then `DT_INIT_ARRAY` (entries `0` and `-1` skipped) of each module, in the order given. The CPU side
implements one trait:

```rust
pub trait GuestCaller {
    /// Call the ARM function at `entry` with r0..r2 = `args`, for at most `max_cycles` guest cycles
    /// (0 = unlimited). Return r0, or why it did not return.
    fn call(&mut self, entry: u32, args: [u32; 3], max_cycles: u64) -> Result<u32, CallFailure>;
}
pub enum CallFailure { Watchdog { pc: u32 }, Fault { pc: u32, reason: String } }
```

`run_constructors(&linker.modules()[sum.range], &mut caller, &mut trace, &InitOptions)` returns an `InitReport`
(`ran`, `per_module`, `failures`). `InitOptions`: `watchdog_cycles` (per constructor; map it onto
`Cpu`'s existing `--watchdog` cycle counter), `args` (r0..r2, Android passes `argc, argv, envp`), `keep_going`.
`InitTrace::{before, after}` fire around every constructor (module, `CtorKind::Init | InitArray(n)`, entry, result,
wall time) and default to no-ops. Nested guest calls made by a constructor (`qsort` comparators and the like) are
the caller's business (G0 fix 3); the runner only needs `call` to be callable once per constructor.

## Verification

- `crates/bhpc-loader/coverage.sh` (needs `cargo-llvm-cov`) gates 100 % line and function coverage of the crate,
  excluding only `src/testelf.rs` (the test ELF/zip builder). Region coverage is reported only: the remaining
  uncovered regions are `?` edges on guest-memory reads of an already mapped image.
- `cargo test -p bhpc-loader --test real_apk -- --ignored --nocapture` with `BHPC_APK=<apk>` loads all 25 libraries,
  checks the per-library relocation counts of 1.7.6 against `llvm-readelf -r` and prints the table.

Measured on all three releases (trap mode): 25 modules, 9 host libs
(`libc libm libdl liblog libstdc++ libEGL libGLESv1_CM libGLESv2 libjnigraphics`), 383 distinct unresolved imports,
237 constructors (`libApplication` has 158: the 636-byte `.init_array` ends in a zero word). Relocation totals:
262313 (1.7.4), 262276 (1.7.5), 262286 (1.7.6).
