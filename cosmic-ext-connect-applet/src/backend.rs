//! Backend interface — varlink-first, D-Bus fallback.

use anyhow::Result;
use cosmic::iced::Subscription;
use futures::StreamExt;
use kdeconnect_dbus_client::{KdeConnectClient, ServiceEvent};
use std::sync::Arc;
use std::{any::TypeId, collections::HashMap};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::models::{Device, NowPlaying};

lazy_static::lazy_static! {
    static ref CLIENT: Arc<Mutex<Option<Arc<KdeConnectClient>>>> =
        Arc::new(Mutex::new(None));
    static ref DEVICE_CACHE: Arc<Mutex<HashMap<String, Device>>> =
        Arc::new(Mutex::new(HashMap::new()));
    static ref VARLINK_ADDR: Arc<Mutex<Option<String>>> =
        Arc::new(Mutex::new(None));
}

pub async fn initialize() -> Result<()> {
    let addr = kdeconnect_varlink::socket_address();

    match varlink::AsyncConnection::with_address(&addr).await {
        Ok(_probe) => {
            info!("Varlink socket reachable at {}", addr);
            *VARLINK_ADDR.lock().await = Some(addr);
            match KdeConnectClient::new().await {
                Ok(client) => {
                    *CLIENT.lock().await = Some(Arc::new(client));
                    info!("D-Bus client also connected (MPRIS2/signals)");
                }
                Err(e) => {
                    warn!("D-Bus unavailable, varlink-only mode: {:?}", e);
                }
            }
        }
        Err(e) => {
            warn!("Varlink not available ({}), D-Bus only", e);
            let client = KdeConnectClient::new().await?;
            *CLIENT.lock().await = Some(Arc::new(client));
            info!("D-Bus client connected to kdeconnect-service");
        }
    }

    Ok(())
}

async fn via_varlink<F, Fut, T>(f: F) -> Option<Result<T>>
where
    F: FnOnce(kdeconnect_varlink::iface::VarlinkClient) -> Fut,
    Fut: std::future::Future<Output = Result<T, kdeconnect_varlink::Error>>,
{
    let addr = VARLINK_ADDR.lock().await.clone()?;
    match varlink::AsyncConnection::with_address(&addr).await {
        Ok(conn) => Some(
            f(kdeconnect_varlink::iface::VarlinkClient::new(conn))
                .await
                .map_err(|e| anyhow::anyhow!("varlink: {:?}", e)),
        ),
        Err(e) => {
            warn!("Varlink reconnect failed: {:?}", e);
            None
        }
    }
}

macro_rules! dbus_client {
    ($guard:ident) => {
        match $guard.as_ref() {
            Some(c) => c,
            None => return Err(anyhow::anyhow!("D-Bus client not initialized")),
        }
    };
}

/// Builds a `Device`, preserving fields the phone doesn't report on every poll
/// (battery, signal, transfer progress, run commands) from the existing cache
/// entry, and updates the cache with the result.
fn merge_device(
    id: String,
    name: String,
    device_type: String,
    is_paired: bool,
    is_reachable: bool,
    mounted: &[String],
    cache: &mut HashMap<String, Device>,
) -> Device {
    let existing = cache.get(&id).cloned();
    let device = Device {
        id: id.clone(),
        name,
        device_type,
        is_paired,
        is_reachable,
        battery_level: existing.as_ref().and_then(|e| e.battery_level),
        is_charging: existing.as_ref().and_then(|e| e.is_charging),
        network_type: existing.as_ref().and_then(|e| e.network_type.clone()),
        signal_strength: existing.as_ref().and_then(|e| e.signal_strength),
        pairing_requests: 0,
        has_battery: false,
        has_ping: true,
        has_sms: true,
        has_contacts: false,
        has_clipboard: true,
        has_findmyphone: true,
        has_share: true,
        share_progress: existing.as_ref().and_then(|e| e.share_progress),
        has_sftp: true,
        is_mounted: mounted.iter().any(|m| m == &id),
        has_mpris: false,
        has_remote_keyboard: false,
        has_presenter: false,
        has_lockdevice: false,
        has_virtualmonitor: false,
        run_commands: existing
            .as_ref()
            .map(|e| e.run_commands.clone())
            .unwrap_or_default(),
    };
    cache.insert(id, device.clone());
    device
}

