mod address;
mod frame_allocator;
mod heap_allocator;
mod memory_set;
mod page_table;

pub use address::VPNRange;
pub use address::{PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
#[allow(unused_imports)]
pub use frame_allocator::{
    FrameTracker, frame_add_ref, frame_alloc, frame_alloc_more, frame_allocated_count,
    frame_dealloc, frame_ref_count,
};
pub use memory_set::{
    CowForkStats, KERNEL_SPACE, MapArea, MapPermission, MapType, MemorySet, kernel_token,
};
use page_table::PTEFlags;
pub use page_table::{
    PageTable, PageTableEntry, UserBuffer, translated_byte_buffer, translated_ref,
    translated_refmut, translated_str,
};

pub fn init() {
    heap_allocator::init_heap();
    frame_allocator::init_frame_allocator();
    KERNEL_SPACE.exclusive_access().activate();
}
