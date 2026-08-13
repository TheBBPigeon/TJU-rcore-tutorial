use super::{AGING_INTERVAL_TICKS, MAX_PRIORITY, PRIORITY_LEVELS, Scheduler, TIME_SLICE_TICKS};
use crate::task::TaskControlBlock;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

/// Strict priority scheduling with FIFO/Round-Robin ordering inside each
/// priority level. Waiting tasks are periodically promoted by aging.
pub struct PriorityScheduler {
    ready_queues: [VecDeque<Arc<TaskControlBlock>>; PRIORITY_LEVELS],
    ready_tasks: usize,
}

impl PriorityScheduler {
    pub fn new() -> Self {
        Self {
            ready_queues: core::array::from_fn(|_| VecDeque::new()),
            ready_tasks: 0,
        }
    }

    fn refresh_aging(&mut self, now: usize) {
        // Walk from high to low. A promoted task is moved to a queue that has
        // already been visited, so each ready task is examined exactly once.
        for old_priority in (0..PRIORITY_LEVELS).rev() {
            let tasks_at_level = self.ready_queues[old_priority].len();
            for _ in 0..tasks_at_level {
                let task = self.ready_queues[old_priority]
                    .pop_front()
                    .expect("ready queue length changed during aging");
                let new_priority = task.inner.exclusive_session(|inner| {
                    debug_assert!(inner.sched_info.in_ready_queue);
                    inner.sched_info.refresh_effective_priority(now)
                });
                self.ready_queues[new_priority].push_back(task);
            }
        }
    }

    fn highest_ready_priority(&self) -> Option<usize> {
        (0..PRIORITY_LEVELS)
            .rev()
            .find(|priority| !self.ready_queues[*priority].is_empty())
    }
}

impl Scheduler for PriorityScheduler {
    fn name(&self) -> &'static str {
        "priority-aging"
    }

    fn enqueue(&mut self, task: Arc<TaskControlBlock>) {
        let priority = task
            .inner_exclusive_access()
            .sched_info
            .effective_priority
            .min(MAX_PRIORITY);
        self.ready_queues[priority].push_back(task);
        self.ready_tasks += 1;
    }

    fn dequeue(&mut self, now: usize) -> Option<Arc<TaskControlBlock>> {
        if self.ready_tasks == 0 {
            return None;
        }
        self.refresh_aging(now);
        let priority = self.highest_ready_priority()?;
        self.ready_tasks -= 1;
        self.ready_queues[priority].pop_front()
    }

    fn should_preempt(&mut self, current: &Arc<TaskControlBlock>, now: usize) -> bool {
        if self.ready_tasks == 0 {
            return false;
        }
        self.refresh_aging(now);
        let highest_ready = self
            .highest_ready_priority()
            .expect("ready task count and queues disagree");
        let current_inner = current.inner_exclusive_access();
        current_inner.sched_info.slice_ticks >= TIME_SLICE_TICKS
            || highest_ready > current_inner.sched_info.effective_priority
    }
}

const _: () = assert!(AGING_INTERVAL_TICKS > 0);
