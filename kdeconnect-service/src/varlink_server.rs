//! Varlink server — runs alongside D-Bus, handles applet/settings IPC via Unix socket.

use anyhow::Result;
use async_trait::async_trait;
use kdeconnect_core::{PacketType, ProtocolPacket, device::DeviceId, event::AppEvent};
use kdeconnect_varlink::iface::{
    self, BatteryState, Call_AcceptPairing, Call_BroadcastIdentity, Call_BrowseDevice,
    Call_GetCachedContactPhotos, Call_GetCachedContacts, Call_GetCachedSms,
    Call_GetDisabledPlugins, Call_GetPluginEnabled, Call_ListDevices, Call_MountedDevices,
    Call_PairDevice, Call_RejectPairing, Call_RequestContacts, Call_RequestConversation,
    Call_RequestConversations, Call_RequestRunCommands, Call_RequestSmsAttachment, Call_RingDevice,
    Call_RunCommand, Call_SendClipboard, Call_SendFiles, Call_SendPing, Call_SendSms,
    Call_SetPluginEnabled, Call_ShareClipboard, Call_Subscribe, Call_UnmountDevice,
    Call_UnpairDevice, Device, VarlinkInterface,
};
use kdeconnect_varlink::socket_address;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use varlink::{ListenAsyncConfig, listen_async};

use crate::clipboard::ClipboardHandle;
use crate::dbus_interface::DbusDevice;

// DORMANT: built and broadcast on every device/battery/connectivity/clipboard/
// pairing/run-command event in dbus_interface.rs, but nothing can actually
// receive it — see the note on `subscribe()` below for why. Harmless to leave
// wired (broadcast::Sender::send with no receivers is a cheap no-op), kept so
// the only work left when the crate is fixed is re-enabling Subscribe() itself.
#[derive(Debug, Clone, Default)]
pub struct VarlinkEvent {
    pub event_type: String,
    pub device_id: String,
    pub device: Option<DbusDevice>,
    pub battery: Option<(i64, bool)>,
    pub connectivity_strength: Option<i64>,
    pub clipboard_content: Option<String>,
    pub commands_json: Option<String>,
    pub message: Option<String>,
}

pub struct KdeConnectVarlinkService {
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
    devices: Arc<tokio::sync::Mutex<std::collections::HashMap<String, DbusDevice>>>,
    sms_cache: Arc<tokio::sync::Mutex<Option<String>>>,
    clipboard: Option<ClipboardHandle>,
    broadcast_tx: broadcast::Sender<VarlinkEvent>,
}

impl KdeConnectVarlinkService {
    pub fn new(
        event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
        devices: Arc<tokio::sync::Mutex<std::collections::HashMap<String, DbusDevice>>>,
        sms_cache: Arc<tokio::sync::Mutex<Option<String>>>,
        clipboard: Option<ClipboardHandle>,
        broadcast_tx: broadcast::Sender<VarlinkEvent>,
    ) -> Self {
        Self {
            event_sender,
            devices,
            sms_cache,
            clipboard,
            broadcast_tx,
        }
    }
}

fn to_varlink_device(d: &DbusDevice) -> Device {
    Device {
        id: d.id.clone(),
        name: d.name.clone(),
        device_type: d.device_type.clone(),
        is_paired: d.is_paired,
        is_reachable: d.is_reachable,
    }
}

#[async_trait]
impl VarlinkInterface for KdeConnectVarlinkService {
    async fn list_devices(&self, call: &mut dyn Call_ListDevices) -> varlink::Result<()> {
        let guard = self.devices.lock().await;
        let devices: Vec<Device> = guard.values().map(to_varlink_device).collect();
        call.reply(devices)
    }

    async fn pair_device(
        &self,
        call: &mut dyn Call_PairDevice,
        device_id: String,
    ) -> varlink::Result<()> {
        let _ = self.event_sender.send(AppEvent::Pair(DeviceId(device_id)));
        call.reply()
    }

    async fn unpair_device(
        &self,
        call: &mut dyn Call_UnpairDevice,
        device_id: String,
    ) -> varlink::Result<()> {
        let _ = self
            .event_sender
            .send(AppEvent::Unpair(DeviceId(device_id)));
        call.reply()
    }

    async fn send_ping(
        &self,
        call: &mut dyn Call_SendPing,
        device_id: String,
        message: String,
    ) -> varlink::Result<()> {
        let _ = self
            .event_sender
            .send(AppEvent::Ping((DeviceId(device_id), message)));
        call.reply()
    }

    async fn send_files(
        &self,
        call: &mut dyn Call_SendFiles,
        device_id: String,
        files: Vec<String>,
    ) -> varlink::Result<()> {
        let _ = self
            .event_sender
            .send(AppEvent::SendFiles((DeviceId(device_id), files)));
        call.reply()
    }

