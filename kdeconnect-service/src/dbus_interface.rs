//! D-Bus interface implementation for KDE Connect service

use anyhow::Result;
use kdeconnect_core::{
    KdeConnectCore, PacketType, ProtocolPacket,
    device::{DeviceId, DeviceState, PairState},
    event::{AppEvent, ConnectionEvent},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, error, info, warn};
use zbus::object_server::SignalEmitter;
use zbus::{Connection, interface};

use crate::clipboard::{self, ClipboardEvent, ClipboardHandle};

const SERVICE_NAME: &str = "io.github.hepp3n.kdeconnect";
const DAEMON_PATH: &str = "/io/github/hepp3n/kdeconnect/Daemon";
const SMS_PATH: &str = "/io/github/hepp3n/kdeconnect/Sms";
const CONTACTS_PATH: &str = "/io/github/hepp3n/kdeconnect/Contacts";

/// Simplified device info for D-Bus
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    zbus::zvariant::Type,
    zbus::zvariant::Value,
    zbus::zvariant::OwnedValue,
)]
pub struct DbusDevice {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub is_paired: bool,
    pub is_reachable: bool,
}

// --- Per-device cache helpers ------------------------------------------------

fn device_cache_dir(device_id: &str) -> std::path::PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("~/.local/share"));
    base.join(kdeconnect_core::config::CONFIG_DIR)
        .join(device_id)
}

async fn save_contacts_cache(device_id: &str, contacts: &HashMap<String, String>) {
    let path = device_cache_dir(device_id).join("contacts_cache.json");
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match serde_json::to_string(contacts) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                error!("Failed to save contacts cache: {}", e);
            } else {
                debug!(
                    "Contacts cache saved ({} entries) for {}",
                    contacts.len(),
                    device_id
                );
            }
        }
        Err(e) => error!("Failed to serialize contacts for cache: {}", e),
    }
}

pub(crate) async fn load_contacts_cache(device_id: &str) -> Option<HashMap<String, String>> {
    let path = device_cache_dir(device_id).join("contacts_cache.json");
    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(map) => {
                let map: HashMap<String, String> = map;
                debug!(
                    "Loaded contacts cache ({} entries) for {}",
                    map.len(),
                    device_id
                );
                Some(map)
            }
            Err(e) => {
                error!("Failed to parse contacts cache: {}", e);
                None
            }
        },
        Err(_) => None,
    }
}

/// Phone -> base64-encoded photo. Same shape and on-disk convention as
/// the name cache above, just a different file and decoding deferred to
/// the UI (these stay base64 the whole way through, same as SMS
/// thumbnails — see `ConnectionEvent::ContactPhotosReceived`).
async fn save_contact_photos_cache(device_id: &str, photos: &HashMap<String, String>) {
    let path = device_cache_dir(device_id).join("contact_photos_cache.json");
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match serde_json::to_string(photos) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                error!("Failed to save contact photos cache: {}", e);
            } else {
                debug!(
                    "Contact photos cache saved ({} entries) for {}",
                    photos.len(),
                    device_id
                );
            }
        }
        Err(e) => error!("Failed to serialize contact photos for cache: {}", e),
    }
}

pub(crate) async fn load_contact_photos_cache(device_id: &str) -> Option<HashMap<String, String>> {
    let path = device_cache_dir(device_id).join("contact_photos_cache.json");
    match tokio::fs::read_to_string(&path).await {
        Ok(json) => match serde_json::from_str(&json) {
            Ok(map) => Some(map),
            Err(e) => {
                error!("Failed to parse contact photos cache: {}", e);
                None
            }
        },
        Err(_) => None,
    }
}

async fn save_sms_cache(device_id: &str, messages_json: &str) {
    let path = device_cache_dir(device_id).join("sms_cache.json");
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Err(e) = tokio::fs::write(&path, messages_json).await {
        error!("Failed to save SMS cache: {}", e);
    } else {
        debug!(
            "SMS cache saved ({} bytes) for {}",
            messages_json.len(),
            device_id
        );
    }
}

pub(crate) async fn load_sms_cache(device_id: &str) -> Option<String> {
    let path = device_cache_dir(device_id).join("sms_cache.json");
    match tokio::fs::read_to_string(&path).await {
        Ok(json) if !json.is_empty() => {
            debug!("Loaded SMS cache ({} bytes) for {}", json.len(), device_id);
            Some(json)
        }
        _ => None,
    }
}

// ----------------------------------------------------------------------------

/// Main daemon D-Bus interface
pub struct DaemonInterface {
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
    devices: Arc<Mutex<HashMap<String, DbusDevice>>>,
    clipboard: Option<ClipboardHandle>,
}

pub(crate) async fn send_clipboard_packet(
    event_sender: &mpsc::UnboundedSender<AppEvent>,
    devices: &Arc<Mutex<HashMap<String, DbusDevice>>>,
    device_id: String,
    content: String,
) -> std::result::Result<(), String> {
    let device = devices.lock().await.get(&device_id).cloned();
    let Some(device) = device else {
        return Err(format!("Unknown device: {device_id}"));
    };
    if !device.is_paired {
        return Err(format!("Device is not paired: {device_id}"));
    }
    if !device.is_reachable {
        return Err(format!("Device is not reachable: {device_id}"));
    }
    if kdeconnect_core::plugin_config::load_disabled_plugins(&device_id)
        .await
        .contains("clipboard")
    {
        return Err(format!(
            "Clipboard plugin is disabled for device: {device_id}"
        ));
    }

    let packet = ProtocolPacket::new(PacketType::Clipboard, json!({ "content": content }));
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    event_sender
        .send(AppEvent::SendPacketWithReply(
            DeviceId(device_id),
            packet,
            reply_tx,
        ))
        .map_err(|error| error.to_string())?;

    match tokio::time::timeout(std::time::Duration::from_secs(2), reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Clipboard send acknowledgement was dropped".to_string()),
        Err(_) => Err("Clipboard send acknowledgement timed out".to_string()),
    }
}

