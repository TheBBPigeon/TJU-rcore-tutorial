use super::id::TaskUserRes;
use super::{KernelStack, ProcessControlBlock, TaskContext, kstack_alloc};
use crate::trap::TrapContext;
use crate::{
    mm::PhysPageNum,
    sync::{UPIntrFreeCell, UPIntrRefMut},
};
use alloc::sync::{Arc, Weak};

use super::scheduler::{AGING_INTERVAL_TICKS, DEFAULT_PRIORITY, MAX_PRIORITY, MIN_PRIORITY};

#[derive(Copy, Clone, Debug)]
pub struct SchedInfo {
    pub base_priority: usize,
    pub effective_priority: usize,
    pub ready_since: usize,
    pub runtime_ticks: usize,
    pub total_wait_ticks: usize,
    pub scheduled_count: usize,
    pub slice_ticks: usize,
    pub in_ready_queue: bool,
}

impl SchedInfo {
    pub fn new(priority: usize) -> Self {
        assert!((MIN_PRIORITY..=MAX_PRIORITY).contains(&priority));
        Self {
            base_priority: priority,
            effective_priority: priority,
            ready_since: 0,
            runtime_ticks: 0,
            total_wait_ticks: 0,
            scheduled_count: 0,
            slice_ticks: 0,
            in_ready_queue: false,
        }
    }

    pub fn on_enqueue(&mut self, now: usize) -> bool {
        if self.in_ready_queue {
            return false;
        }
        self.ready_since = now;
        self.slice_ticks = 0;
        self.in_ready_queue = true;
        true
    }

    pub fn on_dispatch(&mut self, now: usize) {
        debug_assert!(self.in_ready_queue);
        self.total_wait_ticks = self
            .total_wait_ticks
            .saturating_add(now.saturating_sub(self.ready_since));
        self.scheduled_count = self.scheduled_count.saturating_add(1);
        self.slice_ticks = 0;
        self.in_ready_queue = false;
        self.effective_priority = self.base_priority;
    }

    pub fn on_timer_tick(&mut self) {
        self.runtime_ticks = self.runtime_ticks.saturating_add(1);
        self.slice_ticks = self.slice_ticks.saturating_add(1);
    }

    pub fn refresh_effective_priority(&mut self, now: usize) -> usize {
        if self.in_ready_queue {
            let waited_ticks = now.saturating_sub(self.ready_since);
            let aging_boost = waited_ticks / AGING_INTERVAL_TICKS;
            self.effective_priority = self
                .base_priority
                .saturating_add(aging_boost)
                .min(MAX_PRIORITY);
        } else {
            self.effective_priority = self.base_priority;
        }
        self.effective_priority
    }

    pub fn on_block(&mut self) {
        self.slice_ticks = 0;
        self.in_ready_queue = false;
        self.effective_priority = self.base_priority;
    }

    pub fn set_base_priority(&mut self, priority: usize) {
        self.base_priority = priority;
        self.effective_priority = priority;
    }
}

pub struct TaskControlBlock {
    // immutable
    pub process: Weak<ProcessControlBlock>,
    pub kstack: KernelStack,
    // mutable
    pub inner: UPIntrFreeCell<TaskControlBlockInner>,
}

impl TaskControlBlock {
    pub fn inner_exclusive_access(&self) -> UPIntrRefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }

    pub fn get_user_token(&self) -> usize {
        let process = self.process.upgrade().unwrap();
        let inner = process.inner_exclusive_access();
        inner.memory_set.token()
    }
}

pub struct TaskControlBlockInner {
    pub res: Option<TaskUserRes>,
    pub trap_cx_ppn: PhysPageNum,
    pub task_cx: TaskContext,
    pub task_status: TaskStatus,
    pub exit_code: Option<i32>,
    pub sched_info: SchedInfo,
}

impl TaskControlBlockInner {
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }

    #[allow(unused)]
    fn get_status(&self) -> TaskStatus {
        self.task_status
    }
}

impl TaskControlBlock {
    pub fn new(
        process: Arc<ProcessControlBlock>,
        ustack_base: usize,
        alloc_user_res: bool,
    ) -> Self {
        Self::new_with_priority(process, ustack_base, alloc_user_res, DEFAULT_PRIORITY)
    }

    pub fn new_with_priority(
        process: Arc<ProcessControlBlock>,
        ustack_base: usize,
        alloc_user_res: bool,
        priority: usize,
    ) -> Self {
        let res = TaskUserRes::new(Arc::clone(&process), ustack_base, alloc_user_res);
        let trap_cx_ppn = res.trap_cx_ppn();
        let kstack = kstack_alloc();
        let kstack_top = kstack.get_top();
        Self {
            process: Arc::downgrade(&process),
            kstack,
            inner: unsafe {
                UPIntrFreeCell::new(TaskControlBlockInner {
                    res: Some(res),
                    trap_cx_ppn,
                    task_cx: TaskContext::goto_trap_return(kstack_top),
                    task_status: TaskStatus::Ready,
                    exit_code: None,
                    sched_info: SchedInfo::new(priority),
                })
            },
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TaskStatus {
    Ready,
    Running,
    Blocked,
}
