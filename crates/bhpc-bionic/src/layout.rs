//! Guest-visible struct layouts of the (pre-Lollipop, 32-bit ARM) bionic ABI that the game libraries
//! were built against, plus the constants we share with them (errno values, `sysconf` names, ...).
//!
//! Everything here is plain data so it can be unit tested without a CPU.

/// `FILE` as laid out by bionic's `<stdio.h>` (84 bytes).
pub mod file {
    pub const SIZE: u32 = 84;
    pub const P: u32 = 0;
    pub const R: u32 = 4;
    pub const W: u32 = 8;
    pub const FLAGS: u32 = 12; // short
    pub const FILE_NO: u32 = 14; // short
    pub const BF_BASE: u32 = 16;
    pub const BF_SIZE: u32 = 20;
    pub const LBFSIZE: u32 = 24;
    pub const COOKIE: u32 = 28;
    pub const CLOSE: u32 = 32;
    pub const READ: u32 = 36;
    pub const SEEK: u32 = 40;
    pub const WRITE: u32 = 44;
    pub const EXT_BASE: u32 = 48;
    pub const EXT_SIZE: u32 = 52;
    pub const UP: u32 = 56;
    pub const UR: u32 = 60;
    pub const UBUF: u32 = 64; // 3 bytes
    pub const NBUF: u32 = 67; // 1 byte
    pub const LB_BASE: u32 = 68;
    pub const LB_SIZE: u32 = 72;
    pub const BLKSIZE: u32 = 76;
    pub const OFFSET: u32 = 80;
    /// `__SLBF`: line buffered.
    pub const SLBF: u16 = 0x0001;
    /// `__SNBF`: unbuffered.
    pub const SNBF: u16 = 0x0002;
    /// `__SRD`: open for reading.
    pub const SRD: u16 = 0x0004;
    /// `__SWR`: open for writing.
    pub const SWR: u16 = 0x0008;
    pub const SRW: u16 = 0x0010;
    pub const SEOF: u16 = 0x0020;
    pub const SERR: u16 = 0x0040;
}

/// `struct tm` (44 bytes: nine ints, `tm_gmtoff`, `tm_zone`).
pub mod tm {
    pub const SIZE: u32 = 44;
    pub const SEC: u32 = 0;
    pub const MIN: u32 = 4;
    pub const HOUR: u32 = 8;
    pub const MDAY: u32 = 12;
    pub const MON: u32 = 16;
    pub const YEAR: u32 = 20;
    pub const WDAY: u32 = 24;
    pub const YDAY: u32 = 28;
    pub const ISDST: u32 = 32;
    pub const GMTOFF: u32 = 36;
    pub const ZONE: u32 = 40;
}

/// 32-bit `struct timespec` / `struct timeval` (`time_t` is a 32-bit `long` on ARM bionic).
pub mod timespec {
    pub const SIZE: u32 = 8;
    pub const SEC: u32 = 0;
    pub const NSEC: u32 = 4;
}

/// `pthread_attr_t` (24 bytes).
pub mod attr {
    pub const SIZE: u32 = 24;
    pub const FLAGS: u32 = 0;
    pub const STACK_BASE: u32 = 4;
    pub const STACK_SIZE: u32 = 8;
    pub const GUARD_SIZE: u32 = 12;
    pub const SCHED_POLICY: u32 = 16;
    pub const SCHED_PRIORITY: u32 = 20;
    pub const DEFAULT_STACK_SIZE: u32 = 1024 * 1024 - 8192;
    pub const DEFAULT_GUARD_SIZE: u32 = 4096;
    pub const CREATE_DETACHED: u32 = 1;
}

/// Sizes of the small synchronisation objects: bionic keeps `pthread_mutex_t`, `pthread_cond_t`,
/// `pthread_once_t` and the attribute objects in a single 32-bit word.
pub mod sync {
    pub const MUTEX_SIZE: u32 = 4;
    pub const COND_SIZE: u32 = 4;
    pub const ONCE_SIZE: u32 = 4;
    pub const MUTEXATTR_SIZE: u32 = 4;
    pub const CONDATTR_SIZE: u32 = 4;
    pub const RWLOCK_SIZE: u32 = 40;
    /// Bits 14..15 of the mutex word hold the type.
    pub const MUTEX_TYPE_SHIFT: u32 = 14;
    pub const MUTEX_NORMAL: u32 = 0;
    pub const MUTEX_RECURSIVE: u32 = 1;
    pub const MUTEX_ERRORCHECK: u32 = 2;
    /// Value `pthread_once` stores when the initialiser has completed.
    pub const ONCE_DONE: u32 = 2;
    /// `PTHREAD_KEYS_MAX`.
    pub const KEYS_MAX: u32 = 128;
}

