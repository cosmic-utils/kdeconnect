//! D-Bus client library for KDE Connect service

use anyhow::Result;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::error;
use zbus::{Connection, proxy};

/// Device information
#[derive(
    Debug,
    Clone,
    Serialize,
    Deserialize,
    zbus::zvariant::Type,
    zbus::zvariant::Value,
    zbus::zvariant::OwnedValue,
)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub is_paired: bool,
    pub is_reachable: bool,
}

/// Events from the D-Bus service
#[derive(Debug, Clone)]
pub enum ServiceEvent {
    DeviceConnected(String, Device),
    DevicePaired(String, Device),
    DeviceDisconnected(String),
    SmsMessagesReceived(String),               // JSON string
    ContactsReceived(HashMap<String, String>), // phone -> name
    PairingRequested(String, String),          // device_id, device_name
    ClipboardReceived(String),                 // clipboard content from phone
    BatteryReceived(String, i32, bool),        // device_id, level, is_charging
    ConnectivityReceived(String, i32),         // device_id, signal_strength
    RunCommandListReceived(String, String),    // device_id, commands_json
    BrowseFailed(String, String),              // device_id, message
    MountStateChanged(String, bool),           // device_id, mounted
    /// (filename/unique_identifier, local file path)
    SmsAttachmentReceived(String, String),
    /// Phone -> base64-encoded photo
    ContactPhotosReceived(HashMap<String, String>),
}

