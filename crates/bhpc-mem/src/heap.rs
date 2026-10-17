//! A small power-of-two size-class allocator living inside the guest window.
//!
//! Used for host-owned objects the guest must see and for the HLE `malloc` family. Every block has
//! a 16-byte header `{ class: u32, orig: u32, magic: u32, _pad }` right before the payload.

use crate::{GuestMemory, MemError};
use std::sync::Arc;

const HEADER: u32 = 16;
const MIN_CLASS: u32 = 4; // 16 bytes
const MAGIC: u32 = 0xB10C_4EAD;

/// Allocator over a dedicated mapped region of the guest window.
pub struct GuestHeap {
    mem: Arc<GuestMemory>,
    cursor: u32,
    end: u32,
    free: Vec<Vec<u32>>,
}

impl GuestHeap {
    /// Map `[base, base+size)` and use it as the heap.
    pub fn new(mem: Arc<GuestMemory>, base: u32, size: u32) -> Result<Self, MemError> {
        mem.map(base, size)?;
        Ok(GuestHeap { mem, cursor: base, end: base + size, free: vec![Vec::new(); 33] })
    }

    fn class_of(size: u32) -> u32 {
        size.max(1).next_power_of_two().trailing_zeros().max(MIN_CLASS)
    }

    fn take_block(&mut self, class: u32) -> Option<u32> {
        if let Some(p) = self.free[class as usize].pop() {
            return Some(p);
        }
        let total = 1u64 << class;
        let start = (self.cursor as u64 + 15) & !15;
        let payload = start + HEADER as u64;
        if payload + total > self.end as u64 {
            return None;
        }
        self.cursor = (payload + total) as u32;
        Some(payload as u32)
    }

    fn write_header(&self, payload: u32, class: u32, orig: u32) {
        let h = payload - HEADER;
        // The block lives inside our own mapped region, so these cannot fail.
        self.mem.write_u32(h, class).expect("heap header");
        self.mem.write_u32(h + 4, orig).expect("heap header");
        self.mem.write_u32(h + 8, MAGIC).expect("heap header");
    }

    /// Allocate `size` bytes (16-byte aligned). Returns 0 when out of memory.
    pub fn alloc(&mut self, size: u32) -> u32 {
        if size > (1 << 30) {
            return 0;
        }
        let class = Self::class_of(size);
        match self.take_block(class) {
            Some(p) => {
                self.write_header(p, class, p);
                p
            }
            None => 0,
        }
    }

    /// Allocate zeroed memory.
    pub fn calloc(&mut self, count: u32, size: u32) -> u32 {
        let Some(total) = count.checked_mul(size) else { return 0 };
        let p = self.alloc(total);
        if p != 0 {
            let len = self.usable_size(p);
            self.mem.write_bytes(p, &vec![0u8; len as usize]).expect("heap block");
        }
        p
    }

    /// Allocate with an alignment (a power of two, `> 16` is honoured).
    pub fn memalign(&mut self, align: u32, size: u32) -> u32 {
        if !align.is_power_of_two() || size > (1 << 30) {
            return 0;
        }
        if align <= 16 {
            return self.alloc(size);
        }
        let class = Self::class_of(size + align);
        let Some(orig) = self.take_block(class) else { return 0 };
        let aligned = (orig + align - 1) & !(align - 1);
        // Header of the original block (needed to free it) plus a forwarding header at the aligned address.
        self.write_header(orig, class, orig);
        if aligned != orig {
            self.write_header(aligned, class, orig);
        }
        aligned
    }

    fn header(&self, p: u32) -> (u32, u32) {
        let class = self.mem.read_u32(p - HEADER).expect("heap header");
        let orig = self.mem.read_u32(p - HEADER + 4).expect("heap header");
        let magic = self.mem.read_u32(p - HEADER + 8).expect("heap header");
        assert_eq!(magic, MAGIC, "free/realloc of a pointer not owned by the guest heap: {p:#x}");
        (class, orig)
    }

