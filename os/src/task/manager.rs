use super::scheduler::{DEFAULT_SCHEDULER_POLICY, Scheduler, build_scheduler};
use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::sync::UPIntrFreeCell;
use crate::timer::get_time_ticks;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use lazy_static::*;

pub struct TaskManager {
    scheduler: alloc::boxed::Box<dyn Scheduler>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            scheduler: build_scheduler(DEFAULT_SCHEDULER_POLICY),
        }
    }
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        let now = get_time_ticks();
        let should_enqueue = task.inner.exclusive_session(|inner| {
            debug_assert_eq!(inner.task_status, TaskStatus::Ready);
            inner.sched_info.on_enqueue(now)
        });
        if should_enqueue {
            self.scheduler.enqueue(task);
        }
    }
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let now = get_time_ticks();
        let task = self.scheduler.dequeue(now)?;
        task.inner
            .exclusive_session(|inner| inner.sched_info.on_dispatch(now));
        Some(task)
    }
    pub fn should_preempt(&mut self, current: &Arc<TaskControlBlock>) -> bool {
        self.scheduler.should_preempt(current, get_time_ticks())
    }
    pub fn scheduler_name(&self) -> &'static str {
        self.scheduler.name()
    }
}

lazy_static! {
    pub static ref TASK_MANAGER: UPIntrFreeCell<TaskManager> =
        unsafe { UPIntrFreeCell::new(TaskManager::new()) };
    pub static ref PID2PCB: UPIntrFreeCell<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        unsafe { UPIntrFreeCell::new(BTreeMap::new()) };
}

pub fn add_task(task: Arc<TaskControlBlock>) {
    TASK_MANAGER.exclusive_access().add(task);
}

pub fn wakeup_task(task: Arc<TaskControlBlock>) {
    let mut task_inner = task.inner_exclusive_access();
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    add_task(task);
}

pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.exclusive_access().fetch()
}

pub fn should_preempt(current: &Arc<TaskControlBlock>) -> bool {
    TASK_MANAGER.exclusive_access().should_preempt(current)
}

pub fn scheduler_name() -> &'static str {
    TASK_MANAGER.exclusive_access().scheduler_name()
}

pub fn pid2process(pid: usize) -> Option<Arc<ProcessControlBlock>> {
    let map = PID2PCB.exclusive_access();
    map.get(&pid).map(Arc::clone)
}

pub fn insert_into_pid2process(pid: usize, process: Arc<ProcessControlBlock>) {
    PID2PCB.exclusive_access().insert(pid, process);
}

pub fn remove_from_pid2process(pid: usize) {
    let mut map = PID2PCB.exclusive_access();
    if map.remove(&pid).is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
}