/// D-Bus proxy for daemon interface
#[proxy(
    interface = "io.github.hepp3n.kdeconnect.Daemon",
    default_service = "io.github.hepp3n.kdeconnect",
    default_path = "/io/github/hepp3n/kdeconnect/Daemon"
)]
trait Daemon {
    async fn list_devices(&self) -> zbus::Result<Vec<Device>>;
    async fn pair_device(&self, device_id: &str) -> zbus::Result<()>;
    async fn unpair_device(&self, device_id: &str) -> zbus::Result<()>;
    async fn send_ping(&self, device_id: &str, message: &str) -> zbus::Result<()>;
    async fn send_files(&self, device_id: &str, files: Vec<String>) -> zbus::Result<()>;
    async fn send_clipboard(&self, device_id: &str, content: &str) -> zbus::Result<()>;
    async fn share_clipboard(&self, device_id: &str) -> zbus::Result<()>;
    async fn ring_device(&self, device_id: &str) -> zbus::Result<()>;
    async fn browse_device(&self, device_id: &str) -> zbus::Result<()>;
    async fn unmount_device(&self, device_id: &str) -> zbus::Result<()>;
    async fn mounted_devices(&self) -> zbus::Result<Vec<String>>;
    async fn set_plugin_enabled(
        &self,
        device_id: &str,
        plugin_id: &str,
        enabled: bool,
    ) -> zbus::Result<()>;
    async fn get_disabled_plugins(&self, device_id: &str) -> zbus::Result<Vec<String>>;
    async fn broadcast_identity(&self) -> zbus::Result<()>;
    async fn accept_pairing(&self, device_id: &str) -> zbus::Result<()>;
    async fn reject_pairing(&self, device_id: &str) -> zbus::Result<()>;
    async fn run_command(&self, device_id: &str, key: &str) -> zbus::Result<()>;
    async fn request_run_commands(&self, device_id: &str) -> zbus::Result<()>;
    async fn push_local_commands(&self, device_id: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn pairing_requested(&self, device_id: String, device_name: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn update_transfer_progress(&self, progress: u8) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_connected(&self, device_id: String, device: Device) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_paired(&self, device_id: String, device: Device) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn device_disconnected(&self, device_id: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn clipboard_received(&self, content: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn battery_received(
        &self,
        device_id: String,
        level: i32,
        is_charging: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn connectivity_received(
        &self,
        device_id: String,
        signal_strength: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn run_command_list_received(
        &self,
        device_id: String,
        commands_json: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn browse_failed(&self, device_id: String, message: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn mount_state_changed(&self, device_id: String, mounted: bool) -> zbus::Result<()>;
}

/// D-Bus proxy for SMS interface
#[proxy(
    interface = "io.github.hepp3n.kdeconnect.Sms",
    default_service = "io.github.hepp3n.kdeconnect",
    default_path = "/io/github/hepp3n/kdeconnect/Sms"
)]
trait Sms {
    async fn request_conversations(&self, device_id: &str) -> zbus::Result<()>;
    async fn request_conversation(&self, device_id: &str, thread_id: i64) -> zbus::Result<()>;
    async fn send_sms(
        &self,
        device_id: &str,
        phone_number: &str,
        message: &str,
        attachments: Vec<String>,
    ) -> zbus::Result<()>;
    async fn get_cached_sms(&self, device_id: &str) -> zbus::Result<String>;
    /// Requests the full-resolution file for one MMS attachment. The
    /// result arrives later via `sms_attachment_received`, since the
    /// phone fetches and transfers it asynchronously.
    async fn request_sms_attachment(
        &self,
        device_id: &str,
        part_id: i64,
        unique_identifier: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn sms_messages_received(&self, messages_json: String) -> zbus::Result<()>;

    /// `filename` is the same `unique_identifier` the request was made
    /// with — that's how the caller matches this back to a message.
    #[zbus(signal)]
    async fn sms_attachment_received(&self, filename: String, path: String) -> zbus::Result<()>;
}

/// D-Bus proxy for Contacts interface
#[proxy(
    interface = "io.github.hepp3n.kdeconnect.Contacts",
    default_service = "io.github.hepp3n.kdeconnect",
    default_path = "/io/github/hepp3n/kdeconnect/Contacts"
)]
trait Contacts {
    async fn request_contacts(&self, device_id: &str) -> zbus::Result<()>;
    async fn get_cached_contacts(&self, device_id: &str) -> zbus::Result<String>;
    async fn get_cached_contact_photos(&self, device_id: &str) -> zbus::Result<String>;

    #[zbus(signal)]
    async fn contacts_received(&self, contacts_json: String) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn contact_photos_received(&self, photos_json: String) -> zbus::Result<()>;
}

/// Main client for KDE Connect service
pub struct KdeConnectClient {
    daemon_proxy: DaemonProxy<'static>,
    sms_proxy: SmsProxy<'static>,
    contacts_proxy: ContactsProxy<'static>,
}

impl KdeConnectClient {
    /// Connect to the KDE Connect service
    pub async fn new() -> Result<Self> {
        let connection = Connection::session().await?;

        let daemon_proxy = DaemonProxy::new(&connection).await?;
        let sms_proxy = SmsProxy::new(&connection).await?;
        let contacts_proxy = ContactsProxy::new(&connection).await?;

        Ok(Self {
            daemon_proxy,
            sms_proxy,
            contacts_proxy,
        })
    }

    /// List all devices
    pub async fn list_devices(&self) -> Result<Vec<Device>> {
        Ok(self.daemon_proxy.list_devices().await?)
    }

    /// Pair with a device
    pub async fn pair_device(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.pair_device(device_id).await?)
    }

    /// Unpair from a device
    pub async fn unpair_device(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.unpair_device(device_id).await?)
    }

    /// Send a ping
    pub async fn send_ping(&self, device_id: &str, message: &str) -> Result<()> {
        Ok(self.daemon_proxy.send_ping(device_id, message).await?)
    }

    /// Send files
    pub async fn send_files(&self, device_id: &str, files: Vec<String>) -> Result<()> {
        Ok(self.daemon_proxy.send_files(device_id, files).await?)
    }

    /// Send clipboard content
    pub async fn send_clipboard(&self, device_id: &str, content: &str) -> Result<()> {
        Ok(self.daemon_proxy.send_clipboard(device_id, content).await?)
    }

    /// Send the current desktop clipboard using the service's data-control backend.
    pub async fn share_clipboard(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.share_clipboard(device_id).await?)
    }

    /// Ring a device (findmyphone)
    pub async fn ring_device(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.ring_device(device_id).await?)
    }

    /// Ask a device to start its SFTP server so we can browse its filesystem
    pub async fn browse_device(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.browse_device(device_id).await?)
    }

