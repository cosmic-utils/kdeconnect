//! Background Wayland clipboard access.
//!
//! A panel applet cannot reliably use `wl_data_device`: reads and writes are
//! tied to keyboard focus and an input serial.  This worker uses the privileged
//! `ext-data-control-v1` protocol on a separate connection instead, so clipboard
//! synchronization is independent of popup focus and of the applet process.

use anyhow::{Context, Result, anyhow};
use calloop::{EventLoop, channel};
use calloop_wayland_source::WaylandSource;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Write},
    os::fd::{AsFd, BorrowedFd},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    backend::ObjectId,
    delegate_noop,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

const TEXT_MIME_TYPES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain;charset=UTF-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];
const MAX_CLIPBOARD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardContent {
    pub text: String,
    pub sensitive: bool,
}

#[derive(Debug)]
pub enum ClipboardEvent {
    /// A user/application changed the regular desktop clipboard.  Changes made
    /// through `ClipboardHandle::set_text` are intentionally not emitted.
    Changed(ClipboardContent),
}

#[derive(Clone, Debug)]
pub struct ClipboardHandle {
    commands: channel::Sender<Command>,
    current: Arc<RwLock<Option<ClipboardContent>>>,
}

impl ClipboardHandle {
    pub fn current(&self) -> Option<ClipboardContent> {
        self.current.read().ok().and_then(|value| value.clone())
    }

    pub fn set_text(&self, text: String) -> Result<()> {
        if text.is_empty() {
            return Err(anyhow!("refusing to replace the clipboard with empty text"));
        }
        self.commands
            .send(Command::SetText(text))
            .map_err(|_| anyhow!("clipboard worker has stopped"))
    }
}

#[derive(Debug)]
enum Command {
    SetText(String),
    SelectionRead {
        generation: u64,
        publish: bool,
        content: Option<ClipboardContent>,
    },
}

#[derive(Default)]
struct OfferState {
    mime_types: Vec<String>,
}

struct WorkerState {
    manager: Option<ExtDataControlManagerV1>,
    seats: HashMap<ObjectId, wl_seat::WlSeat>,
    devices: HashMap<ObjectId, ExtDataControlDeviceV1>,
    offers: HashMap<ObjectId, OfferState>,
    sources: HashMap<ObjectId, String>,
    selection_offer: Option<ExtDataControlOfferV1>,
    selection_generation: u64,
    initialized_devices: HashSet<ObjectId>,
    pending_remote_write: Option<String>,
    commands: channel::Sender<Command>,
    events: mpsc::UnboundedSender<ClipboardEvent>,
    current: Arc<RwLock<Option<ClipboardContent>>>,
}

impl WorkerState {
    fn ensure_devices(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = self.manager.as_ref() else {
            return;
        };

        for (seat_id, seat) in &self.seats {
            if !self.devices.contains_key(seat_id) {
                let device = manager.get_data_device(seat, qh, ());
                self.devices.insert(seat_id.clone(), device);
            }
        }
    }

    fn handle_command(&mut self, command: Command, qh: &QueueHandle<Self>) {
        match command {
            Command::SetText(text) => self.set_text(text, qh),
            Command::SelectionRead {
                generation,
                publish,
                content,
            } => self.finish_selection_read(generation, publish, content),
        }
    }

    fn set_text(&mut self, text: String, qh: &QueueHandle<Self>) {
        let Some(manager) = self.manager.as_ref() else {
            warn!("Cannot write clipboard: ext-data-control manager disappeared");
            return;
        };

        if self.devices.is_empty() {
            warn!("Cannot write clipboard: compositor did not advertise a seat");
            return;
        }

        for device in self.devices.values() {
            // A data-control source may only be used in one set_selection
            // request, so every seat gets its own source.
            let source = manager.create_data_source(qh, ());
            for mime_type in TEXT_MIME_TYPES {
                source.offer((*mime_type).to_string());
            }
            self.sources.insert(source.id(), text.clone());
            device.set_selection(Some(&source));
        }

        self.pending_remote_write = Some(text.clone());
        if let Ok(mut current) = self.current.write() {
            *current = Some(ClipboardContent {
                text,
                sensitive: false,
            });
        }
    }

