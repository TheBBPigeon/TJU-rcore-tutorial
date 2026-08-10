use super::{PhysAddr, PhysPageNum};
use crate::config::MEMORY_END;
use crate::sync::UPIntrFreeCell;
use alloc::{collections::BTreeMap, vec::Vec};
use core::fmt::{self, Debug, Formatter};
use lazy_static::*;

pub struct FrameTracker {
    pub ppn: PhysPageNum,
}

impl Clone for FrameTracker {
    fn clone(&self) -> Self {
        frame_add_ref(self.ppn);
        Self { ppn: self.ppn }
    }
}

impl FrameTracker {
    pub fn new(ppn: PhysPageNum) -> Self {
        // Clear a newly allocated physical page.
        let bytes_array = ppn.get_bytes_array();
        for byte in bytes_array {
            *byte = 0;
        }
        Self { ppn }
    }
}

impl Debug for FrameTracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!(
            "FrameTracker:PPN={:#x},ref_count={}",
            self.ppn.0,
            frame_ref_count(self.ppn)
        ))
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}

trait FrameAllocator {
    fn new() -> Self;
    fn alloc(&mut self) -> Option<PhysPageNum>;
    fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>>;
    fn dealloc(&mut self, ppn: PhysPageNum);
}

pub struct StackFrameAllocator {
    current: usize,
    end: usize,
    recycled: Vec<usize>,

    /// Number of FrameTrackers currently referencing each allocated page.
    ///
    /// A physical page is returned to `recycled` only when its reference
    /// count reaches zero.
    ref_counts: BTreeMap<usize, usize>,
}

impl StackFrameAllocator {
    pub fn init(&mut self, l: PhysPageNum, r: PhysPageNum) {
        self.current = l.0;
        self.end = r.0;
    }

    fn add_ref(&mut self, ppn: PhysPageNum) {
        let count = self.ref_counts.get_mut(&ppn.0).unwrap_or_else(|| {
            panic!("Cannot add reference to unallocated frame ppn={:#x}", ppn.0)
        });

        *count += 1;
    }

    fn ref_count(&self, ppn: PhysPageNum) -> usize {
        self.ref_counts.get(&ppn.0).copied().unwrap_or(0)
    }

    fn allocated_count(&self) -> usize {
        self.ref_counts.len()
    }
}

impl FrameAllocator for StackFrameAllocator {
    fn new() -> Self {
        Self {
            current: 0,
            end: 0,
            recycled: Vec::new(),
            ref_counts: BTreeMap::new(),
        }
    }

    fn alloc(&mut self) -> Option<PhysPageNum> {
        let ppn = if let Some(ppn) = self.recycled.pop() {
            ppn
        } else if self.current == self.end {
            return None;
        } else {
            let ppn = self.current;
            self.current += 1;
            ppn
        };

        // A newly allocated frame starts with one owner.
        assert!(
            self.ref_counts.insert(ppn, 1).is_none(),
            "Allocated frame ppn={:#x} already has a reference count",
            ppn
        );

        Some(ppn.into())
    }

    fn alloc_more(&mut self, pages: usize) -> Option<Vec<PhysPageNum>> {
        /*
         * Keep the original allocation order.
         *
         * VirtioHal::dma_alloc() uses the last element as the base physical
         * page, so this vector must be ordered from high PPN to low PPN.
         */
        if self.current + pages >= self.end {
            None
        } else {
            self.current += pages;

            let offsets: Vec<usize> = (1..pages + 1).collect();

            let frames: Vec<PhysPageNum> = offsets
                .iter()
                .map(|offset| PhysPageNum(self.current - offset))
                .collect();

            for ppn in frames.iter() {
                assert!(
                    self.ref_counts.insert(ppn.0, 1).is_none(),
                    "Allocated frame ppn={:#x} already has a reference count",
                    ppn.0
                );
            }

            Some(frames)
        }
    }

