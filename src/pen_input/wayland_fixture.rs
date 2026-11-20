//! A private compositor on a socket pair; never connects to the desktop.
use super::*;
use std::{
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::wl_compositor,
};
use wayland_protocols::wp::tablet::zv2::server::{
    zwp_tablet_manager_v2 as s_manager, zwp_tablet_seat_v2 as s_tablet_seat,
    zwp_tablet_tool_v2 as s_tool, zwp_tablet_v2 as s_tablet,
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{wl_compositor as s_compositor, wl_seat as s_seat, wl_surface as s_surface},
    Client, DataInit, Dispatch as ServerDispatch, Display, DisplayHandle, GlobalDispatch, New,
    Resource,
};

#[derive(Default)]
struct Host;
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Host {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(Host: ignore wl_compositor::WlCompositor);
delegate_noop!(Host: ignore wl_surface::WlSurface);

struct ClientInfo;
impl ClientData for ClientInfo {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

enum Command {
    Tap(bool),
    Hover,
    Pressure,
    Removed,
    Stop,
}

struct Server {
    surfaces: Vec<s_surface::WlSurface>,
    tablet: Option<s_tablet::ZwpTabletV2>,
    tool: Option<s_tool::ZwpTabletToolV2>,
    tool_destroyed: Arc<AtomicUsize>,
    surface_destroyed: Arc<AtomicUsize>,
}

impl GlobalDispatch<s_compositor::WlCompositor, ()> for Server {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<s_compositor::WlCompositor>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}
impl ServerDispatch<s_compositor::WlCompositor, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &s_compositor::WlCompositor,
        request: s_compositor::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let s_compositor::Request::CreateSurface { id } = request {
            state.surfaces.push(init.init(id, ()));
        }
    }
}
impl ServerDispatch<s_surface::WlSurface, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &s_surface::WlSurface,
        request: s_surface::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, s_surface::Request::Destroy) {
            state.surface_destroyed.fetch_add(1, Ordering::Relaxed);
        }
    }
}
impl GlobalDispatch<s_seat::WlSeat, ()> for Server {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<s_seat::WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(s_seat::Capability::empty());
        seat.name("generated-seat".into());
    }
}
impl ServerDispatch<s_seat::WlSeat, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &s_seat::WlSeat,
        _: s_seat::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
impl GlobalDispatch<s_manager::ZwpTabletManagerV2, ()> for Server {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<s_manager::ZwpTabletManagerV2>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}
impl ServerDispatch<s_manager::ZwpTabletManagerV2, ()> for Server {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &s_manager::ZwpTabletManagerV2,
        request: s_manager::Request,
        _: &(),
        handle: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let s_manager::Request::GetTabletSeat { tablet_seat, .. } = request {
            let seat = init.init(tablet_seat, ());
            let tablet = client
                .create_resource::<s_tablet::ZwpTabletV2, (), Self>(handle, 1, ())
                .unwrap();
            let tool = client
                .create_resource::<s_tool::ZwpTabletToolV2, (), Self>(handle, 1, ())
                .unwrap();
            seat.tablet_added(&tablet);
            tablet.done();
            seat.tool_added(&tool);
            tool.send_event(s_tool::Event::Type {
                tool_type: wayland_server::WEnum::Value(s_tool::Type::Pen),
            })
            .unwrap();
            tool.done();
            state.tablet = Some(tablet);
            state.tool = Some(tool);
        }
    }
}
impl ServerDispatch<s_tablet_seat::ZwpTabletSeatV2, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &s_tablet_seat::ZwpTabletSeatV2,
        _: s_tablet_seat::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
impl ServerDispatch<s_tablet::ZwpTabletV2, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &s_tablet::ZwpTabletV2,
        _: s_tablet::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