/// Size of the `sigset_t` bionic uses on 32-bit ARM.
pub const SIGSET_SIZE: u32 = 4;
/// `ctype` table flag bits (`<ctype.h>` of old bionic).
pub mod ctype {
    pub const U: u8 = 0x01;
    pub const L: u8 = 0x02;
    pub const N: u8 = 0x04;
    pub const S: u8 = 0x08;
    pub const P: u8 = 0x10;
    pub const C: u8 = 0x20;
    pub const X: u8 = 0x40;
    pub const B: u8 = 0x80;
    /// `_ctype_` has one entry per byte value plus one for EOF at index 0.
    pub const TABLE_LEN: usize = 257;
}

/// `PROP_VALUE_MAX`: longest system property value including the NUL.
pub const PROP_VALUE_MAX: u32 = 92;
/// Page size reported to the guest.
pub const PAGE_SIZE: u32 = 4096;

pub mod errno {
    pub const EPERM: u32 = 1;
    pub const ENOENT: u32 = 2;
    pub const EINTR: u32 = 4;
    pub const ECHILD: u32 = 10;
    pub const EACCES: u32 = 13;
    pub const ENODEV: u32 = 19;
    pub const ENOTTY: u32 = 25;
    pub const ESPIPE: u32 = 29;
    pub const ENOSYS: u32 = 38;
    pub const EAFNOSUPPORT: u32 = 97;
    pub const ESRCH: u32 = 3;
    pub const EBADF: u32 = 9;
    pub const EAGAIN: u32 = 11;
    pub const ENOMEM: u32 = 12;
    pub const EBUSY: u32 = 16;
    pub const EINVAL: u32 = 22;
    pub const ERANGE: u32 = 34;
    pub const EDEADLK: u32 = 35;
    pub const ENOTSUP: u32 = 95;
    pub const ETIMEDOUT: u32 = 110;
}

/// `<unistd.h>` `sysconf` names (bionic numbering).
pub mod sc {
    pub const ARG_MAX: u32 = 0x00;
    pub const CHILD_MAX: u32 = 0x05;
    pub const CLK_TCK: u32 = 0x06;
    pub const OPEN_MAX: u32 = 0x0b;
    pub const PAGESIZE: u32 = 0x27;
    pub const PAGE_SIZE: u32 = 0x28;
    pub const NPROCESSORS_CONF: u32 = 0x60;
    pub const NPROCESSORS_ONLN: u32 = 0x61;
    pub const PHYS_PAGES: u32 = 0x62;
    pub const AVPHYS_PAGES: u32 = 0x63;
}