    /// Unmount a device's SFTP share (the counterpart of browse_device)
    pub async fn unmount_device(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.unmount_device(device_id).await?)
    }

    /// Device IDs whose SFTP share is currently mounted
    pub async fn mounted_devices(&self) -> Result<Vec<String>> {
        Ok(self.daemon_proxy.mounted_devices().await?)
    }

    /// Enable or disable a plugin for a device
    pub async fn set_plugin_enabled(
        &self,
        device_id: &str,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<()> {
        Ok(self
            .daemon_proxy
            .set_plugin_enabled(device_id, plugin_id, enabled)
            .await?)
    }

    pub async fn accept_pairing(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.accept_pairing(device_id).await?)
    }

    pub async fn reject_pairing(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.reject_pairing(device_id).await?)
    }

    pub async fn get_disabled_plugins(&self, device_id: &str) -> Result<Vec<String>> {
        Ok(self.daemon_proxy.get_disabled_plugins(device_id).await?)
    }

    /// Broadcast our identity over UDP to trigger device discovery
    pub async fn broadcast_identity(&self) -> Result<()> {
        Ok(self.daemon_proxy.broadcast_identity().await?)
    }

    /// Execute a remote command on a device by key
    pub async fn run_command(&self, device_id: &str, key: &str) -> Result<()> {
        Ok(self.daemon_proxy.run_command(device_id, key).await?)
    }

    /// Request the remote command list from a device
    pub async fn request_run_commands(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.request_run_commands(device_id).await?)
    }

    /// Push our local command list to a connected device immediately.
    pub async fn push_local_commands(&self, device_id: &str) -> Result<()> {
        Ok(self.daemon_proxy.push_local_commands(device_id).await?)
    }

    /// Request SMS conversations
    pub async fn request_conversations(&self, device_id: &str) -> Result<()> {
        Ok(self.sms_proxy.request_conversations(device_id).await?)
    }

    /// Request specific conversation
    pub async fn request_conversation(&self, device_id: &str, thread_id: i64) -> Result<()> {
        Ok(self
            .sms_proxy
            .request_conversation(device_id, thread_id)
            .await?)
    }

    /// Send an SMS message, optionally with file attachments (MMS). Each
    /// entry in `attachments` is a local file path; the service reads and
    /// base64-encodes them before sending.
    pub async fn send_sms(
        &self,
        device_id: &str,
        phone_number: &str,
        message: &str,
        attachments: Vec<String>,
    ) -> Result<()> {
        Ok(self
            .sms_proxy
            .send_sms(device_id, phone_number, message, attachments)
            .await?)
    }

    /// Fetch cached SMS — in-memory in service, disk fallback, empty string if neither
    pub async fn get_cached_sms(&self, device_id: &str) -> Result<String> {
        Ok(self.sms_proxy.get_cached_sms(device_id).await?)
    }

    /// Requests the full-resolution file for one MMS attachment. The
    /// download happens asynchronously — listen for
    /// `ServiceEvent::SmsAttachmentReceived` for the result.
    pub async fn request_sms_attachment(
        &self,
        device_id: &str,
        part_id: i64,
        unique_identifier: &str,
    ) -> Result<()> {
        Ok(self
            .sms_proxy
            .request_sms_attachment(device_id, part_id, unique_identifier)
            .await?)
    }

    /// Manually request contacts sync for a device
    pub async fn request_contacts(&self, device_id: &str) -> Result<()> {
        Ok(self.contacts_proxy.request_contacts(device_id).await?)
    }

    /// Get cached contacts as a raw JSON string (phone → name map)
    pub async fn get_cached_contacts(&self, device_id: &str) -> Result<String> {
        Ok(self.contacts_proxy.get_cached_contacts(device_id).await?)
    }

    /// Get cached contact photos as a raw JSON string (phone → base64)
    pub async fn get_cached_contact_photos(&self, device_id: &str) -> Result<String> {
        Ok(self
            .contacts_proxy
            .get_cached_contact_photos(device_id)
            .await?)
    }

    /// Create a stream of service events
    pub async fn listen_for_events(
        &self,
    ) -> Result<futures::stream::BoxStream<'static, ServiceEvent>> {
        use futures::stream::select_all;

        let connected = self
            .daemon_proxy
            .receive_device_connected()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::DeviceConnected(
                        args.device_id.clone(),
                        args.device.clone(),
                    )),
                    Err(e) => {
                        error!("Failed to parse DeviceConnected signal: {:?}", e);
                        None
                    }
                }
            });

        let paired = self
            .daemon_proxy
            .receive_device_paired()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::DevicePaired(
                        args.device_id.clone(),
                        args.device.clone(),
                    )),
                    Err(e) => {
                        error!("Failed to parse DevicePaired signal: {:?}", e);
                        None
                    }
                }
            });

        let disconnected = self
            .daemon_proxy
            .receive_device_disconnected()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::DeviceDisconnected(args.device_id.clone())),
                    Err(e) => {
                        error!("Failed to parse DeviceDisconnected signal: {:?}", e);
                        None
                    }
                }
            });

        let sms = self
            .sms_proxy
            .receive_sms_messages_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::SmsMessagesReceived(
                        args.messages_json.clone(),
                    )),
                    Err(e) => {
                        error!("Failed to parse SmsMessagesReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let contacts = self
            .contacts_proxy
            .receive_contacts_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => {
                        let map: HashMap<String, String> =
                            serde_json::from_str(&args.contacts_json).unwrap_or_default();
                        Some(ServiceEvent::ContactsReceived(map))
                    }
                    Err(e) => {
                        error!("Failed to parse ContactsReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let pairing_req = self
            .daemon_proxy
            .receive_pairing_requested()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::PairingRequested(
                        args.device_id.clone(),
                        args.device_name.clone(),
                    )),
                    Err(e) => {
                        error!("Failed to parse PairingRequested signal: {:?}", e);
                        None
                    }
                }
            });

        let clipboard = self
            .daemon_proxy
            .receive_clipboard_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::ClipboardReceived(args.content.clone())),
                    Err(e) => {
                        error!("Failed to parse ClipboardReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let battery = self
            .daemon_proxy
            .receive_battery_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::BatteryReceived(
                        args.device_id.clone(),
                        args.level,
                        args.is_charging,
                    )),
                    Err(e) => {
                        error!("Failed to parse BatteryReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let connectivity = self
            .daemon_proxy
            .receive_connectivity_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::ConnectivityReceived(
                        args.device_id.clone(),
                        args.signal_strength,
                    )),
                    Err(e) => {
                        error!("Failed to parse ConnectivityReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let run_command_list = self
            .daemon_proxy
            .receive_run_command_list_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::RunCommandListReceived(
                        args.device_id.clone(),
                        args.commands_json.clone(),
                    )),
                    Err(e) => {
                        error!("Failed to parse RunCommandListReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let browse_failed =
            self.daemon_proxy
                .receive_browse_failed()
                .await?
                .filter_map(|s| async move {
                    match s.args() {
                        Ok(args) => Some(ServiceEvent::BrowseFailed(
                            args.device_id.clone(),
                            args.message.clone(),
                        )),
                        Err(e) => {
                            error!("Failed to parse BrowseFailed signal: {:?}", e);
                            None
                        }
                    }
                });

        let mount_state = self
            .daemon_proxy
            .receive_mount_state_changed()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::MountStateChanged(
                        args.device_id.clone(),
                        args.mounted,
                    )),
                    Err(e) => {
                        error!("Failed to parse MountStateChanged signal: {:?}", e);
                        None
                    }
                }
            });

        let attachment = self
            .sms_proxy
            .receive_sms_attachment_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => Some(ServiceEvent::SmsAttachmentReceived(
                        args.filename.clone(),
                        args.path.clone(),
                    )),
                    Err(e) => {
                        error!("Failed to parse SmsAttachmentReceived signal: {:?}", e);
                        None
                    }
                }
            });

        let contact_photos = self
            .contacts_proxy
            .receive_contact_photos_received()
            .await?
            .filter_map(|s| async move {
                match s.args() {
                    Ok(args) => {
                        let map: HashMap<String, String> =
                            serde_json::from_str(&args.photos_json).unwrap_or_default();
                        Some(ServiceEvent::ContactPhotosReceived(map))
                    }
                    Err(e) => {
                        error!("Failed to parse ContactPhotosReceived signal: {:?}", e);
                        None
                    }
                }
            });

        Ok(Box::pin(select_all(vec![
            Box::pin(connected) as futures::stream::BoxStream<'static, ServiceEvent>,
            Box::pin(paired),
            Box::pin(disconnected),
            Box::pin(sms),
            Box::pin(contacts),
            Box::pin(pairing_req),
            Box::pin(clipboard),
            Box::pin(battery),
            Box::pin(connectivity),
            Box::pin(run_command_list),
            Box::pin(browse_failed),
            Box::pin(mount_state),
            Box::pin(attachment),
            Box::pin(contact_photos),
        ])))
    }

    pub async fn transfer_progress_stream(&self) -> impl futures::stream::Stream<Item = u8> {
        let daemon_update_transfer_progress = self
            .daemon_proxy
            .receive_update_transfer_progress()
            .await
            .unwrap();

        let update_transfer_stream =
            daemon_update_transfer_progress.filter_map(|signal| async move {
                match signal.args() {
                    Ok(args) => Some(args.progress),
                    Err(e) => {
                        error!("Failed to parse UpdateTransferProgress signal: {:?}", e);
                        None
                    }
                }
            });

        let result = Box::pin(update_transfer_stream);

        futures::stream::select_all(vec![result])
    }
}