pub async fn fetch_devices() -> Vec<Device> {
    let mounted = mounted_devices().await;

    if let Some(Ok(reply)) = via_varlink(|c| async move {
        use kdeconnect_varlink::iface::VarlinkClientInterface;
        c.list_devices().call().await
    })
    .await
    {
        let mut cache = DEVICE_CACHE.lock().await;
        let devices: Vec<Device> = reply
            .devices
            .into_iter()
            .map(|d| {
                merge_device(
                    d.id,
                    d.name,
                    d.device_type,
                    d.is_paired,
                    d.is_reachable,
                    &mounted,
                    &mut cache,
                )
            })
            .collect();
        return devices;
    }

    let client_guard = CLIENT.lock().await;
    let Some(client) = client_guard.as_ref() else {
        warn!("D-Bus client not initialized");
        return vec![];
    };

    match client.list_devices().await {
        Ok(dbus_devices) => {
            let mut cache = DEVICE_CACHE.lock().await;
            dbus_devices
                .into_iter()
                .map(|d| {
                    merge_device(
                        d.id,
                        d.name,
                        "phone".to_string(),
                        d.is_paired,
                        d.is_reachable,
                        &mounted,
                        &mut cache,
                    )
                })
                .collect()
        }
        Err(e) => {
            error!("Failed to fetch devices: {:?}", e);
            vec![]
        }
    }
}

pub async fn update_device(device_id: String, device: Device) {
    DEVICE_CACHE.lock().await.insert(device_id, device);
}

pub async fn pair_device(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.pair_device(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).pair_device(&device_id).await
}

pub async fn unpair_device(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.unpair_device(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).unpair_device(&device_id).await
}

pub async fn ping_device(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.send_ping(id, "Ping from COSMIC!".into())
                .call()
                .await
                .map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g)
        .send_ping(&device_id, "Ping from COSMIC!")
        .await
}

pub async fn send_files(device_id: String, files: Vec<String>) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        let f = files.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.send_files(id, f).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).send_files(&device_id, files).await
}

/// Ask the service to read its background clipboard cache and send it.
pub async fn share_clipboard(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.share_clipboard(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).share_clipboard(&device_id).await
}

pub async fn browse_device_filesystem(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.browse_device(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).browse_device(&device_id).await
}

/// Unmount a device's SFTP share (the counterpart of browse).
pub async fn unmount_device(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.unmount_device(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).unmount_device(&device_id).await
}

/// Device IDs whose SFTP share is currently mounted. Errors degrade to
/// "nothing mounted" — the applet then just shows the Browse button.
pub async fn mounted_devices() -> Vec<String> {
    if let Some(Ok(reply)) = via_varlink(|c| async move {
        use kdeconnect_varlink::iface::VarlinkClientInterface;
        c.mounted_devices().call().await
    })
    .await
    {
        return reply.device_ids;
    }
    let g = CLIENT.lock().await;
    match g.as_ref() {
        Some(client) => client.mounted_devices().await.unwrap_or_default(),
        None => vec![],
    }
}

pub async fn accept_pairing(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.accept_pairing(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).accept_pairing(&device_id).await
}

pub async fn reject_pairing(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.reject_pairing(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).reject_pairing(&device_id).await
}

pub async fn ring_device(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.ring_device(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).ring_device(&device_id).await
}

pub async fn set_plugin_enabled(device_id: String, plugin_id: String, enabled: bool) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        let plug = plugin_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.set_plugin_enabled(id, plug, enabled)
                .call()
                .await
                .map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g)
        .set_plugin_enabled(&device_id, &plugin_id, enabled)
        .await
}

