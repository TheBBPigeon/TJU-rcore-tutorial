use super::{FrameTracker, frame_alloc, frame_allocated_count, frame_ref_count};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::{StepByOne, VPNRange};
use crate::config::{MEMORY_END, MMIO, PAGE_SIZE, TRAMPOLINE};
use crate::sync::UPIntrFreeCell;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;
use lazy_static::*;
use riscv::register::satp;

unsafe extern "C" {
    safe fn stext();
    safe fn etext();
    safe fn srodata();
    safe fn erodata();
    safe fn sdata();
    safe fn edata();
    safe fn sbss_with_stack();
    safe fn ebss();
    safe fn ekernel();
    safe fn strampoline();
}

lazy_static! {
    pub static ref KERNEL_SPACE: Arc<UPIntrFreeCell<MemorySet>> =
        Arc::new(unsafe { UPIntrFreeCell::new(MemorySet::new_kernel()) });
}

pub fn kernel_token() -> usize {
    KERNEL_SPACE.exclusive_access().token()
}

pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}

#[derive(Debug, Clone, Copy)]
pub struct CowForkStats {
    pub shared_pages: usize,
    pub copied_pages: usize,
    pub allocated_frames: usize,
}

impl MemorySet {
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// Assume that no conflicts.
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) {
        self.push(
            MapArea::new(start_va, end_va, MapType::Framed, permission),
            None,
        );
    }
    pub fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some((idx, area)) = self
            .areas
            .iter_mut()
            .enumerate()
            .find(|(_, area)| area.vpn_range.get_start() == start_vpn)
        {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
        }
    }
    /// Add a new MapArea into this MemorySet.
    /// Assuming that there are no conflicts in the virtual address
    /// space.
    pub fn push(&mut self, mut map_area: MapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data);
        }
        self.areas.push(map_area);
    }
    /// Mention that trampoline is not collected by areas.
    fn map_trampoline(&mut self) {
        self.page_table.map(
            VirtAddr::from(TRAMPOLINE).into(),
            PhysAddr::from(linker_symbol_addr!(strampoline)).into(),
            PTEFlags::R | PTEFlags::X,
        );
    }
    /// Without kernel stacks.
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map kernel sections
        // println!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        // println!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        // println!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        // println!(
        //     ".bss [{:#x}, {:#x})",
        //     sbss_with_stack as usize, ebss as usize
        // );
        // println!("mapping .text section");
        memory_set.push(
            MapArea::new(
                linker_symbol_addr!(stext).into(),
                linker_symbol_addr!(etext).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
            ),
            None,
        );
        // println!("mapping .rodata section");
        memory_set.push(
            MapArea::new(
                linker_symbol_addr!(srodata).into(),
                linker_symbol_addr!(erodata).into(),
                MapType::Identical,
                MapPermission::R,
            ),
            None,
        );
        // println!("mapping .data section");
        memory_set.push(
            MapArea::new(
                linker_symbol_addr!(sdata).into(),
                linker_symbol_addr!(edata).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        // println!("mapping .bss section");
        memory_set.push(
            MapArea::new(
                linker_symbol_addr!(sbss_with_stack).into(),
                linker_symbol_addr!(ebss).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        // println!("mapping physical memory");
        memory_set.push(
            MapArea::new(
                linker_symbol_addr!(ekernel).into(),
                MEMORY_END.into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        //println!("mapping memory-mapped registers");
        for pair in MMIO {
            memory_set.push(
                MapArea::new(
                    (*pair).0.into(),
                    ((*pair).0 + (*pair).1).into(),
                    MapType::Identical,
                    MapPermission::R | MapPermission::W,
                ),
                None,
            );
        }
        memory_set
    }
    /// Include sections in elf and trampoline,
    /// also returns user_sp_base and entry point.
    pub fn from_elf(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map program headers of elf, with U flag
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_vpn = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_end_vpn = map_area.vpn_range.get_end();
                memory_set.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                );
            }
        }
        let max_end_va: VirtAddr = max_end_vpn.into();
        let mut user_stack_base: usize = max_end_va.into();
        user_stack_base += PAGE_SIZE;
        (
            memory_set,
            user_stack_base,
            elf.header.pt2.entry_point() as usize,
        )
    }
    #[allow(dead_code)]
    pub fn from_existed_user(user_space: &MemorySet) -> MemorySet {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // copy data sections/trap_context/user_stack
        for area in user_space.areas.iter() {
            let new_area = MapArea::from_another(area);
            memory_set.push(new_area, None);
            // copy data from another space
            for vpn in area.vpn_range {
                let src_ppn = user_space.translate(vpn).unwrap().ppn();
                let dst_ppn = memory_set.translate(vpn).unwrap().ppn();
                dst_ppn
                    .get_bytes_array()
                    .copy_from_slice(src_ppn.get_bytes_array());
            }
        }
        memory_set
    }
    pub fn from_existed_user_cow(user_space: &mut MemorySet) -> (MemorySet, CowForkStats) {
        let before = frame_allocated_count();
        let mut memory_set = Self::new_bare();
        memory_set.map_trampoline();
        let mut shared_pages = 0;
        let mut copied_pages = 0;

        let parent_page_table = &mut user_space.page_table;
        for area in user_space.areas.iter_mut() {
            let mut new_area = MapArea::from_another(area);
            for vpn in area.vpn_range {
                let parent_pte = parent_page_table.translate(vpn).unwrap();
                let ppn = parent_pte.ppn();
                let flags = parent_pte.flags();

                if parent_pte.user_accessible() {
                    assert_eq!(area.map_type, MapType::Framed);
                    let shared_frame = area
                        .data_frames
                        .get(&vpn)
                        .expect("user page has no FrameTracker")
                        .clone();
                    new_area.data_frames.insert(vpn, shared_frame);

                    if parent_pte.writable() || parent_pte.is_cow() {
                        parent_page_table.mark_cow(vpn);
                        memory_set.page_table.map_cow(vpn, ppn, flags);
                    } else {
                        memory_set.page_table.map(vpn, ppn, flags);
                    }
                    shared_pages += 1;
                } else {
                    new_area.map_one(&mut memory_set.page_table, vpn);
                    let dst_ppn = memory_set.page_table.translate(vpn).unwrap().ppn();
                    dst_ppn
                        .get_bytes_array()
                        .copy_from_slice(ppn.get_bytes_array());
                    copied_pages += 1;
                }
            }
            memory_set.areas.push(new_area);
        }

        unsafe { asm!("sfence.vma") };
        let allocated_frames = frame_allocated_count() - before;
        (
            memory_set,
            CowForkStats {
                shared_pages,
                copied_pages,
                allocated_frames,
            },
        )
    }
    pub fn handle_cow_fault(&mut self, fault_va: VirtAddr) -> bool {
        let vpn = fault_va.floor();
        let pte = match self.page_table.translate(vpn) {
            Some(pte) if pte.is_valid() && pte.user_accessible() && pte.is_cow() => pte,
            _ => return false,
        };
        let old_ppn = pte.ppn();

        if frame_ref_count(old_ppn) == 1 {
            self.page_table.resolve_cow(vpn, old_ppn);
            unsafe { asm!("sfence.vma") };
            return true;
        }

        let new_frame = frame_alloc().expect("COW fault: out of physical memory");
        let new_ppn = new_frame.ppn;
        new_ppn
            .get_bytes_array()
            .copy_from_slice(old_ppn.get_bytes_array());

        let area = self
            .areas
            .iter_mut()
            .find(|area| area.vpn_range.get_start() <= vpn && vpn < area.vpn_range.get_end());
        let area = match area {
            Some(area) if area.map_type == MapType::Framed => area,
            _ => return false,
        };
        let old_frame = area
            .data_frames
            .insert(vpn, new_frame)
            .expect("COW page has no FrameTracker");
        self.page_table.resolve_cow(vpn, new_ppn);
        drop(old_frame);
        unsafe { asm!("sfence.vma") };
        true
    }
    pub fn ensure_private_range(&mut self, start_va: VirtAddr, len: usize) -> bool {
        if len == 0 {
            return true;
        }
        let start: usize = start_va.into();
        let end = match start.checked_add(len) {
            Some(end) => end,
            None => return false,
        };
        let mut vpn = start_va.floor();
        let end_vpn = VirtAddr::from(end).ceil();
        while vpn < end_vpn {
            let pte = match self.page_table.translate(vpn) {
                Some(pte) if pte.is_valid() && pte.user_accessible() => pte,
                _ => return false,
            };
            if pte.is_cow() {
                if !self.handle_cow_fault(VirtAddr::from(vpn)) {
                    return false;
                }
            } else if !pte.writable() {
                return false;
            }
            vpn.step();
        }
        true
    }
    pub fn activate(&self) {
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp::Satp::from_bits(satp));
            asm!("sfence.vma");
        }
    }
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }
    pub fn recycle_data_pages(&mut self) {
        //*self = Self::new_bare();
        self.areas.clear();
    }
}

pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
}

impl MapArea {
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
        }
    }
    pub fn from_another(another: &MapArea) -> Self {
        Self {
            vpn_range: VPNRange::new(another.vpn_range.get_start(), another.vpn_range.get_end()),
            data_frames: BTreeMap::new(),
            map_type: another.map_type,
            map_perm: another.map_perm,
        }
    }
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        match self.map_type {
            MapType::Identical => {
                ppn = PhysPageNum(vpn.0);
            }
            MapType::Framed => {
                let frame = frame_alloc().unwrap();
                ppn = frame.ppn;
                self.data_frames.insert(vpn, frame);
            }
            MapType::Linear(pn_offset) => {
                // check for sv39
                assert!(vpn.0 < (1usize << 27));
                ppn = PhysPageNum((vpn.0 as isize + pn_offset) as usize);
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        if self.map_type == MapType::Framed {
            self.data_frames.remove(&vpn);
        }
        page_table.unmap(vpn);
    }
    pub fn map(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.map_one(page_table, vpn);
        }
    }
    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }
    /// data: start-aligned but maybe with shorter length
    /// assume that all frames were cleared before
    pub fn copy_data(&mut self, page_table: &PageTable, data: &[u8]) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum MapType {
    Identical,
    Framed,
    /// offset of page num
    Linear(isize),
}

bitflags! {
    pub struct MapPermission: u8 {
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
    }
}

#[allow(unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_SPACE.exclusive_access();
    let mid_text: VirtAddr = ((linker_symbol_addr!(stext) + linker_symbol_addr!(etext)) / 2).into();
    let mid_rodata: VirtAddr =
        ((linker_symbol_addr!(srodata) + linker_symbol_addr!(erodata)) / 2).into();
    let mid_data: VirtAddr = ((linker_symbol_addr!(sdata) + linker_symbol_addr!(edata)) / 2).into();
    assert!(
        !kernel_space
            .page_table
            .translate(mid_text.floor())
            .unwrap()
            .writable(),
    );
    assert!(
        !kernel_space
            .page_table
            .translate(mid_rodata.floor())
            .unwrap()
            .writable(),
    );
    assert!(
        !kernel_space
            .page_table
            .translate(mid_data.floor())
            .unwrap()
            .executable(),
    );
    println!("remap_test passed!");
}
