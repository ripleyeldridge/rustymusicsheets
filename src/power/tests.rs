use super::*;
use crossbeam_channel::{unbounded, Sender};
use std::thread::ThreadId;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Acquire(ThreadId),
    Release(ThreadId),
    Cancel(ThreadId),
}

struct FakeBackend {
    events: Sender<Event>,
    wait_for_cancel: bool,
    fail: bool,
}

struct FakeInhibition {
    events: Sender<Event>,
    status: PowerStatus,
}

impl Inhibition for FakeInhibition {
    fn status(&self) -> PowerStatus {
        self.status.clone()
    }
}

impl Drop for FakeInhibition {
    fn drop(&mut self) {
        let _ = self
            .events
            .send(Event::Release(std::thread::current().id()));
    }
}

impl PowerBackend for FakeBackend {
    fn acquire(&mut self, cancellation: &Cancellation) -> Option<Box<dyn Inhibition>> {
        let thread = std::thread::current().id();
        self.events.send(Event::Acquire(thread)).unwrap();
        if self.wait_for_cancel {
            while !cancellation.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.events.send(Event::Cancel(thread)).unwrap();
            return None;
        }
        Some(Box::new(FakeInhibition {
            events: self.events.clone(),
            status: PowerStatus {
                requested: true,
                screen: !self.fail,
                suspend: !self.fail,
                message: if self.fail {
                    "test failure"
                } else {
                    "test protection"
                }
                .into(),
            },
        }))
    }
}

fn backend() -> (FakeBackend, Receiver<Event>) {
    let (events, receiver) = unbounded();
    (
        FakeBackend {
            events,
            wait_for_cancel: false,
            fail: false,
        },
        receiver,
    )
}

fn next_event(events: &Receiver<Event>) -> Event {
    events.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn next_status(worker: &KeepAwake) -> PowerStatus {
    worker.updates.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn disabled_worker_does_not_acquire_power_locks() {
    let (backend, events) = backend();
    let worker = KeepAwake::with_backend(false, || {}, backend);
    assert!(!next_status(&worker).requested);
    worker.set_enabled(false);
    assert!(!next_status(&worker).requested);
    drop(worker);
    assert!(events.try_recv().is_err());
}

#[test]
fn disable_retry_and_shutdown_release_on_the_acquiring_thread() {
    let (backend, events) = backend();
    let worker = KeepAwake::with_backend(true, || {}, backend);
    let Event::Acquire(thread) = next_event(&events) else {
        panic!("Expected acquisition")
    };
    assert_ne!(thread, std::thread::current().id());
    assert!(next_status(&worker).complete());
    worker.set_enabled(true);
    assert_eq!(next_event(&events), Event::Release(thread));
    assert_eq!(next_event(&events), Event::Acquire(thread));
    assert!(next_status(&worker).complete());
    worker.set_enabled(false);
    assert_eq!(next_event(&events), Event::Release(thread));
    assert!(!next_status(&worker).requested);
    worker.set_enabled(true);
    assert_eq!(next_event(&events), Event::Acquire(thread));
    assert!(next_status(&worker).complete());
    drop(worker);
    assert_eq!(next_event(&events), Event::Release(thread));
    assert!(events.try_recv().is_err());
}

#[test]
fn cancellation_during_acquisition_never_publishes_active_protection() {
    let (mut backend, events) = backend();
    backend.wait_for_cancel = true;
    let worker = KeepAwake::with_backend(true, || {}, backend);
    let Event::Acquire(thread) = next_event(&events) else {
        panic!("Expected acquisition")
    };
    worker.set_enabled(false);
    assert_eq!(next_event(&events), Event::Cancel(thread));
    let status = next_status(&worker);
    assert!(!status.requested && !status.complete());
    drop(worker);
    assert!(events.try_recv().is_err());
}

#[test]
fn dropping_worker_cancels_an_inflight_request() {
    let (mut backend, events) = backend();
    backend.wait_for_cancel = true;
    let worker = KeepAwake::with_backend(true, || {}, backend);
    let Event::Acquire(thread) = next_event(&events) else {
        panic!("Expected acquisition")
    };
    drop(worker);
    assert_eq!(next_event(&events), Event::Cancel(thread));
    assert!(events.try_recv().is_err());
}

#[test]
fn failed_backend_reports_failure_and_can_be_disabled() {
    let (mut backend, events) = backend();
    backend.fail = true;
    let worker = KeepAwake::with_backend(true, || {}, backend);
    let Event::Acquire(thread) = next_event(&events) else {
        panic!("Expected acquisition")
    };
    let status = next_status(&worker);
    assert!(status.requested && !status.complete());
    assert_eq!(status.message, "test failure");
    worker.set_enabled(false);
    assert_eq!(next_event(&events), Event::Release(thread));
    assert!(!next_status(&worker).requested);
}

#[test]
fn slow_ui_still_receives_the_latest_status() {
    let (backend, _events) = backend();
    let (repaint, repainted) = unbounded();
    let worker = KeepAwake::with_backend(
        false,
        move || {
            repaint.send(()).unwrap();
        },
        backend,
    );
    repainted.recv_timeout(Duration::from_secs(5)).unwrap();
    for index in 0..20 {
        worker.set_enabled(index % 2 == 0);
        repainted.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    let statuses: Vec<_> = worker.updates.try_iter().collect();
    assert!(!statuses.is_empty());
    assert!(!statuses.last().unwrap().requested);
    assert!(statuses.len() <= 8);
}

#[test]
fn unsupported_backend_never_claims_protection() {
    let worker = KeepAwake::with_backend(true, || {}, unsupported::Backend);
    let status = next_status(&worker);
    assert!(status.requested);
    assert!(!status.screen && !status.suspend && !status.complete());
    assert!(status.message.contains("not implemented"));
}