    fn begin_selection_read(
        &mut self,
        offer: Option<ExtDataControlOfferV1>,
        publish: bool,
        connection: &Connection,
    ) {
        self.selection_generation = self.selection_generation.wrapping_add(1);
        let generation = self.selection_generation;

        if let Some(previous) = self.selection_offer.take() {
            self.offers.remove(&previous.id());
            previous.destroy();
        }

        let Some(offer) = offer else {
            self.finish_selection_read(generation, publish, None);
            return;
        };

        let offer_state = self.offers.get(&offer.id());
        let mime_type = TEXT_MIME_TYPES.iter().find_map(|preferred| {
            offer_state?
                .mime_types
                .iter()
                .find(|offered| offered.eq_ignore_ascii_case(preferred))
                .cloned()
        });
        let sensitive = offer_state.is_some_and(|state| {
            state.mime_types.iter().any(|mime| {
                mime.eq_ignore_ascii_case("application/x-kde-passwordmanagerhint")
                    || mime.eq_ignore_ascii_case("x-kde-passwordmanagerhint")
            })
        });
        self.selection_offer = Some(offer.clone());

        let Some(mime_type) = mime_type else {
            self.finish_selection_read(generation, publish, None);
            return;
        };

        let Ok((mut reader, writer)) = UnixStream::pair() else {
            warn!("Failed to create clipboard transfer socket");
            self.finish_selection_read(generation, publish, None);
            return;
        };

        offer.receive(mime_type, writer.as_fd());
        if let Err(error) = connection.flush() {
            warn!("Failed to start clipboard transfer: {error}");
            self.finish_selection_read(generation, publish, None);
            return;
        }
        drop(writer);

        let commands = self.commands.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let content = match Read::by_ref(&mut reader)
                .take(MAX_CLIPBOARD_BYTES + 1)
                .read_to_end(&mut bytes)
            {
                Ok(_) if bytes.len() as u64 > MAX_CLIPBOARD_BYTES => {
                    warn!("Clipboard selection exceeds the 16 MiB safety limit");
                    None
                }
                Ok(_) => match String::from_utf8(bytes) {
                    Ok(text) if !text.is_empty() => Some(ClipboardContent { text, sensitive }),
                    Ok(_) => None,
                    Err(error) => {
                        warn!("Clipboard selection is not valid UTF-8: {error}");
                        None
                    }
                },
                Err(error) => {
                    warn!("Failed to read clipboard selection: {error}");
                    None
                }
            };
            let _ = commands.send(Command::SelectionRead {
                generation,
                publish,
                content,
            });
        });
    }

    fn finish_selection_read(
        &mut self,
        generation: u64,
        publish: bool,
        content: Option<ClipboardContent>,
    ) {
        // A slow owner may finish after a newer clipboard selection was made.
        if generation != self.selection_generation {
            return;
        }

        let previous = self.current.read().ok().and_then(|value| value.clone());
        if let Ok(mut current) = self.current.write() {
            *current = content.clone();
        }

        let Some(content) = content else {
            self.pending_remote_write = None;
            return;
        };

        if self.pending_remote_write.as_deref() == Some(&content.text) {
            self.pending_remote_write = None;
            debug!("Suppressed clipboard echo after phone-to-desktop update");
            return;
        }
        self.pending_remote_write = None;

        // Multiple seats and clipboard owners replacing themselves can produce
        // duplicate selection events for identical text.
        if previous.as_ref() == Some(&content) {
            return;
        }

        // Do not send whatever happened to be in the clipboard before the
        // service started. Only later user/application changes are synchronized.
        if publish {
            let _ = self.events.send(ClipboardEvent::Changed(content));
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for WorkerState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == ExtDataControlManagerV1::interface().name => {
                state.manager = Some(registry.bind(name, version.min(1), qh, ()));
                state.ensure_devices(qh);
            }
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == wl_seat::WlSeat::interface().name => {
                let seat: wl_seat::WlSeat = registry.bind(name, version.min(9), qh, ());
                state.seats.insert(seat.id(), seat);
                state.ensure_devices(qh);
            }
            _ => {}
        }
    }
}

delegate_noop!(WorkerState: ignore wl_seat::WlSeat);
delegate_noop!(WorkerState: ignore ExtDataControlManagerV1);

impl Dispatch<ExtDataControlDeviceV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        device: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        connection: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id } => {
                // Every seat sends one initial selection snapshot. It seeds the
                // cache but must not be mistaken for a user clipboard change.
                let publish = !state.initialized_devices.insert(device.id());
                state.begin_selection_read(id, publish, connection);
            }
            ext_data_control_device_v1::Event::Finished => {
                warn!("Wayland compositor revoked clipboard data-control access");
            }
            // Primary selection is intentionally independent from Ctrl+C/Ctrl+V.
            ext_data_control_device_v1::Event::DataOffer { .. }
            | ext_data_control_device_v1::Event::PrimarySelection { .. } => {}
            _ => {}
        }
    }

    fn event_created_child(
        opcode: u16,
        qh: &QueueHandle<Self>,
    ) -> Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            0 => qh.make_data::<ExtDataControlOfferV1, _>(()),
            _ => unreachable!("unknown ext_data_control_device_v1 child opcode"),
        }
    }
}

impl Dispatch<ExtDataControlOfferV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state
                .offers
                .entry(offer.id())
                .or_default()
                .mime_types
                .push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for WorkerState {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { fd, .. } => {
                let Some(text) = state.sources.get(&source.id()).cloned() else {
                    return;
                };
                std::thread::spawn(move || {
                    let mut file = File::from(fd);
                    if let Err(error) = file.write_all(text.as_bytes()) {
                        warn!("Failed to serve clipboard data: {error}");
                    }
                });
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.sources.remove(&source.id());
                source.destroy();
            }
            _ => {}
        }
    }
}

