# M2 merge notes

`m2/gles`, `m2/audio-input`, `m2/vfs` and `m2/jni` were merged into `dev` (branched from `fb9866c`).

* Source of truth: the fuller m2 crates. `bhpc-jni` is m2's fake JavaVM (the minimal permissive one from `efb5e26` and its
  `table.rs` are gone); `bhpc-vfs` is m2's VFS.
* Dev-only behaviour ported into `bhpc-vfs`: `/proc/self/cmdline` and `/proc/<pid>/cmdline`, `/proc/self/exe` (+ `readlink`),
  `/dev/zero`, the loader environment (`CFProcessPath`, `DEBUG_BUILD`, `NSUnrecognizedForwardingDisabled`, `VERSION_CODE`,
  `LOCALE`, `LANGUAGES`, `TZ`), `EEXIST` for `mkdir` on an existing read-only path and `ENOENT` for removing a missing one.
* `bhpc-bionic/src/vfs.rs` is now only the guest descriptor table over `bhpc_vfs::FileSystem` (`Config::fs`).
* `bhpc-jni` gained `host` (`install`, `Jni`, `Usage`, `JniError`), `Assets` for `bhpc_vfs::AssetManager`, `Platform::for_guest`.
  Permissive-by-default is the m2 `lenient` mode: unknown classes/members are invented, reported and answer 0/NULL.
* `bhpc-run`: `Session::open` builds `bhpc_vfs::Storage` (`--data-dir` is a `DataDirs` root; default is the per-user dir),
  the VM allocates from the bionic heap, and the loader hooks run inside a JNI local-reference frame.
* Behaviour changes visible in `--init-only`: the `jni:` line now counts invented classes/members of the m2 VM
  (50 classes, 148 methods, 28 fields, 4 natives, 2 distinct Java methods called on all three APKs).