pub async fn get_disabled_plugins(device_id: String) -> Vec<String> {
    if let Some(Ok(reply)) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.get_disabled_plugins(id).call().await
        }
    })
    .await
    {
        return reply.plugins;
    }

    let g = CLIENT.lock().await;
    let Some(client) = g.as_ref() else {
        warn!("D-Bus client not initialized");
        return vec![];
    };
    match client.get_disabled_plugins(&device_id).await {
        Ok(disabled) => disabled,
        Err(e) => {
            warn!("Failed to get disabled plugins for {}: {:?}", device_id, e);
            vec![]
        }
    }
}

/// Checks whether a device has any unread SMS conversation, for the
/// quick-actions menu's indicator next to "SMS Messages". Reuses the same
/// shared comparison kdeconnect_core::plugins::sms::has_unread as the SMS
/// window itself, against the same on-disk last-seen state — see
/// sms_read_state.rs for why this has to be on-disk at all (the SMS
/// window is a separate process with no other way to share this here).
pub async fn has_unread_sms(device_id: String) -> bool {
    let json = if let Some(Ok(reply)) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.get_cached_sms(id).call().await
        }
    })
    .await
    {
        Some(reply.json)
    } else {
        let g = CLIENT.lock().await;
        match g.as_ref() {
            Some(client) => client.get_cached_sms(&device_id).await.ok(),
            None => None,
        }
    };

    let Some(json) = json.filter(|j| !j.is_empty()) else {
        return false;
    };

    let last_seen = kdeconnect_core::sms_read_state::load_last_seen(&device_id);
    kdeconnect_core::plugins::sms::has_unread(&json, &last_seen)
}

/// Checks unread SMS state for several devices at once, e.g. every device
/// currently shown in the panel popup.
pub async fn check_unread_sms(device_ids: Vec<String>) -> HashMap<String, bool> {
    let mut result = HashMap::new();
    for id in device_ids {
        let unread = has_unread_sms(id.clone()).await;
        result.insert(id, unread);
    }
    result
}

pub async fn broadcast_identity() -> Result<()> {
    if let Some(r) = via_varlink(|c| async move {
        use kdeconnect_varlink::iface::VarlinkClientInterface;
        c.broadcast_identity().call().await.map(|_| ())
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).broadcast_identity().await
}

pub async fn request_run_commands(device_id: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.request_run_commands(id).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).request_run_commands(&device_id).await
}

pub async fn execute_run_command(device_id: String, key: String) -> Result<()> {
    if let Some(r) = via_varlink(|c| {
        let id = device_id.clone();
        let k = key.clone();
        async move {
            use kdeconnect_varlink::iface::VarlinkClientInterface;
            c.run_command(id, k).call().await.map(|_| ())
        }
    })
    .await
    {
        return r;
    }
    let g = CLIENT.lock().await;
    dbus_client!(g).run_command(&device_id, &key).await
}

/// Notify the running service to re-send the local command list to a connected
/// device. Call this immediately after adding or removing a local command so
/// the phone reflects the change without waiting for reconnect.
pub async fn push_local_commands(device_id: String) {
    if let Some(client) = CLIENT.lock().await.clone() {
        let _ = client.push_local_commands(&device_id).await;
    }
}

/// Stream of service events. Reconnects automatically when the client is
/// replaced (e.g. after session logout/login) or the stream ends.
///
/// NOTE: this stays D-Bus-only. varlink's `Subscribe()` looked like a fit,
/// but this version of the varlink/varlink_generator crates doesn't actually
/// support streaming replies in async mode (`set_continues` is a no-op,
/// `reply_struct` only ever keeps the latest reply in memory) — a handler
/// that loops and replies repeatedly never returns, so nothing is ever
/// written to the socket and the client's first `recv()` blocks forever.
pub async fn event_stream() -> futures::stream::BoxStream<'static, ServiceEvent> {
    use tokio::sync::mpsc;
    use tokio::time::{Duration, sleep};

    let (tx, rx) = mpsc::channel::<ServiceEvent>(100);

    tokio::spawn(async move {
        'reconnect: loop {
            let client = 'wait: loop {
                if let Some(client) = CLIENT.lock().await.clone() {
                    break 'wait client;
                }
                sleep(Duration::from_millis(100)).await;
            };

            info!("Event stream: D-Bus client ready, subscribing");
            let mut stream = match client.listen_for_events().await {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to subscribe to event stream: {:?}", e);
                    sleep(Duration::from_secs(1)).await;
                    continue 'reconnect;
                }
            };

            loop {
                tokio::select! {
                    event = stream.next() => {
                        match event {
                            Some(e) => { if tx.send(e).await.is_err() { return; } }
                            None => {
                                warn!("Event stream ended, reconnecting in 1s");
                                sleep(Duration::from_secs(1)).await;
                                continue 'reconnect;
                            }
                        }
                    }
                    _ = async {
                        loop {
                            sleep(Duration::from_millis(500)).await;
                            if let Some(current) = CLIENT.lock().await.clone() {
                                if !Arc::ptr_eq(&current, &client) { return; }
                            }
                        }
                    } => {
                        info!("D-Bus client replaced, reconnecting event stream");
                        continue 'reconnect;
                    }
                }
            }
        }
    });

    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

