use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub struct Controller {
    busy: AtomicBool,
    current_task: Mutex<Option<u64>>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            current_task: Mutex::new(None),
        }
    }

    pub fn submit(&self, handle: u64) -> bool {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        *self.current_task.lock().unwrap() = Some(handle);
        true
    }

    pub fn finish(&self) {
        self.busy.store(false, Ordering::Release);
        self.current_task.lock().unwrap().take();
    }

    pub fn current_handle(&self) -> Option<u64> {
        *self.current_task.lock().unwrap()
    }
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_admission_works() {
        let controller = Controller::new();
        assert!(controller.submit(1));
        controller.finish();
        assert!(controller.submit(2));
        assert_eq!(controller.current_handle(), Some(2));
    }
}