    fn dealloc(&mut self, ppn: PhysPageNum) {
        let ppn = ppn.0;

        let should_recycle = {
            let count = self.ref_counts.get_mut(&ppn).unwrap_or_else(|| {
                panic!(
                    "Frame ppn={:#x} has not been allocated or was already freed",
                    ppn
                )
            });

            assert!(
                *count > 0,
                "Frame ppn={:#x} has an invalid zero reference count",
                ppn
            );

            *count -= 1;
            *count == 0
        };

        /*
         * A shared frame must remain allocated while either the parent or
         * child address space still owns a FrameTracker for it.
         */
        if should_recycle {
            self.ref_counts.remove(&ppn);
            self.recycled.push(ppn);
        }
    }
}

type FrameAllocatorImpl = StackFrameAllocator;

lazy_static! {
    pub static ref FRAME_ALLOCATOR: UPIntrFreeCell<FrameAllocatorImpl> =
        unsafe { UPIntrFreeCell::new(FrameAllocatorImpl::new()) };
}

pub fn init_frame_allocator() {
    unsafe extern "C" {
        safe fn ekernel();
    }

    FRAME_ALLOCATOR.exclusive_access().init(
        PhysAddr::from(linker_symbol_addr!(ekernel)).ceil(),
        PhysAddr::from(MEMORY_END).floor(),
    );
}

pub fn frame_alloc() -> Option<FrameTracker> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc()
        .map(FrameTracker::new)
}

pub fn frame_alloc_more(num: usize) -> Option<Vec<FrameTracker>> {
    FRAME_ALLOCATOR
        .exclusive_access()
        .alloc_more(num)
        .map(|pages| pages.iter().map(|&ppn| FrameTracker::new(ppn)).collect())
}

pub fn frame_dealloc(ppn: PhysPageNum) {
    FRAME_ALLOCATOR.exclusive_access().dealloc(ppn);
}

/// Increase the reference count of an allocated physical page.
pub fn frame_add_ref(ppn: PhysPageNum) {
    FRAME_ALLOCATOR.exclusive_access().add_ref(ppn);
}

/// Return the reference count of a physical page.
///
/// An unallocated or already recycled page has reference count zero.
pub fn frame_ref_count(ppn: PhysPageNum) -> usize {
    FRAME_ALLOCATOR.exclusive_access().ref_count(ppn)
}

/// Return the number of currently allocated physical pages.
///
/// This counts physical pages rather than the total number of references.
pub fn frame_allocated_count() -> usize {
    FRAME_ALLOCATOR.exclusive_access().allocated_count()
}

#[allow(unused)]
pub fn frame_allocator_test() {
    let mut frames: Vec<FrameTracker> = Vec::new();

    for _ in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        frames.push(frame);
    }

    frames.clear();

    for _ in 0..5 {
        let frame = frame_alloc().unwrap();
        println!("{:?}", frame);
        frames.push(frame);
    }

    drop(frames);
    println!("frame_allocator_test passed!");
}

#[allow(unused)]
pub fn frame_allocator_reference_test() {
    let before = frame_allocated_count();

    let frame = frame_alloc().unwrap();
    assert_eq!(frame_ref_count(frame.ppn), 1);
    assert_eq!(frame_allocated_count(), before + 1);

    let shared_frame = frame.clone();
    assert_eq!(frame_ref_count(frame.ppn), 2);
    assert_eq!(frame_allocated_count(), before + 1);

    drop(shared_frame);
    assert_eq!(frame_ref_count(frame.ppn), 1);
    assert_eq!(frame_allocated_count(), before + 1);

    let ppn = frame.ppn;
    drop(frame);

    assert_eq!(frame_ref_count(ppn), 0);
    assert_eq!(frame_allocated_count(), before);

    println!("frame_allocator_reference_test passed!");
}

#[allow(unused)]
pub fn frame_allocator_alloc_more_test() {
    let mut frames: Vec<FrameTracker> = Vec::new();

    let allocated = frame_alloc_more(5).unwrap();
    for frame in &allocated {
        println!("{:?}", frame);
    }

    frames.extend(allocated);
    frames.clear();

    let allocated = frame_alloc_more(5).unwrap();
    for frame in &allocated {
        println!("{:?}", frame);
    }

    drop(allocated);
    println!("frame_allocator_alloc_more_test passed!");
}
