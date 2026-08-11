mod round_robin;

use super::TaskControlBlock;
use alloc::boxed::Box;
use alloc::sync::Arc;

pub use round_robin::RoundRobinScheduler;

/// A scheduling policy owns the ready queues and decides when a running task
/// should give the processor to another ready task.
pub trait Scheduler: Send {
    fn name(&self) -> &'static str;
    fn enqueue(&mut self, task: Arc<TaskControlBlock>);
    fn dequeue(&mut self, now: usize) -> Option<Arc<TaskControlBlock>>;
    fn should_preempt(&mut self, current: &Arc<TaskControlBlock>, now: usize) -> bool;
    fn ready_len(&self) -> usize;
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SchedulerPolicy {
    RoundRobin,
}

pub const DEFAULT_SCHEDULER_POLICY: SchedulerPolicy = SchedulerPolicy::RoundRobin;
pub const DEFAULT_PRIORITY: usize = 4;
pub const MIN_PRIORITY: usize = 0;
pub const MAX_PRIORITY: usize = 7;

pub fn build_scheduler(policy: SchedulerPolicy) -> Box<dyn Scheduler> {
    match policy {
        SchedulerPolicy::RoundRobin => Box::new(RoundRobinScheduler::new()),
    }
}
