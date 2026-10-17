//! Real-APK smoke test. Ignored by default: set `BHPC_APK` to a user-owned Blockheads APK and run
//! `cargo test -p bhpc-loader --test real_apk -- --ignored --nocapture`.

use bhpc_loader::apk::{Apk, Release, IDENTITY_LIB};
use bhpc_loader::{HostSymbols, Linker};
use bhpc_mem::GuestMemory;
use std::path::PathBuf;
use std::sync::Arc;

struct Host {
    traps: Vec<String>,
}

impl HostSymbols for Host {
    fn resolve(&mut self, _name: &str) -> Option<u32> {
        None
    }
    fn trap(&mut self, name: &str) -> Option<u32> {
        self.traps.push(name.to_string());
        Some(0xF000_0000 + 8 * self.traps.len() as u32)
    }
}

/// Per-library relocation counts of 1.7.6, cross-checked against `llvm-readelf -r` (counts only).
const RELOCS_1_7_6: [(&str, usize); 25] = [
    ("libApplication.so", 149287),
    ("libAudioFile.so", 148),
    ("libAudioToolbox.so", 80),
    ("libAudioUnit.so", 196),
    ("libBridgeKit.so", 18172),
    ("libCFNetwork.so", 2081),
    ("libCommonCrypto.so", 442),
    ("libCoreAudio.so", 21),
    ("libCoreFoundation.so", 16307),
    ("libCoreGraphics.so", 7314),
    ("libCoreText.so", 1580),
    ("libcxx.so", 6304),
    ("libFoundation.so", 28796),
    ("libgles_apportable.so", 452),
    ("libGraphicsServices.so", 2102),
    ("libicu.so", 10266),
    ("libNoodleCompatibility.so", 63),
    ("libNoodleFoundation.so", 93),
    ("libOpenAL.so", 635),
    ("libpango.so", 1596),
    ("libSecurity.so", 9881),
    ("libSystemConfiguration.so", 94),
    ("libSystem.so", 2357),
    ("libv.so", 743),
    ("libxml2.so", 3276),
];

#[test]
#[ignore]
fn loads_all_25_libs_and_counts_relocations() {
    let path = PathBuf::from(std::env::var("BHPC_APK").expect("set BHPC_APK"));
    let mut apk = Apk::open(&path).unwrap();
    let release = apk.verify(false).unwrap();
    let libs: Vec<String> = apk.libs().iter().map(|s| s.to_string()).collect();
    assert_eq!(libs.len(), 25);
    println!("release {release:?}, {} libs", libs.len());

    let mem = Arc::new(GuestMemory::new().unwrap());
    let mut linker = Linker::new(mem);
    linker.set_trap_unresolved(true);
    let mut host = Host { traps: Vec::new() };
    let roots: Vec<&str> = vec![IDENTITY_LIB];
    let sum = linker.load_all(&roots, &mut apk, &mut host).unwrap();
    println!("loaded {} modules; host libs {:?}", sum.range.len(), sum.host_libs);
    let (mut total, mut ctors) = (0, 0);
    for m in linker.modules() {
        println!(
            "{:28} reloc {:7} init {} ctors {:4} exidx {:?} syms {}",
            m.name,
            m.relocs_applied,
            m.init.is_some(),
            m.init_array.len(),
            m.exidx.map(|e| e.1),
            m.symbols.len()
        );
        total += m.relocs_applied;
        ctors += m.constructors().len();
    }
    if release == Some(Release::V1_7_6) {
        for (name, n) in RELOCS_1_7_6 {
            assert_eq!(linker.module(name).unwrap().relocs_applied, n, "{name}");
        }
        assert_eq!(total, 262_286);
    }
    println!("total relocs {total}, ctors {ctors}, unresolved distinct {}", linker.unresolved().len());
}