/// Start the clipboard worker. Failure is non-fatal: the KDE Connect network
/// service remains usable and manual clipboard actions return a clear error.
pub fn start() -> Result<(ClipboardHandle, mpsc::UnboundedReceiver<ClipboardEvent>)> {
    let connection = connect_to_host_compositor()?;
    let mut event_queue = connection.new_event_queue::<WorkerState>();
    let qh = event_queue.handle();
    connection.display().get_registry(&qh, ());

    let (command_sender, command_channel) = channel::channel();
    let (event_sender, event_receiver) = mpsc::unbounded_channel();
    let current = Arc::new(RwLock::new(None));
    let mut state = WorkerState {
        manager: None,
        seats: HashMap::new(),
        devices: HashMap::new(),
        offers: HashMap::new(),
        sources: HashMap::new(),
        selection_offer: None,
        selection_generation: 0,
        initialized_devices: HashSet::new(),
        pending_remote_write: None,
        commands: command_sender.clone(),
        events: event_sender,
        current: current.clone(),
    };

    event_queue
        .roundtrip(&mut state)
        .context("failed to enumerate Wayland clipboard globals")?;
    event_queue
        .roundtrip(&mut state)
        .context("failed to initialize Wayland data-control device")?;

    if state.manager.is_none() {
        return Err(anyhow!(
            "the compositor does not advertise ext-data-control-v1"
        ));
    }
    if state.devices.is_empty() {
        return Err(anyhow!("the compositor did not advertise a Wayland seat"));
    }

    let handle = ClipboardHandle {
        commands: command_sender,
        current,
    };

    std::thread::Builder::new()
        .name("kdeconnect-clipboard".to_string())
        .spawn(move || {
            let mut event_loop: EventLoop<WorkerState> = match EventLoop::try_new() {
                Ok(event_loop) => event_loop,
                Err(error) => {
                    warn!("Failed to create clipboard event loop: {error}");
                    return;
                }
            };
            let loop_handle = event_loop.handle();
            if let Err(error) =
                WaylandSource::new(connection, event_queue).insert(loop_handle.clone())
            {
                warn!("Failed to register clipboard Wayland source: {error}");
                return;
            }
            if let Err(error) =
                loop_handle.insert_source(command_channel, |event, _, state| match event {
                    channel::Event::Msg(command) => state.handle_command(command, &qh),
                    channel::Event::Closed => {}
                })
            {
                warn!("Failed to register clipboard command source: {error}");
                return;
            }

            info!("Wayland ext-data-control clipboard worker started");
            if let Err(error) = event_loop.run(None, &mut state, |_| {}) {
                warn!("Clipboard worker stopped: {error}");
            }
        })
        .context("failed to spawn clipboard worker thread")?;

    Ok((handle, event_receiver))
}

fn connect_to_host_compositor() -> Result<Connection> {
    if let Ok(raw_fd) = std::env::var("X_PRIVILEGED_WAYLAND_SOCKET") {
        if let Ok(raw_fd) = raw_fd.parse::<i32>() {
            // The panel owns the inherited descriptor. Duplicate it so this
            // connection can close independently from the applet/panel.
            let borrowed = unsafe { BorrowedFd::borrow_raw(raw_fd) };
            if let Ok(owned) = borrowed.try_clone_to_owned() {
                let stream = UnixStream::from(owned);
                return Connection::from_socket(stream)
                    .context("failed to connect through X_PRIVILEGED_WAYLAND_SOCKET");
            }
        }
        warn!("Ignoring invalid X_PRIVILEGED_WAYLAND_SOCKET");
    }

    let display = std::env::var_os("WAYLAND_DISPLAY")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("WAYLAND_DISPLAY is not set"))?;
    let socket_path = if display.is_absolute() {
        display
    } else {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
        runtime_dir.join(display)
    };
    let stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    Connection::from_socket(stream).context("failed to initialize Wayland connection")
}

#[derive(Clone, Copy, Debug)]
pub struct ClipboardPluginConfig {
    pub auto_share: bool,
    pub send_password: bool,
}

impl Default for ClipboardPluginConfig {
    fn default() -> Self {
        Self {
            auto_share: true,
            send_password: false,
        }
    }
}

/// Read the same per-device KDE config that the settings UI writes. Keeping
/// this parser in the service makes auto-sync independent of applet lifetime.
pub async fn load_plugin_config(device_id: &str) -> ClipboardPluginConfig {
    let path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("kdeconnect")
        .join(device_id)
        .join("kdeconnect_clipboard")
        .join("config");
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return ClipboardPluginConfig::default();
    };

    let mut config = ClipboardPluginConfig::default();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "autoShare" => config.auto_share = value.trim().parse().unwrap_or(true),
            "sendPassword" => config.send_password = value.trim().parse().unwrap_or(false),
            _ => {}
        }
    }
    config
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "requires a running Wayland compositor with ext-data-control-v1"]
    fn data_control_smoke() {
        let (_handle, _events) =
            super::start().expect("the current Wayland session must provide ext-data-control-v1");
    }
}