    async fn send_clipboard(
        &self,
        call: &mut dyn Call_SendClipboard,
        device_id: String,
        content: String,
    ) -> varlink::Result<()> {
        match crate::dbus_interface::send_clipboard_packet(
            &self.event_sender,
            &self.devices,
            device_id,
            content,
        )
        .await
        {
            Ok(()) => call.reply(),
            Err(error) => call.reply_service_error(error),
        }
    }

    async fn share_clipboard(
        &self,
        call: &mut dyn Call_ShareClipboard,
        device_id: String,
    ) -> varlink::Result<()> {
        let Some(clipboard) = self.clipboard.as_ref() else {
            return call.reply_service_error(
                "Background clipboard access is unavailable; COSMIC must expose ext- or wlr-data-control-v1 (Flatpak builds require host WAYLAND_DISPLAY/XDG_RUNTIME_DIR access)"
                    .to_string(),
            );
        };
        let Some(content) = clipboard.current() else {
            return call
                .reply_service_error("The current clipboard does not contain text".to_string());
        };
        match crate::dbus_interface::send_clipboard_packet(
            &self.event_sender,
            &self.devices,
            device_id,
            content.text,
        )
        .await
        {
            Ok(()) => call.reply(),
            Err(error) => call.reply_service_error(error),
        }
    }

