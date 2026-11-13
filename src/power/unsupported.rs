use super::{Cancellation, Inhibition, PowerBackend, PowerStatus};

pub(super) struct Backend;
struct Unsupported;

impl Inhibition for Unsupported {
    fn status(&self) -> PowerStatus {
        PowerStatus {
            requested: true,
            message: format!(
                "Keep-awake protection is not implemented for {}.",
                std::env::consts::OS
            ),
            ..Default::default()
        }
    }
}

impl PowerBackend for Backend {
    fn acquire(&mut self, cancellation: &Cancellation) -> Option<Box<dyn Inhibition>> {
        if cancellation.cancelled() {
            None
        } else {
            Some(Box::new(Unsupported))
        }
    }
}