async fn send_clipboard_connect_when_ready(
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
    device_id: String,
    clipboard: Option<ClipboardHandle>,
) {
    let Some(clipboard) = clipboard else {
        debug!("Not sending clipboard.connect to {device_id}: clipboard backend unavailable");
        return;
    };
    if kdeconnect_core::plugin_config::load_disabled_plugins(&device_id)
        .await
        .contains("clipboard")
    {
        return;
    }
    let config = clipboard::load_plugin_config(&device_id).await;
    if !config.auto_share {
        return;
    }

    // The initial Wayland offer is read asynchronously. Give it a short time
    // to seed the cache when a device connects during service startup.
    let mut content = clipboard.current();
    for _ in 0..20 {
        if content.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        content = clipboard.current();
    }
    let Some(content) = content else {
        debug!("Not sending clipboard.connect to {device_id}: desktop clipboard is empty");
        return;
    };
    if content.sensitive && !config.send_password {
        debug!("Not sending clipboard.connect to {device_id}: clipboard is sensitive");
        return;
    }

    let packet = ProtocolPacket::new(
        PacketType::ClipboardConnect,
        json!({
            "content": content.text,
            "timestamp": content.timestamp,
        }),
    );
    match event_sender.send(AppEvent::SendPacket(DeviceId(device_id.clone()), packet)) {
        Ok(()) => debug!(
            "Queued clipboard.connect for {} with timestamp {}",
            device_id, content.timestamp
        ),
        Err(error) => error!("Failed to queue clipboard.connect for {device_id}: {error}"),
    }
}

#[interface(name = "io.github.hepp3n.kdeconnect.Daemon")]
impl DaemonInterface {
    /// List all known devices
    async fn list_devices(&self) -> Vec<DbusDevice> {
        info!("D-Bus: ListDevices called");
        let devices = self.devices.lock().await;
        let device_list: Vec<DbusDevice> = devices.values().cloned().collect();
        info!("D-Bus: Returning {} devices", device_list.len());
        device_list
    }