/// `<time.h>` clock ids (Linux numbering).
pub mod clock {
    pub const REALTIME: u32 = 0;
    pub const MONOTONIC: u32 = 1;
    pub const PROCESS_CPUTIME: u32 = 2;
    pub const THREAD_CPUTIME: u32 = 3;
    pub const MONOTONIC_RAW: u32 = 4;
    pub const REALTIME_COARSE: u32 = 5;
    pub const MONOTONIC_COARSE: u32 = 6;
    pub const BOOTTIME: u32 = 7;
    /// `CLOCKS_PER_SEC` in bionic.
    pub const CLOCKS_PER_SEC: u32 = 1_000_000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_layout_is_bionic_84_bytes() {
        // Offsets must be strictly increasing and the last field must end exactly at SIZE.
        let offsets = [
            file::P,
            file::R,
            file::W,
            file::FLAGS,
            file::FILE_NO,
            file::BF_BASE,
            file::BF_SIZE,
            file::LBFSIZE,
            file::COOKIE,
            file::CLOSE,
            file::READ,
            file::SEEK,
            file::WRITE,
            file::EXT_BASE,
            file::EXT_SIZE,
            file::UP,
            file::UR,
            file::UBUF,
            file::NBUF,
            file::LB_BASE,
            file::LB_SIZE,
            file::BLKSIZE,
            file::OFFSET,
        ];
        assert!(offsets.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(file::OFFSET + 4, file::SIZE);
        assert_eq!(file::SIZE, 84);
        assert_eq!(file::UBUF + 3, file::NBUF);
        assert_eq!(file::NBUF + 1, file::LB_BASE);
        assert_eq!(file::FLAGS + 2, file::FILE_NO);
        assert_eq!(file::FILE_NO + 2, file::BF_BASE);
        assert_eq!(file::SLBF | file::SNBF | file::SRD | file::SWR, 0xf);
    }

    #[test]
    fn tm_and_timespec_layout() {
        let f = [
            tm::SEC,
            tm::MIN,
            tm::HOUR,
            tm::MDAY,
            tm::MON,
            tm::YEAR,
            tm::WDAY,
            tm::YDAY,
            tm::ISDST,
            tm::GMTOFF,
            tm::ZONE,
        ];
        for (i, o) in f.iter().enumerate() {
            assert_eq!(*o, 4 * i as u32);
        }
        assert_eq!(tm::ZONE + 4, tm::SIZE);
        assert_eq!(timespec::NSEC + 4, timespec::SIZE);
        assert_eq!(timespec::SEC, 0);
    }

    #[test]
    fn pthread_object_layout() {
        assert_eq!(attr::SCHED_PRIORITY + 4, attr::SIZE);
        let f = [
            attr::FLAGS,
            attr::STACK_BASE,
            attr::STACK_SIZE,
            attr::GUARD_SIZE,
            attr::SCHED_POLICY,
            attr::SCHED_PRIORITY,
        ];
        for (i, o) in f.iter().enumerate() {
            assert_eq!(*o, 4 * i as u32);
        }
        assert_eq!(attr::DEFAULT_STACK_SIZE, 1024 * 1024 - 8192);
        // mutex + cond + 4 ints + 4 reserved pointers.
        assert_eq!(sync::MUTEX_SIZE + sync::COND_SIZE + 4 * 4 + 4 * 4, sync::RWLOCK_SIZE);
        for s in [sync::MUTEX_SIZE, sync::COND_SIZE, sync::ONCE_SIZE, sync::MUTEXATTR_SIZE, sync::CONDATTR_SIZE] {
            assert_eq!(s, 4);
        }
        assert_eq!(SIGSET_SIZE, 4);
        assert_eq!(sync::MUTEX_RECURSIVE << sync::MUTEX_TYPE_SHIFT, 0x4000);
        assert_eq!(sync::MUTEX_ERRORCHECK << sync::MUTEX_TYPE_SHIFT, 0x8000);
    }

    #[test]
    fn ctype_flag_bits_are_disjoint_and_complete() {
        let all = [ctype::U, ctype::L, ctype::N, ctype::S, ctype::P, ctype::C, ctype::X, ctype::B];
        assert_eq!(all.iter().fold(0u8, |a, b| a | b), 0xff);
        assert_eq!(all.iter().map(|b| b.count_ones()).sum::<u32>(), 8);
        assert_eq!(ctype::TABLE_LEN, 257);
    }

    #[test]
    fn constants_match_bionic_headers() {
        assert_eq!(errno::EINVAL, 22);
        assert_eq!(errno::ETIMEDOUT, 110);
        assert_eq!(sc::PAGESIZE, 0x27);
        assert_eq!(sc::NPROCESSORS_ONLN, 0x61);
        assert_eq!(clock::BOOTTIME, 7);
        assert_eq!(PROP_VALUE_MAX, 92);
        assert_eq!(PAGE_SIZE, 4096);
        let _ = (errno::EPERM, errno::ENOENT, errno::ESRCH, errno::EBADF, errno::EAGAIN, errno::ENOMEM, errno::EBUSY);
        let _ = (errno::ERANGE, errno::EDEADLK, errno::ENOTSUP, sc::ARG_MAX, sc::CHILD_MAX, sc::CLK_TCK, sc::OPEN_MAX);
        let _ =
            (sc::PAGE_SIZE, sc::NPROCESSORS_CONF, sc::PHYS_PAGES, sc::AVPHYS_PAGES, clock::REALTIME, clock::MONOTONIC);
        let _ = (clock::PROCESS_CPUTIME, clock::THREAD_CPUTIME, clock::MONOTONIC_RAW, clock::REALTIME_COARSE);
        let _ = (clock::MONOTONIC_COARSE, clock::CLOCKS_PER_SEC, attr::DEFAULT_GUARD_SIZE, attr::CREATE_DETACHED);
        let _ = (sync::MUTEX_NORMAL, sync::ONCE_DONE, sync::KEYS_MAX);
    }
}
