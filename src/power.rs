//! Scoped desktop inhibitors; no persistent power settings are modified.
use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::Duration,
};
use zbus::{
    zvariant::{DynamicType, OwnedFd},
    Connection, Message,
};

const APP: &str = "RustyMusicSheets";
const REASON: &str = "Reading and performing sheet music";

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

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
struct Service {
    destination: &'static str,
    path: &'static str,
    interface: &'static str,
    release: &'static str,
}

const GNOME: Service = Service {
    destination: "org.gnome.SessionManager",
    path: "/org/gnome/SessionManager",
    interface: "org.gnome.SessionManager",
    release: "Uninhibit",
};
const KDE: Service = Service {
    destination: "org.kde.Solid.PowerManagement",
    path: "/org/kde/Solid/PowerManagement/PolicyAgent",
    interface: "org.kde.Solid.PowerManagement.PolicyAgent",
    release: "ReleaseInhibition",
};
const SCREENSAVER: Service = Service {
    destination: "org.freedesktop.ScreenSaver",
    path: "/ScreenSaver",
    interface: "org.freedesktop.ScreenSaver",
    release: "UnInhibit",
};
const POWER: Service = Service {
    destination: "org.freedesktop.PowerManagement",
    path: "/org/freedesktop/PowerManagement/Inhibit",
    interface: "org.freedesktop.PowerManagement.Inhibit",
    release: "UnInhibit",
};
const LOGIND: Service = Service {
    destination: "org.freedesktop.login1",
    path: "/org/freedesktop/login1",
    interface: "org.freedesktop.login1.Manager",
    release: "",
};

async fn limited<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    futures_lite::future::race(future, async {
        async_io::Timer::after(Duration::from_secs(2)).await;
        Err(anyhow!("Power service did not respond within two seconds"))
    })
    .await
}

async fn call<B: serde::Serialize + DynamicType>(
    connection: &Connection,
    service: Service,
    method: &str,
    body: &B,
) -> Result<Message> {
    limited(async {
        let owner: String = connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(service.destination,),
            )
            .await?
            .body()
            .deserialize()?;
        Ok(connection
            .call_method(
                Some(owner.as_str()),
                service.path,
                Some(service.interface),
                method,
                body,
            )
            .await?)
    })
    .await
}

#[derive(Default)]
struct Inhibitors {
    session: Option<Connection>,
    cookies: Vec<(Service, String, u32)>,
    sleep_fd: Option<OwnedFd>,
    status: PowerStatus,
}

impl Inhibitors {
    async fn cookie<B: serde::Serialize + DynamicType>(
        &mut self,
        service: Service,
        method: &str,
        body: &B,
    ) -> Result<()> {
        let connection = self.session.as_ref().context("No desktop session bus")?;
        let reply = call(connection, service, method, body).await?;
        let cookie: u32 = reply.body().deserialize()?;
        let owner = reply
            .header()
            .sender()
            .context("Missing power-service owner")?
            .to_string();
        self.cookies.push((service, owner, cookie));
        Ok(())
    }

    async fn session(&mut self, connection: Connection) {
        self.session = Some(connection);
        // GNOME: 4 = suspend, 8 = session idle (including screen blanking).
        if self
            .cookie(GNOME, "Inhibit", &(APP, 0u32, REASON, 12u32))
            .await
            .is_ok()
        {
            self.status.screen = true;
            self.status.suspend = true;
            self.status.message = "Keep awake active (GNOME session).".into();
            return;
        }
        // KDE: 1 = interrupt session, 4 = change screen settings (dim/off).
        if self
            .cookie(KDE, "AddInhibition", &(5u32, APP, REASON))
            .await
            .is_ok()
        {
            self.status.screen = true;
            self.status.suspend = true;
            // Separate screen-lock inhibition on desktops exposing this API.
            let _ = self.cookie(SCREENSAVER, "Inhibit", &(APP, REASON)).await;
            self.status.message = "Keep awake active (KDE screen and session).".into();
            return;
        }
        let mut screen = self.cookie(SCREENSAVER, "Inhibit", &(APP, REASON)).await;
        if screen.is_err() {
            screen = self
                .cookie(
                    Service {
                        path: "/org/freedesktop/ScreenSaver",
                        ..SCREENSAVER
                    },
                    "Inhibit",
                    &(APP, REASON),
                )
                .await;
        }
        self.status.screen = screen.is_ok();
        self.status.suspend = self.cookie(POWER, "Inhibit", &(APP, REASON)).await.is_ok();
        self.status.message = "Keep awake requested through freedesktop services.".into();
    }