    /// Pair with a device
    async fn pair_device(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: PairDevice called for {}", device_id);
        self.event_sender
            .send(AppEvent::Pair(DeviceId(device_id)))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Unpair from a device
    async fn unpair_device(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: UnpairDevice called for {}", device_id);
        self.event_sender
            .send(AppEvent::Unpair(DeviceId(device_id)))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Send a ping to a device
    async fn send_ping(&self, device_id: String, message: String) -> zbus::fdo::Result<()> {
        info!(
            "D-Bus: SendPing called for {} with message: {}",
            device_id, message
        );
        let packet = ProtocolPacket::new(PacketType::Ping, json!({ "message": message }));
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Send files to a device
    async fn send_files(&self, device_id: String, files: Vec<String>) -> zbus::fdo::Result<()> {
        info!(
            "D-Bus: SendFiles called for {} ({} files)",
            device_id,
            files.len()
        );
        self.event_sender
            .send(AppEvent::SendFiles((DeviceId(device_id), files)))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Send clipboard content
    async fn send_clipboard(&self, device_id: String, content: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: SendClipboard called for {}", device_id);
        send_clipboard_packet(&self.event_sender, &self.devices, device_id, content)
            .await
            .map_err(zbus::fdo::Error::Failed)
    }

    /// Send the current desktop clipboard. Reading is performed by the
    /// service's background data-control worker, not by the focused applet.
    async fn share_clipboard(&self, device_id: String) -> zbus::fdo::Result<()> {
        let clipboard = self.clipboard.as_ref().ok_or_else(|| {
            zbus::fdo::Error::Failed(
                "Background clipboard access is unavailable; COSMIC must expose ext- or wlr-data-control-v1 (Flatpak builds require host WAYLAND_DISPLAY/XDG_RUNTIME_DIR access)"
                    .to_string(),
            )
        })?;
        let content = clipboard.current().ok_or_else(|| {
            zbus::fdo::Error::Failed("The current clipboard does not contain text".to_string())
        })?;
        self.send_clipboard(device_id, content.text).await
    }

    /// Ring a device (findmyphone)
    async fn ring_device(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: RingDevice called for {}", device_id);
        let packet = ProtocolPacket::new(PacketType::FindMyPhoneRequest, json!({}));
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Ask a device to start its SFTP server so we can browse its filesystem
    async fn browse_device(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: BrowseDevice called for {}", device_id);
        // Fast path: if the share is already mounted and responsive, just
        // (re)open the file manager — no phone round-trip, no remount.
        let device_name = self
            .devices
            .lock()
            .await
            .get(&device_id)
            .map(|d| d.name.clone());
        if let Some(name) = device_name.as_deref() {
            if kdeconnect_core::plugins::sftp::open_mounted(&device_id, name).await {
                debug!(
                    "BrowseDevice: {} already mounted, opened directly",
                    device_id
                );
                return Ok(());
            }
        }
        let packet = ProtocolPacket::new(PacketType::SftpRequest, json!({ "startBrowsing": true }));
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Unmount a device's SFTP share (the counterpart of BrowseDevice).
    async fn unmount_device(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        device_id: String,
    ) -> zbus::fdo::Result<()> {
        info!("D-Bus: UnmountDevice called for {}", device_id);
        let device_name = self
            .devices
            .lock()
            .await
            .get(&device_id)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        match kdeconnect_core::plugins::sftp::unmount(&device_id, &device_name).await {
            Ok(unmounted) => {
                if unmounted {
                    let _ = Self::mount_state_changed(&emitter, device_id, false).await;
                }
                Ok(())
            }
            Err(e) => Err(zbus::fdo::Error::Failed(e.to_string())),
        }
    }

    /// Device IDs whose SFTP share is currently mounted, straight from the
    /// host mount table (so a mount the user ejected from the file manager
    /// no longer counts).
    async fn mounted_devices(&self) -> zbus::fdo::Result<Vec<String>> {
        let pairs: Vec<(String, String)> = self
            .devices
            .lock()
            .await
            .iter()
            .map(|(id, d)| (id.clone(), d.name.clone()))
            .collect();
        Ok(kdeconnect_core::plugins::sftp::mounted_devices(&pairs).await)
    }

    /// Enable or disable a plugin for a device.
    /// Changes take effect immediately and are persisted across restarts.
    async fn set_plugin_enabled(
        &self,
        device_id: String,
        plugin_id: String,
        enabled: bool,
    ) -> zbus::fdo::Result<()> {
        info!(
            "D-Bus: SetPluginEnabled device={} plugin={} enabled={}",
            device_id, plugin_id, enabled
        );
        self.event_sender
            .send(AppEvent::SetPluginEnabled {
                device_id: DeviceId(device_id),
                plugin_id,
                enabled,
            })
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Return the list of disabled plugin IDs for a device.
    async fn get_disabled_plugins(&self, device_id: String) -> Vec<String> {
        kdeconnect_core::plugin_config::load_disabled_plugins(&device_id)
            .await
            .into_iter()
            .collect()
    }

    /// Trigger a UDP identity broadcast to discover nearby devices.
    async fn broadcast_identity(&self) -> zbus::fdo::Result<()> {
        info!("D-Bus: BroadcastIdentity called");
        self.event_sender
            .send(AppEvent::Broadcasting)
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Accept an incoming pairing request from a device.
    async fn accept_pairing(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: AcceptPairing called for {}", device_id);
        self.event_sender
            .send(AppEvent::AcceptPairing(kdeconnect_core::device::DeviceId(
                device_id,
            )))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Reject an incoming pairing request from a device.
    async fn reject_pairing(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: RejectPairing called for {}", device_id);
        self.event_sender
            .send(AppEvent::RejectPairing(kdeconnect_core::device::DeviceId(
                device_id,
            )))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Signal: A device is requesting to pair. Applet shows Accept/Decline UI.
    #[zbus(signal)]
    async fn pairing_requested(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        device_name: String,
    ) -> zbus::Result<()>;

    /// Signal: Device connected
    #[zbus(signal)]
    async fn update_transfer_progress(
        signal_emitter: &SignalEmitter<'_>,
        progress: u8,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        device: DbusDevice,
    ) -> zbus::Result<()>;

    /// Signal: Device paired
    #[zbus(signal)]
    async fn device_paired(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        device: DbusDevice,
    ) -> zbus::Result<()>;

    /// Signal: Device disconnected
    #[zbus(signal)]
    async fn device_disconnected(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
    ) -> zbus::Result<()>;

    /// Signal: Clipboard content received from a paired device
    #[zbus(signal)]
    async fn clipboard_received(
        signal_emitter: &SignalEmitter<'_>,
        content: String,
    ) -> zbus::Result<()>;

    /// Signal: Battery level/charging state received from a paired device
    #[zbus(signal)]
    async fn battery_received(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        level: i32,
        is_charging: bool,
    ) -> zbus::Result<()>;

    /// Signal: Cellular signal strength received from a paired device
    #[zbus(signal)]
    async fn connectivity_received(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        signal_strength: i32,
    ) -> zbus::Result<()>;

    /// Signal: Browse-device (SFTP mount) failed. `message` names the
    /// actual cause (missing sshfs, revoked Flatpak permission, etc.).
    #[zbus(signal)]
    async fn browse_failed(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        message: String,
    ) -> zbus::Result<()>;

    /// Signal: A device's SFTP share was mounted (true) or unmounted
    /// (false) — the latter fires both for explicit UnmountDevice calls and
    /// for the automatic unmount on device disconnect.
    #[zbus(signal)]
    async fn mount_state_changed(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        mounted: bool,
    ) -> zbus::Result<()>;

    /// Execute a remote command on a device by key
    async fn run_command(&self, device_id: String, key: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: RunCommand called for {} key={}", device_id, key);
        let packet = ProtocolPacket::new(PacketType::RunCommandRequest, json!({ "key": key }));
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Request the remote command list from a device
    async fn request_run_commands(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: RequestRunCommands called for {}", device_id);
        let packet = ProtocolPacket::new(
            PacketType::RunCommandRequest,
            json!({ "requestCommandList": true }),
        );
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Push our local command list to a connected device immediately.
    /// Call this after adding or removing a local command.
    async fn push_local_commands(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: PushLocalCommands called for {}", device_id);
        self.event_sender
            .send(AppEvent::PushLocalCommands(
                kdeconnect_core::device::DeviceId(device_id),
            ))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Signal: Remote command list received from a paired device.
    /// commands_json is a JSON array of {key, name, command} objects.
    #[zbus(signal)]
    async fn run_command_list_received(
        signal_emitter: &SignalEmitter<'_>,
        device_id: String,
        commands_json: String,
    ) -> zbus::Result<()>;
}

/// SMS-specific D-Bus interface
pub struct SmsInterface {
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
    sms_cache: Arc<Mutex<Option<String>>>,
}

#[interface(name = "io.github.hepp3n.kdeconnect.Sms")]
impl SmsInterface {
    /// Return cached SMS JSON — in-memory first, disk fallback, empty if neither
    async fn get_cached_sms(&self, device_id: String) -> String {
        if let Some(json) = self.sms_cache.lock().await.as_ref() {
            debug!("Returning in-memory SMS cache ({} bytes)", json.len());
            return json.clone();
        }
        match load_sms_cache(&device_id).await {
            Some(json) => json,
            None => String::new(),
        }
    }

    /// Request all conversations from device
    async fn request_conversations(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: RequestConversations called for {}", device_id);
        let packet = ProtocolPacket::new(PacketType::SmsRequestConversations, json!({}));
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| {
                error!("Failed to send packet: {}", e);
                zbus::fdo::Error::Failed(e.to_string())
            })?;
        debug!("RequestConversations sent to core");
        Ok(())
    }

    /// Request messages from a specific conversation
    async fn request_conversation(
        &self,
        device_id: String,
        thread_id: i64,
    ) -> zbus::fdo::Result<()> {
        info!(
            "D-Bus: RequestConversation called for {} thread {}",
            device_id, thread_id
        );
        let packet = ProtocolPacket::new(
            PacketType::SmsRequestConversation,
            json!({ "threadID": thread_id }),
        );
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| {
                error!("Failed to send packet: {}", e);
                zbus::fdo::Error::Failed(e.to_string())
            })?;
        debug!("RequestConversation sent to core");
        Ok(())
    }

    /// Send an SMS message
    async fn send_sms(
        &self,
        device_id: String,
        phone_number: String,
        message: String,
        attachments: Vec<String>,
    ) -> zbus::fdo::Result<()> {
        info!(
            "D-Bus: SendSms called for {} to {} ({} attachment(s))",
            device_id,
            phone_number,
            attachments.len()
        );
        let packet =
            kdeconnect_core::plugins::sms::build_send_packet(&phone_number, &message, &attachments)
                .await;
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| {
                error!("Failed to send SMS: {}", e);
                zbus::fdo::Error::Failed(e.to_string())
            })?;
        debug!("SMS send request sent to core");
        Ok(())
    }

    /// Request the full-resolution file for one MMS attachment. The
    /// result arrives asynchronously via `sms_attachment_received`.
    async fn request_sms_attachment(
        &self,
        device_id: String,
        part_id: i64,
        unique_identifier: String,
    ) -> zbus::fdo::Result<()> {
        info!(
            "D-Bus: RequestSmsAttachment called for {} part {}",
            device_id, part_id
        );
        let packet = ProtocolPacket::new(
            PacketType::SmsRequestAttachment,
            json!({ "part_id": part_id, "unique_identifier": unique_identifier }),
        );
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| {
                error!("Failed to send packet: {}", e);
                zbus::fdo::Error::Failed(e.to_string())
            })?;
        debug!("RequestSmsAttachment sent to core");
        Ok(())
    }

    /// Signal: SMS messages received
    #[zbus(signal)]
    async fn sms_messages_received(
        signal_emitter: &SignalEmitter<'_>,
        messages_json: String,
    ) -> zbus::Result<()>;

    /// Signal: a full-resolution MMS attachment finished downloading.
    /// `filename` is the same `unique_identifier` the request was made
    /// with — that's how the caller matches this back to a message.
    #[zbus(signal)]
    async fn sms_attachment_received(
        signal_emitter: &SignalEmitter<'_>,
        filename: String,
        path: String,
    ) -> zbus::Result<()>;
}

/// Contacts D-Bus interface
pub struct ContactsInterface {
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
}

#[interface(name = "io.github.hepp3n.kdeconnect.Contacts")]
impl ContactsInterface {
    /// Manually trigger a contacts sync from a device
    async fn request_contacts(&self, device_id: String) -> zbus::fdo::Result<()> {
        info!("D-Bus: RequestContacts called for {}", device_id);
        let packet = ProtocolPacket::new(PacketType::ContactsRequestAllUidsTimestamps, json!({}));
        self.event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(())
    }

    /// Return cached contacts from disk — no phone required
    async fn get_cached_contacts(&self, device_id: String) -> String {
        match load_contacts_cache(&device_id).await {
            Some(contacts) => serde_json::to_string(&contacts).unwrap_or_else(|_| "{}".to_string()),
            None => "{}".to_string(),
        }
    }

    /// Return cached contact photos from disk (phone -> base64) — no
    /// phone required
    async fn get_cached_contact_photos(&self, device_id: String) -> String {
        match load_contact_photos_cache(&device_id).await {
            Some(photos) => serde_json::to_string(&photos).unwrap_or_else(|_| "{}".to_string()),
            None => "{}".to_string(),
        }
    }

    /// Signal: contacts received — JSON object mapping phone → name
    #[zbus(signal)]
    async fn contacts_received(
        signal_emitter: &SignalEmitter<'_>,
        contacts_json: String,
    ) -> zbus::Result<()>;

    /// Signal: contact photos received — JSON object mapping phone →
    /// base64-encoded photo
    #[zbus(signal)]
    async fn contact_photos_received(
        signal_emitter: &SignalEmitter<'_>,
        photos_json: String,
    ) -> zbus::Result<()>;
}

/// Main service coordinator
pub struct KdeConnectService {
    #[allow(dead_code)]
    connection: Connection,
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
    devices: Arc<Mutex<HashMap<String, DbusDevice>>>,
    sms_cache: Arc<Mutex<Option<String>>>,
    clipboard: Option<ClipboardHandle>,
}

impl KdeConnectService {
    /// Block until the session ends or a signal is received. All work runs in
    /// spawned tasks started by `new()`; this just keeps the process alive and
    /// ensures a clean exit so the phone sees the disconnect promptly.
    ///
    /// Two exit paths are monitored:
    /// - SIGTERM / SIGINT: covers systemd-managed local installs.
    /// - Session D-Bus closed: covers cosmic-session logout, where the session
    ///   daemon closes all connections without necessarily delivering SIGTERM to
    ///   processes it did not directly register.
    pub fn start_varlink(
        &self,
        broadcast_tx: broadcast::Sender<crate::varlink_server::VarlinkEvent>,
    ) {
        let event_sender = self.event_sender.clone();
        let devices = self.devices.clone();
        let sms_cache = self.sms_cache.clone();
        let clipboard = self.clipboard.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::varlink_server::run_varlink_server(
                event_sender,
                devices,
                sms_cache,
                clipboard,
                broadcast_tx,
            )
            .await
            {
                warn!("Varlink server exited: {:?}", e);
            }
        });
    }

    pub async fn run(&self) -> Result<()> {
        use futures::StreamExt;
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = signal(SignalKind::terminate())?;

        // Dedicated watch connection — no messages are expected on it.
        // When the session bus closes (cosmic-session logout or systemd user
        // session teardown), the MessageStream ends and this future completes.
        let session_ended = async {
            let Ok(watch_conn) = zbus::Connection::session().await else {
                std::future::pending::<()>().await;
                return;
            };
            let mut stream = zbus::MessageStream::from(&watch_conn);
            while stream.next().await.is_some() {}
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => { info!("SIGINT received, shutting down"); }
            _ = sigterm.recv() => { info!("SIGTERM received, shutting down"); }
            _ = session_ended => { info!("Session D-Bus closed, shutting down"); }
        }

        // Retire any SFTP mounts we created: once this process (and with it
        // the phone link) is gone they would only go stale.
        kdeconnect_core::plugins::sftp::unmount_all().await;
        Ok(())
    }
}
impl KdeConnectService {
    pub async fn new(
        broadcast_tx: broadcast::Sender<crate::varlink_server::VarlinkEvent>,
    ) -> Result<Self> {
        info!("Initializing KDE Connect D-Bus service");

        let connection = Connection::session().await?;
        info!("D-Bus session connection established");

        connection.request_name(SERVICE_NAME).await?;
        info!("D-Bus service name '{}' registered", SERVICE_NAME);

        info!("Initializing kdeconnect-core");
        let (mut core, mut event_receiver) = KdeConnectCore::new().await?;
        let event_sender = core.take_events();
        info!("kdeconnect-core initialized");

        let devices = Arc::new(Mutex::new(HashMap::new()));

        let (clipboard, clipboard_events) = match clipboard::start() {
            Ok((handle, events)) => (Some(handle), Some(events)),
            Err(error) => {
                error!("Background clipboard synchronization unavailable: {error:#}");
                (None, None)
            }
        };

        // Pre-populate known paired devices as offline so list_devices() returns
        // them immediately after reboot, before the phone actively reconnects.
        {
            use kdeconnect_core::{config::CONFIG_DIR, device::Device as CoreDevice};
            let mut map = devices.lock().await;
            if let Some(config_dir) = dirs::config_dir() {
                let kc_dir = config_dir.join(CONFIG_DIR);
                if let Ok(mut entries) = tokio::fs::read_dir(&kc_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("ron") {
                            if let Ok(raw) = tokio::fs::read_to_string(&path).await {
                                if let Ok(dev) = ron::de::from_str::<CoreDevice>(&raw) {
                                    if dev.pair_state == PairState::Paired {
                                        info!("Restoring offline paired device: {}", dev.name);
                                        map.insert(
                                            dev.device_id.0.clone(),
                                            DbusDevice {
                                                id: dev.device_id.0.clone(),
                                                name: dev.name.clone(),
                                                device_type: "phone".to_string(),
                                                is_paired: true,
                                                is_reachable: false,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let daemon_interface = DaemonInterface {
            event_sender: event_sender.clone(),
            devices: devices.clone(),
            clipboard: clipboard.clone(),
        };
        connection
            .object_server()
            .at(DAEMON_PATH, daemon_interface)
            .await?;
        info!("Daemon interface registered at {}", DAEMON_PATH);

        let sms_cache: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let current_device_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let sms_interface = SmsInterface {
            event_sender: event_sender.clone(),
            sms_cache: sms_cache.clone(),
        };
        connection
            .object_server()
            .at(SMS_PATH, sms_interface)
            .await?;
        info!("SMS interface registered at {}", SMS_PATH);

        let contacts_interface = ContactsInterface {
            event_sender: event_sender.clone(),
        };
        connection
            .object_server()
            .at(CONTACTS_PATH, contacts_interface)
            .await?;
        info!("Contacts interface registered at {}", CONTACTS_PATH);

        let conn_clone = connection.clone();
        let devices_clone = devices.clone();
        let event_sender_clone = event_sender.clone();
        let broadcast_tx_clone = broadcast_tx.clone();
        let sms_cache_for_service = sms_cache.clone();

        tokio::spawn(async move {
            debug!("Event handler started");
            while let Some(event) = event_receiver.recv().await {
                if let Err(e) = Self::handle_event(
                    &conn_clone,
                    event,
                    &devices_clone,
                    &event_sender_clone,
                    &sms_cache,
                    &current_device_id,
                    &broadcast_tx_clone,
                )
                .await
                {
                    error!("Event handler error: {:?}", e);
                }
            }
        });

        if let Some(mut clipboard_events) = clipboard_events {
            let devices = devices.clone();
            let event_sender = event_sender.clone();
            tokio::spawn(async move {
                while let Some(event) = clipboard_events.recv().await {
                    let ClipboardEvent::Changed(content) = event;
                    let candidates: Vec<DbusDevice> = devices
                        .lock()
                        .await
                        .values()
                        .filter(|device| device.is_paired && device.is_reachable)
                        .cloned()
                        .collect();

                    for device in candidates {
                        if kdeconnect_core::plugin_config::load_disabled_plugins(&device.id)
                            .await
                            .contains("clipboard")
                        {
                            continue;
                        }
                        let config = clipboard::load_plugin_config(&device.id).await;
                        if !config.auto_share || (content.sensitive && !config.send_password) {
                            continue;
                        }

                        let packet = ProtocolPacket::new(
                            PacketType::Clipboard,
                            json!({ "content": content.text.clone() }),
                        );
                        if let Err(error) = event_sender
                            .send(AppEvent::SendPacket(DeviceId(device.id.clone()), packet))
                        {
                            error!(
                                "Failed to queue automatic clipboard for {}: {error}",
                                device.id
                            );
                        } else {
                            debug!("Automatic clipboard queued for {}", device.id);
                        }
                    }
                }
            });
        }

        let core_handle = tokio::spawn(async move {
            core.run_event_loop().await;
        });

        tokio::spawn(async move {
            match core_handle.await {
                Ok(_) => error!("Core event loop exited unexpectedly - connections will fail"),
                Err(e) if e.is_panic() => {
                    error!("Core event loop PANICKED - connections will fail: {:?}", e)
                }
                Err(e) => error!("Core event loop cancelled: {:?}", e),
            }
        });

        Ok(Self {
            connection,
            event_sender,
            devices,
            sms_cache: sms_cache_for_service,
            clipboard,
        })
    }

    // broadcast_tx: dormant — see DORMANT note on VarlinkEvent / subscribe()
    // in varlink_server.rs. The .send() calls below are cheap no-ops until
    // that's resolved.
    async fn handle_event(
        connection: &Connection,
        event: ConnectionEvent,
        devices: &Arc<Mutex<HashMap<String, DbusDevice>>>,
        event_sender: &Arc<mpsc::UnboundedSender<AppEvent>>,
        sms_cache: &Arc<Mutex<Option<String>>>,
        current_device_id: &Arc<Mutex<Option<String>>>,
        broadcast_tx: &broadcast::Sender<crate::varlink_server::VarlinkEvent>,
    ) -> Result<()> {
        match event {
            ConnectionEvent::Connected((device_id, device)) => {
                info!("Device connected: {} ({})", device.name, device_id.0);

                *current_device_id.lock().await = Some(device_id.0.clone());

                let is_paired = matches!(device.pair_state, PairState::Paired);
                let dbus_device = DbusDevice {
                    id: device_id.0.clone(),
                    name: device.name.clone(),
                    device_type: "phone".to_string(),
                    is_paired,
                    is_reachable: true,
                };

                devices
                    .lock()
                    .await
                    .insert(device_id.0.clone(), dbus_device.clone());

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                let clipboard = { iface_ref.get().await.clipboard.clone() };

                DaemonInterface::device_connected(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                    dbus_device,
                )
                .await?;
                debug!("Device connected signal emitted");

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "device_connected".into(),
                    device_id: device_id.0.clone(),
                    device: devices.lock().await.get(&device_id.0).cloned(),
                    ..Default::default()
                });

                if is_paired {
                    let did = device_id.0.clone();

                    tokio::spawn(send_clipboard_connect_when_ready(
                        event_sender.clone(),
                        did.clone(),
                        clipboard,
                    ));

                    if let Some(cached) = load_contacts_cache(&did).await {
                        if let Ok(contacts_json) = serde_json::to_string(&cached) {
                            let iface_ref = connection
                                .object_server()
                                .interface::<_, ContactsInterface>(CONTACTS_PATH)
                                .await?;
                            ContactsInterface::contacts_received(
                                iface_ref.signal_emitter(),
                                contacts_json,
                            )
                            .await?;
                            debug!(
                                "Emitted cached contacts on connect ({} entries)",
                                cached.len()
                            );
                        }
                    }

                    if sms_cache.lock().await.is_none() {
                        if let Some(cached_sms) = load_sms_cache(&did).await {
                            *sms_cache.lock().await = Some(cached_sms);
                            debug!("Seeded in-memory SMS cache from disk on connect");
                        }
                    }

                    // Note: auto-requests for SMS/contacts are now handled in
                    // kdeconnect-core with plugin-enabled gating.
                }
            }
            ConnectionEvent::DevicePaired((device_id, device)) => {
                info!("Device paired: {} ({})", device.name, device_id.0);

                *current_device_id.lock().await = Some(device_id.0.clone());

                let dbus_device = DbusDevice {
                    id: device_id.0.clone(),
                    name: device.name.clone(),
                    device_type: "phone".to_string(),
                    is_paired: true,
                    is_reachable: true,
                };

                devices
                    .lock()
                    .await
                    .insert(device_id.0.clone(), dbus_device.clone());

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                let clipboard = { iface_ref.get().await.clipboard.clone() };

                DaemonInterface::device_paired(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                    dbus_device,
                )
                .await?;
                debug!("Device paired signal emitted");

                tokio::spawn(send_clipboard_connect_when_ready(
                    event_sender.clone(),
                    device_id.0.clone(),
                    clipboard,
                ));

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "device_paired".into(),
                    device_id: device_id.0.clone(),
                    device: devices.lock().await.get(&device_id.0).cloned(),
                    ..Default::default()
                });

                let sender = event_sender.clone();
                let did = device_id.0.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let sms_packet =
                        ProtocolPacket::new(PacketType::SmsRequestConversations, json!({}));
                    let _ = sender.send(AppEvent::SendPacket(DeviceId(did.clone()), sms_packet));
                    debug!("Auto-requested SMS conversations after pairing");

                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let contacts_packet = ProtocolPacket::new(
                        PacketType::ContactsRequestAllUidsTimestamps,
                        json!({}),
                    );
                    let _ = sender.send(AppEvent::SendPacket(DeviceId(did), contacts_packet));
                    debug!("Auto-requested contacts after pairing");
                });
            }
            ConnectionEvent::Disconnected(device_id) => {
                info!("Device disconnected: {}", device_id.0);

                // Mark unreachable but keep in map so UI can still show it
                // and allow pairing attempts after reconnect.
                let device_name = {
                    let mut map = devices.lock().await;
                    if let Some(dev) = map.get_mut(&device_id.0) {
                        dev.is_reachable = false;
                        Some(dev.name.clone())
                    } else {
                        None
                    }
                };

                // Auto-unmount the SFTP share: with the phone gone the FUSE
                // mount can only produce IO errors, so retire it now rather
                // than leaving a wedged entry in the file manager. Spawned —
                // unmounting talks to host processes and must not stall the
                // event loop.
                if let Some(name) = device_name {
                    let conn = connection.clone();
                    let did = device_id.0.clone();
                    tokio::spawn(async move {
                        match kdeconnect_core::plugins::sftp::unmount(&did, &name).await {
                            Ok(true) => {
                                info!("[sftp] auto-unmounted {} after disconnect", did);
                                if let Ok(iface_ref) = conn
                                    .object_server()
                                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                                    .await
                                {
                                    let _ = DaemonInterface::mount_state_changed(
                                        iface_ref.signal_emitter(),
                                        did,
                                        false,
                                    )
                                    .await;
                                }
                            }
                            Ok(false) => {}
                            Err(e) => warn!("[sftp] auto-unmount of {} failed: {}", did, e),
                        }
                    });
                }

                let mut cid = current_device_id.lock().await;
                if cid.as_deref() == Some(&device_id.0) {
                    *cid = None;
                }

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;

                DaemonInterface::device_disconnected(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                )
                .await?;
                debug!("Device disconnected signal emitted");

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "device_disconnected".into(),
                    device_id: device_id.0,
                    ..Default::default()
                });
            }
            ConnectionEvent::PairStateChanged((device_id, pair_state)) => {
                info!(
                    "Event: PairStateChanged - {} → {:?}",
                    device_id.0, pair_state
                );
                let is_paired = matches!(pair_state, PairState::Paired);

                {
                    let mut map = devices.lock().await;
                    if let Some(dev) = map.get_mut(&device_id.0) {
                        dev.is_paired = is_paired;
                    }
                }

                // Push updated device info immediately via device_connected signal
                // so the applet UI reflects the new pair state without waiting for poll.
                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                if let Some(dev) = devices.lock().await.get(&device_id.0).cloned() {
                    DaemonInterface::device_connected(
                        iface_ref.signal_emitter(),
                        device_id.0.clone(),
                        dev.clone(),
                    )
                    .await?;

                    let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                        event_type: "device_connected".into(),
                        device_id: device_id.0.clone(),
                        device: Some(dev),
                        ..Default::default()
                    });
                }
                debug!("PairStateChanged signal emitted for {}", device_id.0);
            }
            ConnectionEvent::SmsMessages(sms_data) => {
                info!(
                    "SMS messages received: {} messages",
                    sms_data.messages.len()
                );

                let messages_json = serde_json::to_string(&sms_data)?;
                debug!("SMS JSON size: {} bytes", messages_json.len());

                *sms_cache.lock().await = Some(messages_json.clone());

                if let Some(did) = current_device_id.lock().await.as_deref() {
                    save_sms_cache(did, &messages_json).await;
                }

                let iface_ref = connection
                    .object_server()
                    .interface::<_, SmsInterface>(SMS_PATH)
                    .await?;

                SmsInterface::sms_messages_received(iface_ref.signal_emitter(), messages_json)
                    .await?;
                debug!("SMS D-Bus signal emitted");
            }
            ConnectionEvent::ContactsReceived(contacts) => {
                info!("Contacts received: {} entries", contacts.len());

                if let Some(did) = current_device_id.lock().await.as_deref() {
                    save_contacts_cache(did, &contacts).await;
                }

                let contacts_json = serde_json::to_string(&contacts)?;

                let iface_ref = connection
                    .object_server()
                    .interface::<_, ContactsInterface>(CONTACTS_PATH)
                    .await?;

                ContactsInterface::contacts_received(iface_ref.signal_emitter(), contacts_json)
                    .await?;
                debug!("Contacts D-Bus signal emitted");
            }
            ConnectionEvent::ContactPhotosReceived(photos) => {
                info!("Contact photos received: {} entries", photos.len());

                if let Some(did) = current_device_id.lock().await.as_deref() {
                    save_contact_photos_cache(did, &photos).await;
                }

                let photos_json = serde_json::to_string(&photos)?;

                let iface_ref = connection
                    .object_server()
                    .interface::<_, ContactsInterface>(CONTACTS_PATH)
                    .await?;

                ContactsInterface::contact_photos_received(iface_ref.signal_emitter(), photos_json)
                    .await?;
                debug!("ContactPhotosReceived D-Bus signal emitted");
            }
            ConnectionEvent::SmsAttachmentReceived((_device_id, filename, path)) => {
                info!("SMS attachment received: {} -> {:?}", filename, path);

                let iface_ref = connection
                    .object_server()
                    .interface::<_, SmsInterface>(SMS_PATH)
                    .await?;

                SmsInterface::sms_attachment_received(
                    iface_ref.signal_emitter(),
                    filename,
                    path.display().to_string(),
                )
                .await?;
                debug!("SmsAttachmentReceived D-Bus signal emitted");
            }
            ConnectionEvent::UpdateTransferProgress(progress) => {
                info!("Current transfer progress: {}%", progress);

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;

                DaemonInterface::update_transfer_progress(iface_ref.signal_emitter(), progress)
                    .await?;

                debug!("UpdateTransferProgress D-Bus signal emitted");
            }
            ConnectionEvent::PairingRequested((device_id, device_name)) => {
                info!("Pairing requested by {} ({})", device_name, device_id.0);

                // Emit D-Bus signal so the applet can show Accept/Decline UI.
                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                DaemonInterface::pairing_requested(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                    device_name.clone(),
                )
                .await?;

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "pairing_requested".into(),
                    device_id: device_id.0,
                    message: Some(device_name),
                    ..Default::default()
                });

                // The D-Bus signal is the primary mechanism — the applet
                // subscription delivers it immediately and opens the popup.
            }
            ConnectionEvent::ClipboardReceived(content) => {
                info!("Clipboard received ({} bytes)", content.len());

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                let clipboard = { iface_ref.get().await.clipboard.clone() };
                if let Some(clipboard) = clipboard {
                    if let Err(error) = clipboard.set_text(content.clone()) {
                        error!("Failed to write phone clipboard to desktop: {error}");
                    }
                } else {
                    error!("Cannot write phone clipboard: background clipboard access unavailable");
                }

                DaemonInterface::clipboard_received(iface_ref.signal_emitter(), content.clone())
                    .await?;
                debug!("ClipboardReceived D-Bus signal emitted");

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "clipboard_received".into(),
                    device_id: current_device_id.lock().await.clone().unwrap_or_default(),
                    clipboard_content: Some(content),
                    ..Default::default()
                });
            }
            ConnectionEvent::ClipboardConnectReceived { content, timestamp } => {
                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                let clipboard = { iface_ref.get().await.clipboard.clone() };
                let local_timestamp = clipboard
                    .as_ref()
                    .and_then(ClipboardHandle::current)
                    .map_or(0, |content| content.timestamp);

                if !clipboard::should_accept_connect(timestamp, local_timestamp) {
                    info!(
                        "Ignored clipboard.connect: remote timestamp {:?}, local timestamp {}",
                        timestamp, local_timestamp
                    );
                    return Ok(());
                }

                info!(
                    "Accepted clipboard.connect ({} bytes, remote timestamp {:?}, local timestamp {})",
                    content.len(),
                    timestamp,
                    local_timestamp
                );
                if let Some(clipboard) = clipboard {
                    if let Err(error) = clipboard.set_text(content.clone()) {
                        error!("Failed to write connected device clipboard to desktop: {error}");
                    }
                } else {
                    error!(
                        "Cannot write connected device clipboard: background clipboard access unavailable"
                    );
                }

                DaemonInterface::clipboard_received(iface_ref.signal_emitter(), content.clone())
                    .await?;
                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "clipboard_received".into(),
                    device_id: current_device_id.lock().await.clone().unwrap_or_default(),
                    clipboard_content: Some(content),
                    ..Default::default()
                });
            }
            ConnectionEvent::StateUpdated(state) => {
                let device_id = match current_device_id.lock().await.clone() {
                    Some(id) => id,
                    None => {
                        debug!("StateUpdated but no current device id");
                        return Ok(());
                    }
                };
                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                match state {
                    DeviceState::Battery { level, charging } => {
                        DaemonInterface::battery_received(
                            iface_ref.signal_emitter(),
                            device_id.clone(),
                            level as i32,
                            charging,
                        )
                        .await?;
                        debug!("BatteryReceived D-Bus signal emitted");

                        let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                            event_type: "battery_received".into(),
                            device_id,
                            battery: Some((level as i64, charging)),
                            ..Default::default()
                        });
                    }
                    DeviceState::Connectivity((_, signal_strength)) => {
                        DaemonInterface::connectivity_received(
                            iface_ref.signal_emitter(),
                            device_id.clone(),
                            signal_strength,
                        )
                        .await?;
                        debug!("ConnectivityReceived D-Bus signal emitted");

                        let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                            event_type: "connectivity_received".into(),
                            device_id,
                            connectivity_strength: Some(signal_strength as i64),
                            ..Default::default()
                        });
                    }
                }
            }
            ConnectionEvent::RunCommandListReceived((device_id, commands)) => {
                info!(
                    "[dbus] RunCommandListReceived: {} commands from {}",
                    commands.len(),
                    device_id.0
                );
                let commands_json = serde_json::to_string(
                    &commands
                        .iter()
                        .map(|c| {
                            json!({
                                "key": c.key,
                                "name": c.name,
                                "command": c.command,
                            })
                        })
                        .collect::<Vec<_>>(),
                )
                .unwrap_or_default();

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                DaemonInterface::run_command_list_received(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                    commands_json.clone(),
                )
                .await?;
                debug!("RunCommandListReceived D-Bus signal emitted");

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "run_command_list_received".into(),
                    device_id: device_id.0,
                    commands_json: Some(commands_json),
                    ..Default::default()
                });
            }
            ConnectionEvent::SftpMountStateChanged((device_id, mounted)) => {
                info!(
                    "[dbus] MountStateChanged for {}: mounted={}",
                    device_id.0, mounted
                );

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                DaemonInterface::mount_state_changed(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                    mounted,
                )
                .await?;
                debug!("MountStateChanged D-Bus signal emitted");
            }
            ConnectionEvent::SftpBrowseFailed((device_id, message)) => {
                warn!("[dbus] BrowseFailed for {}: {}", device_id.0, message);

                let iface_ref = connection
                    .object_server()
                    .interface::<_, DaemonInterface>(DAEMON_PATH)
                    .await?;
                DaemonInterface::browse_failed(
                    iface_ref.signal_emitter(),
                    device_id.0.clone(),
                    message.clone(),
                )
                .await?;
                debug!("BrowseFailed D-Bus signal emitted");

                let _ = broadcast_tx.send(crate::varlink_server::VarlinkEvent {
                    event_type: "browse_failed".into(),
                    device_id: device_id.0,
                    message: Some(message),
                    ..Default::default()
                });
            }
            _ => {
                debug!("Unhandled event type received");
            }
        }

        Ok(())
    }
}
