use super::{Cancellation, Inhibition, PowerBackend, PowerStatus};
use std::{marker::PhantomData, rc::Rc};
use windows_sys::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
};

pub(super) struct Backend;

struct WindowsInhibition {
    previous: u32,
    status: PowerStatus,
    // SetThreadExecutionState requests belong to the calling thread.
    _thread_bound: PhantomData<Rc<()>>,
}

impl Inhibition for WindowsInhibition {
    fn status(&self) -> PowerStatus {
        self.status.clone()
    }
}

impl Drop for WindowsInhibition {
    fn drop(&mut self) {
        if self.previous != 0 {
            // SAFETY: The guard is created and dropped on the same dedicated worker.
            // Restore that thread's previous execution requirements, not global settings.
            unsafe {
                SetThreadExecutionState(self.previous | ES_CONTINUOUS);
            }
        }
    }
}

impl PowerBackend for Backend {
    fn acquire(&mut self, cancellation: &Cancellation) -> Option<Box<dyn Inhibition>> {
        if cancellation.cancelled() {
            return None;
        }
        // SAFETY: No pointers are used. Continuous requests are scoped to this worker.
        let previous = unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
        };
        let accepted = previous != 0;
        Some(Box::new(WindowsInhibition {
            previous,
            status: PowerStatus {
                requested: true,
                screen: accepted,
                suspend: accepted,
                message: if accepted {
                    "Keep awake active (Windows display and idle sleep). Screen savers and forced sleep are not blocked."
                } else {
                    "Windows rejected the keep-awake request; display timeout and idle sleep are not inhibited."
                }
                .into(),
            },
            _thread_bound: PhantomData,
        }))
    }
}
