mod priority;
mod round_robin;

use super::TaskControlBlock;
use alloc::boxed::Box;
use alloc::sync::Arc;

pub use priority::PriorityScheduler;
pub use round_robin::RoundRobinScheduler;

/// A scheduling policy owns the ready queues and decides when a running task
/// should give the processor to another ready task.
pub trait Scheduler: Send {
    fn name(&self) -> &'static str;
    fn enqueue(&mut self, task: Arc<TaskControlBlock>);
    fn dequeue(&mut self, now: usize) -> Option<Arc<TaskControlBlock>>;
    fn should_preempt(&mut self, current: &Arc<TaskControlBlock>, now: usize) -> bool;
}

#[allow(dead_code)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SchedulerPolicy {
    RoundRobin,
    PriorityAging,
}

pub const DEFAULT_SCHEDULER_POLICY: SchedulerPolicy = SchedulerPolicy::PriorityAging;
pub const DEFAULT_PRIORITY: usize = 4;
pub const MIN_PRIORITY: usize = 0;
pub const MAX_PRIORITY: usize = 7;
pub const PRIORITY_LEVELS: usize = MAX_PRIORITY + 1;
pub const TIME_SLICE_TICKS: usize = 2;
pub const AGING_INTERVAL_TICKS: usize = 20;

pub fn build_scheduler(policy: SchedulerPolicy) -> Box<dyn Scheduler> {
    match policy {
        SchedulerPolicy::RoundRobin => Box::new(RoundRobinScheduler::new()),
        SchedulerPolicy::PriorityAging => Box::new(PriorityScheduler::new()),
    }
}