pub fn service_watcher_subscription() -> Subscription<crate::messages::Message> {
    struct ServiceWatcher;

    Subscription::run_with(TypeId::of::<ServiceWatcher>(), |_| {
        async_stream::stream! {
            use zbus::{MatchRule, MessageStream};
            use futures::StreamExt;

            let Some(connection) = session_bus().await else { return; };

            let rule = MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .interface("org.freedesktop.DBus").unwrap()
                .member("NameOwnerChanged").unwrap()
                .arg(0, "io.github.hepp3n.kdeconnect").unwrap()
                .build();

            let Ok(mut stream): Result<zbus::MessageStream, _> =
                MessageStream::for_match_rule(rule, connection, None).await else { return; };

            while let Some(Ok(msg)) = stream.next().await {
                let msg: zbus::Message = msg;
                if let Ok((_name, _old, new_owner)) =
                    msg.body().deserialize::<(String, String, String)>()
                {
                    if !new_owner.is_empty() {
                        info!("kdeconnect service reappeared on bus — reinitializing");
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        if let Err(e) = initialize().await {
                            error!("Failed to reinitialize: {:?}", e);
                            continue;
                        }
                        broadcast_identity().await.ok();
                        let mut elapsed = 0u64;
                        loop {
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            elapsed += 3;
                            let devices = fetch_devices().await;
                            if !devices.is_empty() {
                                info!("Device found after {}s", elapsed);
                                yield crate::messages::Message::RefreshDevices;
                                break;
                            }
                            if elapsed >= 90 {
                                warn!("Gave up waiting for devices after 90s");
                                break;
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn filetransfer_subscription() -> Subscription<crate::messages::Message> {
    struct Worker;

    Subscription::run_with(TypeId::of::<Worker>(), |_| {
        async_stream::stream! {
            let Ok(client) = KdeConnectClient::new().await else { return; };
            let mut progress_stream = client.transfer_progress_stream().await;
            while let Some(progress) = progress_stream.next().await {
                yield crate::messages::Message::UpdateTransferProgress(progress);
            }
        }
    })
}

// ============================================================================
// Media section — generic MPRIS client of org.mpris.MediaPlayer2.KDEConnect_*
// ============================================================================
//
// kdeconnect-core already registers each phone player as a standard
// org.mpris.MediaPlayer2 D-Bus service (so COSMIC's own media widget can
// control it) — see `register_phone_player` in kdeconnect-core. Reusing that
// existing, working interface means the applet needs zero new IPC of its
// own: just be another MPRIS client, same as the system widget.

const MPRIS_BUS_PREFIX: &str = "org.mpris.MediaPlayer2.KDEConnect_";

/// Shared session-bus connection. Creating a `zbus::Connection` performs a
/// socket connect + auth + Hello() roundtrip; doing that on every MPRIS poll
/// tick and every media-button press added user-visible latency and constant
/// churn. zbus multiplexes concurrent calls on one connection, so a single
/// shared one is all we need.
async fn session_bus() -> Option<&'static zbus::Connection> {
    static BUS: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
    BUS.get_or_try_init(zbus::Connection::session)
        .await
        .map_err(|e| warn!("[mpris] failed to connect to session bus: {:?}", e))
        .ok()
}

#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MprisRoot {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MprisPlayer {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn can_play(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_pause(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_go_next(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_go_previous(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, zbus::zvariant::OwnedValue>>;
}

async fn read_now_playing(connection: &zbus::Connection, bus_name: &str) -> Option<NowPlaying> {
    // These proxies live for one poll: skip zbus's property cache, which
    // would issue a GetAll and subscribe to PropertiesChanged on every build.
    let root = MprisRootProxy::builder(connection)
        .destination(bus_name)
        .ok()?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .ok()?;
    let identity = root
        .identity()
        .await
        .unwrap_or_else(|_| bus_name.to_string());

    let player = MprisPlayerProxy::builder(connection)
        .destination(bus_name)
        .ok()?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .ok()?;

    let is_playing = player
        .playback_status()
        .await
        .map(|s| s == "Playing")
        .unwrap_or(false);
    let can_play = player.can_play().await.unwrap_or(true);
    let can_pause = player.can_pause().await.unwrap_or(true);
    let can_go_next = player.can_go_next().await.unwrap_or(true);
    let can_go_previous = player.can_go_previous().await.unwrap_or(true);
    let metadata = player.metadata().await.unwrap_or_default();

    let title = metadata
        .get("xesam:title")
        .and_then(|v| String::try_from(v.clone()).ok());
    let artist = metadata
        .get("xesam:artist")
        .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
        .and_then(|mut names| names.pop());
    let art_path = metadata
        .get("mpris:artUrl")
        .and_then(|v| String::try_from(v.clone()).ok())
        .map(|uri| {
            uri.strip_prefix("file://")
                .map(str::to_string)
                .unwrap_or(uri)
        });

    Some(NowPlaying {
        identity,
        title,
        artist,
        is_playing,
        can_play,
        can_pause,
        can_go_next,
        can_go_previous,
        art_path,
    })
}

/// Polls every 2s for active KDE Connect MPRIS players and their state.
/// Polling (rather than chasing PropertiesChanged across a dynamic set of
/// bus names) keeps this simple and self-healing if a player disappears.
pub fn mpris_subscription() -> Subscription<crate::messages::Message> {
    struct MprisWatcher;

    Subscription::run_with(TypeId::of::<MprisWatcher>(), |_| {
        async_stream::stream! {
            loop {
                let snapshot = poll_now_playing().await;
                yield crate::messages::Message::MprisSnapshot(snapshot);
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    })
}

async fn poll_now_playing() -> HashMap<String, NowPlaying> {
    let mut snapshot = HashMap::new();

    let Some(connection) = session_bus().await else {
        return snapshot;
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return snapshot;
    };
    let Ok(names) = dbus.list_names().await else {
        return snapshot;
    };

    for name in names {
        let name = name.to_string();
        if !name.starts_with(MPRIS_BUS_PREFIX) {
            continue;
        }
        if let Some(now_playing) = read_now_playing(connection, &name).await {
            snapshot.insert(name, now_playing);
        }
    }

    snapshot
}

/// Sends a transport control to one phone player by calling the standard
/// MPRIS method directly on its D-Bus service.
pub async fn mpris_control(bus_name: String, action: MprisControlAction) {
    let Some(connection) = session_bus().await else {
        warn!("[mpris] no session bus connection for control action");
        return;
    };
    let player = MprisPlayerProxy::builder(connection)
        .destination(bus_name.as_str())
        .ok()
        .map(|b| b.cache_properties(zbus::proxy::CacheProperties::No).build());
    let Some(player) = player else {
        warn!("[mpris] failed to build player proxy for {}", bus_name);
        return;
    };
    let Ok(player) = player.await else {
        warn!("[mpris] failed to connect player proxy for {}", bus_name);
        return;
    };

    let result = match action {
        MprisControlAction::PlayPause => player.play_pause().await,
        MprisControlAction::Next => player.next().await,
        MprisControlAction::Previous => player.previous().await,
    };
    if let Err(e) = result {
        warn!("[mpris] control action failed for {}: {:?}", bus_name, e);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MprisControlAction {
    PlayPause,
    Next,
    Previous,
}