    async fn acquire() -> Self {
        let mut locks = Self::default();
        locks.status.requested = true;
        match limited(async { Ok(Connection::session().await?) }).await {
            Ok(connection) => locks.session(connection).await,
            Err(error) => {
                locks.status.message = format!("Desktop power service unavailable: {error:#}")
            }
        }
        // A desktop inhibitor is sufficient where supported. If unavailable,
        // hold logind's returned FD so suspend/idle is blocked until it closes.
        if !locks.status.suspend {
            let system = limited(async { Ok(Connection::system().await?) }).await;
            let sleep = match system {
                Ok(connection) => call(
                    &connection,
                    LOGIND,
                    "Inhibit",
                    &("sleep:idle", APP, REASON, "block"),
                )
                .await
                .and_then(|reply| Ok(reply.body().deserialize::<OwnedFd>()?)),
                Err(error) => Err(error),
            };
            match sleep {
                Ok(fd) => {
                    locks.sleep_fd = Some(fd);
                    locks.status.suspend = true;
                }
                Err(error) => locks
                    .status
                    .message
                    .push_str(&format!(" Suspend inhibition failed: {error:#}")),
            }
        }
        if !locks.status.screen {
            locks
                .status
                .message
                .push_str(" Screen dimming/timeout could not be inhibited.");
        }
        if !locks.status.suspend {
            locks.status.message.push_str(" Suspend is not inhibited.");
        }
        locks
    }

    async fn release(mut self) {
        if let Some(connection) = &self.session {
            for (service, owner, cookie) in self.cookies.drain(..).rev() {
                let _ = limited(async {
                    Ok(connection
                        .call_method(
                            Some(owner.as_str()),
                            service.path,
                            Some(service.interface),
                            service.release,
                            &(cookie,),
                        )
                        .await?)
                })
                .await;
            }
        }
        // Dropping the FD and session connection also releases locks after
        // errors; the OS closes them automatically if the process crashes.
        self.sleep_fd.take();
        self.session.take();
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
        let enabled = Arc::new(AtomicBool::new(enabled));
        let generation = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (wake, requests) = bounded(1);
        let (tx, updates) = bounded(8);
        let desired = enabled.clone();
        let revision = generation.clone();
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut last = u64::MAX;
            let mut locks: Option<Inhibitors> = None;
            while !stopping.load(Ordering::Relaxed) {
                let current = revision.load(Ordering::Acquire);
                if current != last {
                    if let Some(old) = locks.take() {
                        async_io::block_on(old.release());
                    }
                    let status = if desired.load(Ordering::Acquire) {
                        let acquired = async_io::block_on(futures_lite::future::race(
                            async { Some(Inhibitors::acquire().await) },
                            async {
                                loop {
                                    if stopping.load(Ordering::Relaxed)
                                        || revision.load(Ordering::Acquire) != current
                                    {
                                        return None;
                                    }
                                    async_io::Timer::after(Duration::from_millis(25)).await;
                                }
                            },
                        ));
                        let Some(acquired) = acquired else {
                            continue;
                        };
                        let status = acquired.status.clone();
                        locks = Some(acquired);
                        status
                    } else {
                        PowerStatus {
                            message: "Keep awake disabled.".into(),
                            ..Default::default()
                        }
                    };
                    if revision.load(Ordering::Acquire) == current {
                        let _ = tx.try_send(status);
                        repaint();
                    }
                    last = current;
                }
                let _ = requests.recv_timeout(Duration::from_millis(100));
            }
            if let Some(locks) = locks {
                async_io::block_on(locks.release());
            }
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
        self.generation.fetch_add(1, Ordering::Release);
        let _ = self.wake.try_send(());
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.wake.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
