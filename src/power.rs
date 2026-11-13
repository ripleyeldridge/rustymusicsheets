//! Worker-owned, scoped power requests. Native guards never leave their worker thread.
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;
#[cfg(any(test, not(any(target_os = "linux", windows))))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
use unsupported as platform;

#[derive(Clone, Debug, Default)]
pub struct PowerStatus {
    pub requested: bool,
    pub screen: bool,
    pub suspend: bool,
    pub message: String,
}

impl PowerStatus {
    pub fn complete(&self) -> bool {
        self.requested && self.screen && self.suspend
    }
}

/// Dropping an inhibition releases its native resources on the creating thread.
trait Inhibition {
    fn status(&self) -> PowerStatus;
}

/// Only the backend crosses threads; acquired native guards do not need to be Send.
trait PowerBackend: Send + 'static {
    /// None means acquisition was cancelled; failures return a guard with failure status.
    fn acquire(&mut self, cancellation: &Cancellation) -> Option<Box<dyn Inhibition>>;
}

struct Cancellation {
    stop: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    expected: u64,
}

impl Cancellation {
    fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Acquire)
            || self.generation.load(Ordering::Acquire) != self.expected
    }
}

pub struct KeepAwake {
    enabled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    wake: Sender<()>,
    pub updates: Receiver<PowerStatus>,
    worker: Option<JoinHandle<()>>,
}

impl KeepAwake {
    pub fn new(enabled: bool, repaint: impl Fn() + Send + 'static) -> Self {
        Self::with_backend(enabled, repaint, platform::Backend)
    }

    fn with_backend(
        enabled: bool,
        repaint: impl Fn() + Send + 'static,
        mut backend: impl PowerBackend,
    ) -> Self {
        let enabled = Arc::new(AtomicBool::new(enabled));
        let generation = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (wake, requests) = bounded(1);
        let (tx, updates) = bounded(8);
        // Keep the newest status even if the UI is temporarily not polling.
        let stale = updates.clone();
        let desired = enabled.clone();
        let revision = generation.clone();
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut last = u64::MAX;
            let mut guard: Option<Box<dyn Inhibition>> = None;
            while !stopping.load(Ordering::Acquire) {
                let current = revision.load(Ordering::Acquire);
                if current != last {
                    drop(guard.take());
                    let cancellation = Cancellation {
                        stop: stopping.clone(),
                        generation: revision.clone(),
                        expected: current,
                    };
                    let status = if desired.load(Ordering::Acquire) {
                        let Some(acquired) = backend.acquire(&cancellation) else {
                            continue;
                        };
                        if cancellation.cancelled() {
                            drop(acquired);
                            continue;
                        }
                        let status = acquired.status();
                        guard = Some(acquired);
                        status
                    } else {
                        PowerStatus {
                            message: "Keep awake disabled.".into(),
                            ..Default::default()
                        }
                    };
                    if !cancellation.cancelled() {
                        if let Err(crossbeam_channel::TrySendError::Full(status)) =
                            tx.try_send(status)
                        {
                            let _ = stale.try_recv();
                            let _ = tx.try_send(status);
                        }
                        repaint();
                    }
                    last = current;
                }
                let _ = requests.recv_timeout(Duration::from_millis(100));
            }
            drop(guard);
        });
        Self {
            enabled,
            generation,
            stop,
            wake,
            updates,
            worker: Some(worker),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        let _ = self.wake.try_send(());
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
