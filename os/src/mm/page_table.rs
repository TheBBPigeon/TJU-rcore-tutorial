use super::{FrameTracker, PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum, frame_alloc};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::*;

/*
 * Sv39 page-table entry:
 *
 * bit 0      V
 * bit 1      R
 * bit 2      W
 * bit 3      X
 * bit 4      U
 * bit 5      G
 * bit 6      A
 * bit 7      D
 * bit 8..9   RSW, reserved for supervisor software
 *
 * We use RSW bit 8 as the copy-on-write marker.
 */
const PTE_COW: usize = 1 << 8;

bitflags! {
    pub struct PTEFlags: u8 {
        const V = 1 << 0;
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
        const G = 1 << 5;
        const A = 1 << 6;
        const D = 1 << 7;
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
pub struct PageTableEntry {
    pub bits: usize,
}

impl PageTableEntry {
    pub fn new(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self {
            bits: ppn.0 << 10 | flags.bits as usize,
        }
    }

    fn new_cow(ppn: PhysPageNum, flags: PTEFlags) -> Self {
        Self {
            bits: ppn.0 << 10 | flags.bits as usize | PTE_COW,
        }
    }

    pub fn empty() -> Self {
        Self { bits: 0 }
    }

    pub fn ppn(&self) -> PhysPageNum {
        (self.bits >> 10 & ((1usize << 44) - 1)).into()
    }

    /*
     * PTEFlags contains the eight hardware flag bits only.
     * Casting to u8 intentionally excludes the COW bit at bit 8.
     */
    pub fn flags(&self) -> PTEFlags {
        PTEFlags::from_bits(self.bits as u8).unwrap()
    }

    pub fn is_valid(&self) -> bool {
        self.flags().contains(PTEFlags::V)
    }

    pub fn readable(&self) -> bool {
        self.flags().contains(PTEFlags::R)
    }

    pub fn writable(&self) -> bool {
        self.flags().contains(PTEFlags::W)
    }

    pub fn executable(&self) -> bool {
        self.flags().contains(PTEFlags::X)
    }

    pub fn user_accessible(&self) -> bool {
        self.flags().contains(PTEFlags::U)
    }

    pub fn is_cow(&self) -> bool {
        self.bits & PTE_COW != 0
    }
}

pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}

/// Assume that it will not run out of memory while creating mappings.
impl PageTable {
    pub fn new() -> Self {
        let frame = frame_alloc().unwrap();

        Self {
            root_ppn: frame.ppn,
            frames: vec![frame],
        }
    }

    /// Temporarily used to access another address space by SATP token.
    pub fn from_token(satp: usize) -> Self {
        Self {
            root_ppn: PhysPageNum::from(satp & ((1usize << 44) - 1)),
            frames: Vec::new(),
        }
    }

    fn find_pte_create(&mut self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;

        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];

            if i == 2 {
                result = Some(pte);
                break;
            }

            if !pte.is_valid() {
                let frame = frame_alloc().unwrap();
                *pte = PageTableEntry::new(frame.ppn, PTEFlags::V);
                self.frames.push(frame);
            }

            ppn = pte.ppn();
        }

        result
    }

    fn find_pte(&self, vpn: VirtPageNum) -> Option<&mut PageTableEntry> {
        let idxs = vpn.indexes();
        let mut ppn = self.root_ppn;
        let mut result: Option<&mut PageTableEntry> = None;

        for (i, idx) in idxs.iter().enumerate() {
            let pte = &mut ppn.get_pte_array()[*idx];

            if i == 2 {
                result = Some(pte);
                break;
            }

            if !pte.is_valid() {
                return None;
            }

            ppn = pte.ppn();
        }

        result
    }

    #[allow(unused)]
    pub fn map(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();

        assert!(!pte.is_valid(), "vpn {:?} is mapped before mapping", vpn);

        *pte = PageTableEntry::new(ppn, flags | PTEFlags::V);
    }

    /*
     * Map a shared physical page as copy-on-write.
     *
     * A COW mapping is always read-only in hardware. A later user write
     * therefore raises StorePageFault and enters the COW fault handler.
     */
    pub fn map_cow(&mut self, vpn: VirtPageNum, ppn: PhysPageNum, mut flags: PTEFlags) {
        let pte = self.find_pte_create(vpn).unwrap();

        assert!(
            !pte.is_valid(),
            "vpn {:?} is mapped before COW mapping",
            vpn
        );

        flags.remove(PTEFlags::W);
        flags.insert(PTEFlags::V);

        *pte = PageTableEntry::new_cow(ppn, flags);
    }

    /*
     * Convert an existing writable mapping into a read-only COW mapping.
     *
     * This is used on the parent address space during fork.
     */
    pub fn mark_cow(&mut self, vpn: VirtPageNum) {
        let pte = self
            .find_pte(vpn)
            .expect("mark_cow: page-table entry not found");

        assert!(pte.is_valid(), "mark_cow: vpn {:?} is not mapped", vpn);

        let ppn = pte.ppn();
        let mut flags = pte.flags();

        /*
         * Calling mark_cow twice is harmless. This can happen when a
         * process that already owns COW pages forks again.
         */
        if pte.is_cow() {
            assert!(
                !flags.contains(PTEFlags::W),
                "COW page must not be hardware-writable"
            );
            return;
        }

        assert!(
            flags.contains(PTEFlags::W),
            "mark_cow: vpn {:?} is not writable",
            vpn
        );

        flags.remove(PTEFlags::W);
        *pte = PageTableEntry::new_cow(ppn, flags);
    }

    /*
     * Replace a COW mapping with a writable private mapping.
     *
     * `new_ppn` can either be a newly copied physical page or the old
     * page when its reference count has already fallen to one.
     */
    pub fn resolve_cow(&mut self, vpn: VirtPageNum, new_ppn: PhysPageNum) {
        let pte = self
            .find_pte(vpn)
            .expect("resolve_cow: page-table entry not found");

        assert!(pte.is_valid(), "resolve_cow: vpn {:?} is not mapped", vpn);

        assert!(pte.is_cow(), "resolve_cow: vpn {:?} is not a COW page", vpn);

        let mut flags = pte.flags();
        flags.insert(PTEFlags::W);
        flags.insert(PTEFlags::V);

        /*
         * PageTableEntry::new does not include PTE_COW, so resolving the
         * fault also removes the software COW marker.
         */
        *pte = PageTableEntry::new(new_ppn, flags);
    }

    #[allow(unused)]
    pub fn unmap(&mut self, vpn: VirtPageNum) {
        let pte = self.find_pte(vpn).unwrap();

        assert!(pte.is_valid(), "vpn {:?} is invalid before unmapping", vpn);

        *pte = PageTableEntry::empty();
    }

    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.find_pte(vpn).map(|pte| *pte)
    }

    pub fn translate_va(&self, va: VirtAddr) -> Option<PhysAddr> {
        self.find_pte(va.floor()).map(|pte| {
            let aligned_pa: PhysAddr = pte.ppn().into();
            let offset = va.page_offset();
            let aligned_pa_usize: usize = aligned_pa.into();

            (aligned_pa_usize + offset).into()
        })
    }

    pub fn token(&self) -> usize {
        8usize << 60 | self.root_ppn.0
    }
}