    async fn run_command(
        &self,
        call: &mut dyn Call_RunCommand,
        device_id: String,
        key: String,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(PacketType::RunCommandRequest, json!({ "key": key }));
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn ring_device(
        &self,
        call: &mut dyn Call_RingDevice,
        device_id: String,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(PacketType::FindMyPhoneRequest, json!({}));
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn browse_device(
        &self,
        call: &mut dyn Call_BrowseDevice,
        device_id: String,
    ) -> varlink::Result<()> {
        // Fast path mirroring the D-Bus BrowseDevice: already mounted and
        // healthy → just reopen the file manager, skip the phone round-trip.
        let device_name = self
            .devices
            .lock()
            .await
            .get(&device_id)
            .map(|d| d.name.clone());
        if let Some(name) = device_name.as_deref() {
            if kdeconnect_core::plugins::sftp::open_mounted(&device_id, name).await {
                return call.reply();
            }
        }
        let packet = ProtocolPacket::new(PacketType::SftpRequest, json!({ "startBrowsing": true }));
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn unmount_device(
        &self,
        call: &mut dyn Call_UnmountDevice,
        device_id: String,
    ) -> varlink::Result<()> {
        let device_name = self
            .devices
            .lock()
            .await
            .get(&device_id)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        match kdeconnect_core::plugins::sftp::unmount(&device_id, &device_name).await {
            Ok(_) => call.reply(),
            Err(e) => call.reply_service_error(e.to_string()),
        }
    }

    async fn mounted_devices(&self, call: &mut dyn Call_MountedDevices) -> varlink::Result<()> {
        let pairs: Vec<(String, String)> = self
            .devices
            .lock()
            .await
            .iter()
            .map(|(id, d)| (id.clone(), d.name.clone()))
            .collect();
        call.reply(kdeconnect_core::plugins::sftp::mounted_devices(&pairs).await)
    }

    async fn broadcast_identity(
        &self,
        call: &mut dyn Call_BroadcastIdentity,
    ) -> varlink::Result<()> {
        let _ = self.event_sender.send(AppEvent::Broadcasting);
        call.reply()
    }

    async fn request_run_commands(
        &self,
        call: &mut dyn Call_RequestRunCommands,
        device_id: String,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(
            PacketType::RunCommandRequest,
            json!({ "requestCommandList": true }),
        );
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn set_plugin_enabled(
        &self,
        call: &mut dyn Call_SetPluginEnabled,
        device_id: String,
        plugin: String,
        enabled: bool,
    ) -> varlink::Result<()> {
        let _ = self.event_sender.send(AppEvent::SetPluginEnabled {
            device_id: DeviceId(device_id),
            plugin_id: plugin,
            enabled,
        });
        call.reply()
    }

    async fn get_plugin_enabled(
        &self,
        call: &mut dyn Call_GetPluginEnabled,
        device_id: String,
        plugin: String,
    ) -> varlink::Result<()> {
        let disabled = kdeconnect_core::plugin_config::load_disabled_plugins(&device_id).await;
        call.reply(!disabled.contains(&plugin))
    }

    async fn get_disabled_plugins(
        &self,
        call: &mut dyn Call_GetDisabledPlugins,
        device_id: String,
    ) -> varlink::Result<()> {
        let disabled = kdeconnect_core::plugin_config::load_disabled_plugins(&device_id).await;
        call.reply(disabled.into_iter().collect())
    }

    async fn accept_pairing(
        &self,
        call: &mut dyn Call_AcceptPairing,
        device_id: String,
    ) -> varlink::Result<()> {
        let _ = self
            .event_sender
            .send(AppEvent::AcceptPairing(DeviceId(device_id)));
        call.reply()
    }

    async fn reject_pairing(
        &self,
        call: &mut dyn Call_RejectPairing,
        device_id: String,
    ) -> varlink::Result<()> {
        let _ = self
            .event_sender
            .send(AppEvent::RejectPairing(DeviceId(device_id)));
        call.reply()
    }

    async fn request_conversations(
        &self,
        call: &mut dyn Call_RequestConversations,
        device_id: String,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(PacketType::SmsRequestConversations, json!({}));
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn request_conversation(
        &self,
        call: &mut dyn Call_RequestConversation,
        device_id: String,
        thread_id: i64,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(
            PacketType::SmsRequestConversation,
            json!({ "threadID": thread_id }),
        );
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn send_sms(
        &self,
        call: &mut dyn Call_SendSms,
        device_id: String,
        phone_number: String,
        message: String,
        attachments: Vec<String>,
    ) -> varlink::Result<()> {
        let packet =
            kdeconnect_core::plugins::sms::build_send_packet(&phone_number, &message, &attachments)
                .await;
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn get_cached_sms(
        &self,
        call: &mut dyn Call_GetCachedSms,
        device_id: String,
    ) -> varlink::Result<()> {
        if let Some(json) = self.sms_cache.lock().await.as_ref() {
            return call.reply(json.clone());
        }
        call.reply(
            crate::dbus_interface::load_sms_cache(&device_id)
                .await
                .unwrap_or_default(),
        )
    }

    async fn request_contacts(
        &self,
        call: &mut dyn Call_RequestContacts,
        device_id: String,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(PacketType::ContactsRequestAllUidsTimestamps, json!({}));
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    async fn get_cached_contacts(
        &self,
        call: &mut dyn Call_GetCachedContacts,
        device_id: String,
    ) -> varlink::Result<()> {
        let json = match crate::dbus_interface::load_contacts_cache(&device_id).await {
            Some(contacts) => serde_json::to_string(&contacts).unwrap_or_else(|_| "{}".to_string()),
            None => "{}".to_string(),
        };
        call.reply(json)
    }

    async fn get_cached_contact_photos(
        &self,
        call: &mut dyn Call_GetCachedContactPhotos,
        device_id: String,
    ) -> varlink::Result<()> {
        let json = match crate::dbus_interface::load_contact_photos_cache(&device_id).await {
            Some(photos) => serde_json::to_string(&photos).unwrap_or_else(|_| "{}".to_string()),
            None => "{}".to_string(),
        };
        call.reply(json)
    }

    async fn request_sms_attachment(
        &self,
        call: &mut dyn Call_RequestSmsAttachment,
        device_id: String,
        part_id: i64,
        unique_identifier: String,
    ) -> varlink::Result<()> {
        let packet = ProtocolPacket::new(
            PacketType::SmsRequestAttachment,
            json!({ "part_id": part_id, "unique_identifier": unique_identifier }),
        );
        let _ = self
            .event_sender
            .send(AppEvent::SendPacket(DeviceId(device_id), packet));
        call.reply()
    }

    // DORMANT: varlink_generator's async codegen doesn't support streaming
    // replies yet (AsyncCall::set_continues is a no-op, reply_struct just
    // overwrites one in-memory Option — see lib.rs of varlink_generator).
    // A handler that loops and replies repeatedly never returns, so nothing
    // is ever flushed to the socket and the client's first recv() blocks
    // forever. Re-enable Subscribe() (and the matching client-side call in
    // cosmic-ext-connect-applet's backend.rs) once that crate adds real
    // async streaming support. Until then this method is unreachable —
    // nothing should call it.
    async fn subscribe(&self, call: &mut dyn Call_Subscribe) -> varlink::Result<()> {
        let mut rx = self.broadcast_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let device = ev.device.as_ref().map(to_varlink_device);
                    let battery = ev
                        .battery
                        .map(|(level, is_charging)| BatteryState { level, is_charging });
                    call.reply(
                        ev.event_type,
                        ev.device_id,
                        device,
                        battery,
                        ev.connectivity_strength,
                        ev.clipboard_content,
                        ev.commands_json,
                        ev.message,
                    )?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
        Ok(())
    }
}

pub async fn run_varlink_server(
    event_sender: Arc<mpsc::UnboundedSender<AppEvent>>,
    devices: Arc<tokio::sync::Mutex<std::collections::HashMap<String, DbusDevice>>>,
    sms_cache: Arc<tokio::sync::Mutex<Option<String>>>,
    clipboard: Option<ClipboardHandle>,
    broadcast_tx: broadcast::Sender<VarlinkEvent>,
) -> Result<()> {
    let service = Arc::new(KdeConnectVarlinkService::new(
        event_sender,
        devices,
        sms_cache,
        clipboard,
        broadcast_tx,
    ));
    let handler = Arc::new(iface::new(service));

    listen_async(handler, &socket_address(), &ListenAsyncConfig::default()).await?;

    Ok(())
}