impl ServerDispatch<s_tool::ZwpTabletToolV2, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &s_tool::ZwpTabletToolV2,
        request: s_tool::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, s_tool::Request::Destroy) {
            state.tool_destroyed.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Fixture {
    reader: Option<Reader>,
    host: Connection,
    surfaces: Vec<wl_surface::WlSurface>,
    commands: mpsc::Sender<(Command, mpsc::Sender<()>)>,
    thread: Option<JoinHandle<()>>,
    tool_destroyed: Arc<AtomicUsize>,
    surface_destroyed: Arc<AtomicUsize>,
}

impl Fixture {
    fn new(with_tablet: bool) -> Self {
        let (client_socket, server_socket) = UnixStream::pair().unwrap();
        let (commands, incoming) = mpsc::channel::<(Command, mpsc::Sender<()>)>();
        let tool_destroyed = Arc::new(AtomicUsize::new(0));
        let surface_destroyed = Arc::new(AtomicUsize::new(0));
        let mut server = Server {
            surfaces: Vec::new(),
            tablet: None,
            tool: None,
            tool_destroyed: tool_destroyed.clone(),
            surface_destroyed: surface_destroyed.clone(),
        };
        let thread = thread::spawn(move || {
            let mut display = Display::<Server>::new().unwrap();
            let handle = display.handle();
            handle.create_global::<Server, s_compositor::WlCompositor, _>(1, ());
            handle.create_global::<Server, s_seat::WlSeat, _>(5, ());
            if with_tablet {
                handle.create_global::<Server, s_manager::ZwpTabletManagerV2, _>(1, ());
            }
            display
                .handle()
                .insert_client(server_socket, Arc::new(ClientInfo))
                .unwrap();
            loop {
                display.dispatch_clients(&mut server).unwrap();
                display.flush_clients().unwrap();
                if let Ok((command, done)) = incoming.try_recv() {
                    match command {
                        Command::Stop => {
                            let _ = done.send(());
                            break;
                        }
                        Command::Tap(_) | Command::Hover => {
                            let tool = server.tool.as_ref().unwrap();
                            let surface = &server.surfaces
                                [usize::from(matches!(command, Command::Tap(true)))];
                            tool.proximity_in(1, server.tablet.as_ref().unwrap(), surface);
                            tool.motion(80.0, 120.0);
                            if !matches!(command, Command::Hover) {
                                tool.down(2);
                                tool.motion(100.0, 140.0);
                                tool.up();
                            }
                            tool.frame(10);
                        }
                        Command::Removed => server.tool.as_ref().unwrap().removed(),
                        Command::Pressure => {
                            let tool = server.tool.as_ref().unwrap();
                            tool.pressure(32000);
                            tool.frame(11);
                        }
                    }
                    display.flush_clients().unwrap();
                    let _ = done.send(());
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        let host = Connection::from_socket(client_socket).unwrap();
        let (globals, queue) = registry_queue_init::<Host>(&host).unwrap();
        let compositor: wl_compositor::WlCompositor =
            globals.bind(&queue.handle(), 1..=1, ()).unwrap();
        let surfaces = vec![
            compositor.create_surface(&queue.handle(), ()),
            compositor.create_surface(&queue.handle(), ()),
        ];
        host.roundtrip().unwrap();
        // SAFETY: host outlives reader, and only host reads/roundtrips below.
        let guest = unsafe {
            wayland_backend::sys::client::Backend::from_foreign_display(
                host.backend().display_ptr(),
            )
        };
        // SAFETY: the host surface outlives the guest; borrow only its identity.
        let target = unsafe {
            ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                surfaces[0].id().as_ptr(),
            )
            .unwrap()
        };
        let reader = Reader::from_guest(Connection::from_backend(guest), target).unwrap();
        let mut fixture = Self {
            reader: Some(reader),
            host,
            surfaces,
            commands,
            thread: Some(thread),
            tool_destroyed,
            surface_destroyed,
        };
        fixture
            .pump_until(|r| r.state.registry_done && (!with_tablet || !r.state.tools.is_empty()));
        fixture
    }

    fn command(&self, command: Command) {
        let (tx, rx) = mpsc::channel();
        self.commands.send((command, tx)).unwrap();
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
    }

    fn pump_until(&mut self, ready: impl Fn(&Reader) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            self.host.roundtrip().unwrap(); // The toolkit/host is the sole socket reader.
            self.reader.as_mut().unwrap().poll(true, true).unwrap();
            if ready(self.reader.as_ref().unwrap()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Fake compositor initialization timed out"
            );
        }
    }

    fn packets(&mut self) -> Vec<Packet> {
        self.host.roundtrip().unwrap();
        self.reader.as_mut().unwrap().poll(true, true).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.reader.take();
        self.host.roundtrip().unwrap();
        let (tx, rx) = mpsc::channel();
        let _ = self.commands.send((Command::Stop, tx));
        let _ = rx.recv_timeout(Duration::from_secs(3));
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[test]
fn real_guest_queue_dispatches_native_tap_and_preserves_host_display_and_surface() {
    let mut fixture = Fixture::new(true);
    fixture.command(Command::Tap(false));
    assert!(
        fixture
            .reader
            .as_mut()
            .unwrap()
            .poll(true, true)
            .unwrap()
            .is_empty(),
        "Guest must not read events still waiting on the host socket"
    );
    let packets = fixture.packets();
    let buttons: Vec<_> = packets
        .iter()
        .filter_map(|p| match p.kind {
            Kind::Button { pressed, .. } => Some(pressed),
            _ => None,
        })
        .collect();
    assert_eq!(buttons, [true, false]);
    assert!(fixture
        .reader
        .as_ref()
        .unwrap()
        .status()
        .contains("tablet-v2 bound"));
    fixture.reader.take();
    fixture.host.roundtrip().unwrap();
    assert_eq!(fixture.tool_destroyed.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.surface_destroyed.load(Ordering::Relaxed), 0);
    assert!(fixture.surfaces[0].is_alive());
    fixture.host.roundtrip().unwrap();
}

#[test]
fn native_hover_has_visible_app_local_cursor_without_making_contact() {
    let mut fixture = Fixture::new(true);
    fixture.command(Command::Hover);
    fixture.packets();
    let mut input = super::super::PenInput::default();
    input.wayland = fixture.reader.take();
    let ctx = eframe::egui::Context::default();
    let output = ctx.run(
        eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(
                Pos2::ZERO,
                eframe::egui::vec2(300.0, 300.0),
            )),
            ..Default::default()
        },
        |ctx| input.paint_cursor(ctx),
    );
    assert_eq!(output.shapes.len(), 4);
    assert!(input.translator.active.is_none());
    input.shutdown();
    fixture.host.roundtrip().unwrap();
}

#[test]
fn real_guest_ignores_foreign_surface_and_reports_missing_protocol() {
    let mut fixture = Fixture::new(true);
    fixture.command(Command::Tap(true));
    assert!(!fixture
        .packets()
        .iter()
        .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
    fixture.command(Command::Hover);
    assert!(!fixture
        .packets()
        .iter()
        .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
    fixture.command(Command::Pressure);
    assert!(!fixture
        .packets()
        .iter()
        .any(|p| matches!(p.kind, Kind::Button { pressed: true, .. })));
    fixture.command(Command::Removed);
    assert!(fixture
        .packets()
        .iter()
        .any(|p| matches!(p.kind, Kind::Cancel)));
    let fixture = Fixture::new(false);
    assert!(fixture
        .reader
        .as_ref()
        .unwrap()
        .status()
        .contains("does not advertise"));
}

#[test]
fn real_wayland_input_uses_zoom_only_and_no_x11_polling_delay() {
    let mut fixture = Fixture::new(true);
    let ctx = eframe::egui::Context::default();
    ctx.set_zoom_factor(1.5);
    let _ = ctx.run(Default::default(), |_| {});
    let mut input = super::super::PenInput::default();
    input.wayland = fixture.reader.take();
    fixture.command(Command::Tap(false));
    fixture.host.roundtrip().unwrap();
    let mut raw = eframe::egui::RawInput::default();
    raw.viewports
        .get_mut(&raw.viewport_id)
        .unwrap()
        .native_pixels_per_point = Some(3.0);
    input.raw_input(&ctx, &mut raw);
    let touches: Vec<_> = raw
        .events
        .iter()
        .filter_map(|event| match event {
            eframe::egui::Event::Touch { phase, pos, .. } => Some((*phase, *pos)),
            _ => None,
        })
        .collect();
    assert_eq!(touches.len(), 3);
    assert_eq!(touches[0].0, eframe::egui::TouchPhase::Start);
    assert_eq!(touches.last().unwrap().0, eframe::egui::TouchPhase::End);
    assert!((touches[0].1.x - 80.0 / 1.5).abs() < 0.01);
    assert!(input.pending.is_none());
    input.shutdown();
    fixture.host.roundtrip().unwrap();
}
