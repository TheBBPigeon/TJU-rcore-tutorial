use super::Scheduler;
use crate::task::TaskControlBlock;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

/// Keep the original rCore behavior: every timer tick ends the current time
/// slice when another task is ready.
const ROUND_ROBIN_TIME_SLICE: usize = 1;

pub struct RoundRobinScheduler {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl RoundRobinScheduler {
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
}

impl Scheduler for RoundRobinScheduler {
    fn name(&self) -> &'static str {
        "round-robin"
    }

    fn enqueue(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }

    fn dequeue(&mut self, _now: usize) -> Option<Arc<TaskControlBlock>> {
        self.ready_queue.pop_front()
    }

    fn should_preempt(&mut self, current: &Arc<TaskControlBlock>, _now: usize) -> bool {
        if self.ready_queue.is_empty() {
            return false;
        }
        current.inner_exclusive_access().sched_info.slice_ticks >= ROUND_ROBIN_TIME_SLICE
    }

    fn ready_len(&self) -> usize {
        self.ready_queue.len()
    }
}