pub fn translated_byte_buffer(token: usize, ptr: *const u8, len: usize) -> Vec<&'static mut [u8]> {
    let page_table = PageTable::from_token(token);
    let mut start = ptr as usize;
    let end = start + len;
    let mut buffers = Vec::new();

    while start < end {
        let start_va = VirtAddr::from(start);
        let mut vpn = start_va.floor();

        let ppn = page_table.translate(vpn).unwrap().ppn();

        vpn.step();

        let mut end_va: VirtAddr = vpn.into();
        end_va = end_va.min(VirtAddr::from(end));

        if end_va.page_offset() == 0 {
            buffers.push(&mut ppn.get_bytes_array()[start_va.page_offset()..]);
        } else {
            buffers.push(&mut ppn.get_bytes_array()[start_va.page_offset()..end_va.page_offset()]);
        }

        start = end_va.into();
    }

    buffers
}

/// Load a zero-terminated string from another address space.
pub fn translated_str(token: usize, ptr: *const u8) -> String {
    let page_table = PageTable::from_token(token);
    let mut string = String::new();
    let mut va = ptr as usize;

    loop {
        let ch: u8 = *page_table
            .translate_va(VirtAddr::from(va))
            .unwrap()
            .get_mut();

        if ch == 0 {
            break;
        }

        string.push(ch as char);
        va += 1;
    }

    string
}

pub fn translated_ref<T>(token: usize, ptr: *const T) -> &'static T {
    let page_table = PageTable::from_token(token);

    page_table
        .translate_va(VirtAddr::from(ptr as usize))
        .unwrap()
        .get_ref()
}

pub fn translated_refmut<T>(token: usize, ptr: *mut T) -> &'static mut T {
    let page_table = PageTable::from_token(token);
    let va = ptr as usize;

    page_table
        .translate_va(VirtAddr::from(va))
        .unwrap()
        .get_mut()
}

pub struct UserBuffer {
    pub buffers: Vec<&'static mut [u8]>,
}

impl UserBuffer {
    pub fn new(buffers: Vec<&'static mut [u8]>) -> Self {
        Self { buffers }
    }

    pub fn len(&self) -> usize {
        let mut total = 0usize;

        for buffer in self.buffers.iter() {
            total += buffer.len();
        }

        total
    }
}

impl IntoIterator for UserBuffer {
    type Item = *mut u8;
    type IntoIter = UserBufferIterator;

    fn into_iter(self) -> Self::IntoIter {
        UserBufferIterator {
            buffers: self.buffers,
            current_buffer: 0,
            current_idx: 0,
        }
    }
}

pub struct UserBufferIterator {
    buffers: Vec<&'static mut [u8]>,
    current_buffer: usize,
    current_idx: usize,
}

impl Iterator for UserBufferIterator {
    type Item = *mut u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_buffer >= self.buffers.len() {
            return None;
        }

        let result = &mut self.buffers[self.current_buffer][self.current_idx] as *mut _;

        if self.current_idx + 1 == self.buffers[self.current_buffer].len() {
            self.current_idx = 0;
            self.current_buffer += 1;
        } else {
            self.current_idx += 1;
        }

        Some(result)
    }
}