    /// Usable bytes of the block at `p`.
    pub fn usable_size(&self, p: u32) -> u32 {
        let (class, orig) = self.header(p);
        (1u32 << class) - (p - orig)
    }

    /// Release a block (`free(NULL)` is a no-op).
    pub fn free(&mut self, p: u32) {
        if p == 0 {
            return;
        }
        let (class, orig) = self.header(p);
        self.free[class as usize].push(orig);
    }

    /// Resize a block, preserving contents.
    pub fn realloc(&mut self, p: u32, size: u32) -> u32 {
        if p == 0 {
            return self.alloc(size);
        }
        let old = self.usable_size(p);
        if size <= old {
            return p;
        }
        let n = self.alloc(size);
        if n == 0 {
            return 0;
        }
        let data = self.mem.read_bytes(p, old).expect("heap block");
        self.mem.write_bytes(n, &data).expect("heap block");
        self.free(p);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heap() -> (Arc<GuestMemory>, GuestHeap) {
        let m = Arc::new(GuestMemory::new().unwrap());
        let h = GuestHeap::new(m.clone(), 0x4000_0000, 0x10_0000).unwrap();
        (m, h)
    }

    #[test]
    fn alloc_is_aligned_distinct_and_reused_after_free() {
        let (m, mut h) = heap();
        let a = h.alloc(10);
        let b = h.alloc(10);
        assert_ne!(a, 0);
        assert_eq!(a % 16, 0);
        assert_ne!(a, b);
        m.write_bytes(a, &[7; 10]).unwrap();
        assert_eq!(h.usable_size(a), 16);
        h.free(a);
        assert_eq!(h.alloc(12), a, "same size class is recycled");
        h.free(0);
    }

    #[test]
    fn calloc_zeroes_recycled_memory() {
        let (m, mut h) = heap();
        let a = h.alloc(64);
        m.write_bytes(a, &[0xff; 64]).unwrap();
        h.free(a);
        let c = h.calloc(8, 8);
        assert_eq!(c, a);
        assert_eq!(m.read_bytes(c, 64).unwrap(), vec![0; 64]);
        assert_eq!(h.calloc(u32::MAX, 2), 0);
    }

    #[test]
    fn realloc_grow_shrink_and_null() {
        let (m, mut h) = heap();
        let a = h.realloc(0, 8);
        assert_ne!(a, 0);
        m.write_bytes(a, b"12345678").unwrap();
        assert_eq!(h.realloc(a, 4), a, "shrink keeps block");
        let b = h.realloc(a, 100);
        assert_ne!(b, a);
        assert_eq!(m.read_bytes(b, 8).unwrap(), b"12345678");
        assert_eq!(h.realloc(b, u32::MAX), 0);
    }

    #[test]
    fn memalign_big_alignment_and_free() {
        let (_m, mut h) = heap();
        let p = h.memalign(256, 40);
        assert_eq!(p % 256, 0);
        assert!(h.usable_size(p) >= 40);
        h.free(p);
        let q = h.memalign(16, 8);
        assert_eq!(q % 16, 0);
        assert_eq!(h.memalign(3, 8), 0, "not a power of two");
        assert_eq!(h.memalign(64, u32::MAX), 0);
        // An already aligned original needs no forwarding header.
        let (_m2, mut h2) = heap();
        let r = h2.memalign(32, 8);
        assert_eq!(r % 32, 0);
    }

    #[test]
    fn out_of_memory_returns_null() {
        let m = Arc::new(GuestMemory::new().unwrap());
        let mut h = GuestHeap::new(m, 0x4000_0000, 0x1000).unwrap();
        assert_eq!(h.alloc(0x2000), 0);
        assert_eq!(h.alloc(u32::MAX), 0);
        assert_eq!(h.memalign(64, 0x2000), 0);
        assert_ne!(h.alloc(16), 0);
    }

    #[test]
    #[should_panic(expected = "not owned")]
    fn free_of_foreign_pointer_panics() {
        let (m, mut h) = heap();
        m.map(0x1000, 0x1000).unwrap();
        h.free(0x1100);
    }
}
