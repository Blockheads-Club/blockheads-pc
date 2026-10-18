//! `pread`/`pwrite` and shared file mappings over a real (temporary) data directory.
use crate::mmapio::SharedMaps;
use crate::new_env_with;
use bhpc_bionic::layout::errno;
use bhpc_loader::testelf::{build_zip, Builder};
use bhpc_vfs::{DataDirs, Storage, VfsConfig};

const NEG1: u32 = u32::MAX;
const PATH: &str = "/data/data/org.example.g/files/map.bin";

struct Rig {
    cpu: Box<bhpc_cpu::Cpu>,
    st: bhpc_bionic::Shared,
    scratch: u32,
    dir: std::path::PathBuf,
}

fn rig() -> Rig {
    let dir =
        std::env::temp_dir().join(format!("bhpc-mmapio-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lib = Builder::default().build().bytes;
    let apk = dir.join("g.apk");
    std::fs::write(&apk, build_zip(&[("lib/armeabi-v7a/libApplication.so", &lib, false)])).unwrap();
    let mut vcfg = VfsConfig::new(&apk, DataDirs::new(dir.join("data")));
    vcfg.package = "org.example.g".into();
    let dirs = DataDirs::new(dir.join("data"));
    dirs.create_all().unwrap();
    let storage = Storage::open(&vcfg).unwrap();
    let cfg = bhpc_bionic::Config { fs: storage.vfs.clone(), ..bhpc_bionic::Config::default() };
    let (cpu, st) = new_env_with(cfg, None).unwrap();
    SharedMaps::default().install(&cpu, &st);
    let scratch = bhpc_bionic::lock(&st).heap.calloc(1, 16384);
    Rig { cpu, st, scratch, dir }
}

impl Rig {
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        let a = bhpc_bionic::resolve(&mut self.cpu, &self.st, name).unwrap();
        self.cpu.call(a, args).unwrap().r0
    }
    fn faults(&mut self, name: &str, args: &[u32]) -> bool {
        let a = bhpc_bionic::resolve(&mut self.cpu, &self.st, name).unwrap();
        self.cpu.call(a, args).is_err()
    }
    fn errno(&mut self) -> u32 {
        let a = bhpc_bionic::sched::errno_addr(&self.st);
        self.cpu.mem().read_u32(a).unwrap()
    }
    fn put(&self, at: u32, b: &[u8]) -> u32 {
        self.cpu.mem().write_bytes(self.scratch + at, b).unwrap();
        self.scratch + at
    }
    fn open_file(&mut self) -> u32 {
        let p = self.put(0, format!("{PATH}\0").as_bytes());
        let fd = self.call("open", &[p, 0x42, 0o644]);
        assert_ne!(fd, NEG1);
        fd
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn positioned_io_keeps_the_file_position_and_refreshes_maps() {
    let mut r = rig();
    let fd = r.open_file();
    let data: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    let buf = r.put(100, &data);
    assert_eq!(r.call("write", &[fd, buf, 8192]), 8192);
    // pread: does not move the position.
    let out = r.scratch + 9000;
    assert_eq!(r.call("lseek", &[fd, 5, 0]), 5);
    assert_eq!(r.call("pread", &[fd, out, 4, 100]), 4);
    assert_eq!(r.cpu.mem().read_bytes(out, 4).unwrap(), &data[100..104]);
    assert_eq!(r.call("lseek", &[fd, 0, 1]), 5);
    assert_eq!(r.call("pread64", &[fd, out, 4, 0, 200, 0]), 4);
    assert_eq!(r.cpu.mem().read_bytes(out, 4).unwrap(), &data[200..204]);
    // A shared mapping of the file, one page past the first.
    let map = r.call("mmap", &[0, 8192, 1, 1, fd, 0]);
    assert!(map != NEG1 && map % 4096 == 0);
    assert_eq!(r.cpu.mem().read_bytes(map, 8192).unwrap(), data);
    let second = r.call("mmap", &[0, 4096, 1, 1, fd, 4096]);
    assert_eq!(r.cpu.mem().read_bytes(second, 4).unwrap(), &data[4096..4100]);
    // pwrite refreshes the maps it overlaps (and only those bytes), also through pwrite64.
    let new = r.put(500, b"WXYZ");
    assert_eq!(r.call("pwrite", &[fd, new, 4, 4094]), 4);
    assert_eq!(r.cpu.mem().read_bytes(map + 4094, 4).unwrap(), b"WXYZ");
    assert_eq!(r.cpu.mem().read_bytes(second, 2).unwrap(), b"YZ", "the second map starts at offset 4096");
    assert_eq!(r.call("pwrite64", &[fd, new, 4, 0, 10, 0]), 4);
    assert_eq!(r.cpu.mem().read_bytes(map + 10, 4).unwrap(), b"WXYZ");
    assert_eq!(r.cpu.mem().read_bytes(second, 2).unwrap(), b"YZ", "no overlap, untouched");
    assert_eq!(r.call("lseek", &[fd, 0, 1]), 5, "pwrite keeps the position too");
    // A map larger than the file reads zeros past the end; the file is not extended.
    let big = r.call("mmap", &[0, 16384, 1, 1, fd, 0]);
    assert_eq!(r.cpu.mem().read_u32(big + 12000).unwrap(), 0);
    // Unmapping forgets the mapping: later writes no longer reach it.
    assert_eq!(r.call("munmap", &[map, 8192]), 0);
    let before = r.cpu.mem().read_bytes(map + 10, 4).unwrap();
    assert_eq!(r.call("pwrite", &[fd, r.scratch + 500, 4, 10]), 4);
    assert_eq!(r.cpu.mem().read_bytes(map + 10, 4).unwrap(), before);
    assert_eq!(r.call("munmap", &[map, 8192]), 0, "an unknown address goes to bionic");
}

#[test]
fn failures_are_reported() {
    let mut r = rig();
    let fd = r.open_file();
    let buf = r.put(100, b"abcd");
    // Bad descriptors.
    assert_eq!(r.call("pread", &[999, buf, 4, 0]), NEG1);
    assert_eq!(r.errno(), errno::EBADF);
    assert_eq!(r.call("pwrite", &[999, buf, 4, 0]), NEG1);
    assert_eq!(r.call("mmap", &[0, 4096, 1, 1, 999, 0]), NEG1);
    // Bad guest pointers stop the guest.
    assert!(r.faults("pwrite", &[fd, 0xDEAD_0000, 4, 0]));
    assert_eq!(r.call("pwrite", &[fd, buf, 4, 0]), 4);
    assert!(r.faults("pread", &[fd, 0xDEAD_0000, 4, 0]));
    // More address space than exists.
    assert_eq!(r.call("mmap", &[0, 0x3800_0000, 1, 1, fd, 0]), NEG1);
    assert_eq!(r.errno(), errno::ENOMEM);
    // Anonymous and private mappings are bionic's.
    let anon = r.call("mmap", &[0, 4096, 3, 0x22, NEG1, 0]);
    assert!(anon != NEG1 && anon != 0);
    let private = r.call("mmap", &[0, 4096, 3, 2, fd, 0]);
    assert!(private != NEG1);
    assert_eq!(r.call("munmap", &[anon, 4096]), 0);
}
