use super::*;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

struct TestBus {
    daemon: Child,
    address: String,
}

impl TestBus {
    fn new() -> Self {
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("Tests require dbus-daemon");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            daemon,
            address: address.trim().to_owned(),
        }
    }

    fn client(&self) -> Connection {
        async_io::block_on(
            zbus::connection::Builder::address(self.address.as_str())
                .unwrap()
                .build(),
        )
        .unwrap()
    }
}

impl Drop for TestBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

struct Gnome {
    flags: Arc<AtomicU32>,
    released: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.gnome.SessionManager")]
impl Gnome {
    fn inhibit(&self, app: &str, xid: u32, reason: &str, flags: u32) -> u32 {
        assert_eq!(app, APP);
        assert_eq!(xid, 0);
        assert_eq!(reason, REASON);
        self.flags.store(flags, Ordering::SeqCst);
        42
    }
    fn uninhibit(&self, cookie: u32) {
        assert_eq!(cookie, 42);
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

struct Kde {
    flags: Arc<AtomicU32>,
    released: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.kde.Solid.PowerManagement.PolicyAgent")]
impl Kde {
    fn add_inhibition(&self, flags: u32, app: &str, reason: &str) -> u32 {
        assert_eq!(app, APP);
        assert_eq!(reason, REASON);
        self.flags.store(flags, Ordering::SeqCst);
        23
    }
    fn release_inhibition(&self, cookie: u32) {
        assert_eq!(cookie, 23);
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

struct ScreenSaver(Arc<AtomicU32>);

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaver {
    fn inhibit(&self, app: &str, reason: &str) -> u32 {
        assert_eq!(app, APP);
        assert_eq!(reason, REASON);
        self.0.fetch_add(1, Ordering::SeqCst);
        19
    }
    #[zbus(name = "UnInhibit")]
    fn uninhibit(&self, cookie: u32) {
        assert_eq!(cookie, 19);
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Power(Arc<AtomicU32>);

#[zbus::interface(name = "org.freedesktop.PowerManagement.Inhibit")]
impl Power {
    fn inhibit(&self, app: &str, reason: &str) -> u32 {
        assert_eq!(app, APP);
        assert_eq!(reason, REASON);
        self.0.fetch_add(1, Ordering::SeqCst);
        29
    }
    #[zbus(name = "UnInhibit")]
    fn uninhibit(&self, cookie: u32) {
        assert_eq!(cookie, 29);
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn cancelled_future_releases_an_already_acquired_cookie() {
    let bus = TestBus::new();
    let released = Arc::new(AtomicU32::new(0));
    let _server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(GNOME.destination)
        .unwrap()
        .serve_at(
            GNOME.path,
            Gnome {
                flags: Arc::new(AtomicU32::new(0)),
                released: released.clone(),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    let ready = std::sync::atomic::AtomicBool::new(false);
    async_io::block_on(futures_lite::future::race(
        async {
            let mut guard = Inhibitors::default();
            guard.session(bus.client()).await;
            ready.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        },
        async {
            while !ready.load(Ordering::SeqCst) {
                async_io::Timer::after(Duration::from_millis(1)).await;
            }
        },
    ));
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn gnome_requests_idle_and_suspend_and_releases_its_cookie() {
    let bus = TestBus::new();
    let flags = Arc::new(AtomicU32::new(0));
    let released = Arc::new(AtomicU32::new(0));
    let _server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(GNOME.destination)
        .unwrap()
        .serve_at(
            GNOME.path,
            Gnome {
                flags: flags.clone(),
                released: released.clone(),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    let mut locks = Inhibitors::default();
    locks.status.requested = true;
    async_io::block_on(locks.session(bus.client()));
    assert_eq!(flags.load(Ordering::SeqCst), 12);
    assert!(locks.status.complete());
    assert_eq!(released.load(Ordering::SeqCst), 0);
    async_io::block_on(locks.release());
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn kde_requests_dimming_and_suspend_plus_screen_lock_inhibition() {
    let bus = TestBus::new();
    let flags = Arc::new(AtomicU32::new(0));
    let released = Arc::new(AtomicU32::new(0));
    let screensaver = Arc::new(AtomicU32::new(0));
    let _server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(KDE.destination)
        .unwrap()
        .name(SCREENSAVER.destination)
        .unwrap()
        .serve_at(
            KDE.path,
            Kde {
                flags: flags.clone(),
                released: released.clone(),
            },
        )
        .unwrap()
        .serve_at(SCREENSAVER.path, ScreenSaver(screensaver.clone()))
        .unwrap()
        .build()
        .unwrap();
    let mut locks = Inhibitors::default();
    locks.status.requested = true;
    async_io::block_on(locks.session(bus.client()));
    assert!(locks.status.complete());
    assert_eq!(flags.load(Ordering::SeqCst), 5);
    assert_eq!(screensaver.load(Ordering::SeqCst), 1);
    async_io::block_on(locks.release());
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(screensaver.load(Ordering::SeqCst), 2);
}

#[test]
fn freedesktop_fallback_handles_both_locks_and_missing_services() {
    let bus = TestBus::new();
    let screensaver = Arc::new(AtomicU32::new(0));
    let power = Arc::new(AtomicU32::new(0));
    let server = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(SCREENSAVER.destination)
        .unwrap()
        .name(POWER.destination)
        .unwrap()
        .serve_at(SCREENSAVER.path, ScreenSaver(screensaver.clone()))
        .unwrap()
        .serve_at(POWER.path, Power(power.clone()))
        .unwrap()
        .build()
        .unwrap();
    let mut locks = Inhibitors::default();
    locks.status.requested = true;
    async_io::block_on(locks.session(bus.client()));
    assert!(locks.status.complete());
    async_io::block_on(locks.release());
    assert_eq!(screensaver.load(Ordering::SeqCst), 2);
    assert_eq!(power.load(Ordering::SeqCst), 2);
    drop(server);
    let mut missing = Inhibitors::default();
    async_io::block_on(missing.session(bus.client()));
    assert!(!missing.status.screen);
    assert!(!missing.status.suspend);
    assert!(missing.cookies.is_empty());
}
