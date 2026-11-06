use std::collections::HashMap;
use tokio::io::AsyncRead;

use crate::{
    device::{Device, DeviceId, DeviceState, PairState},
    plugins::mpris::{Mpris, MprisAction, MprisRequest},
    plugins::sms::SmsMessages,
    protocol::ProtocolPacket,
};

/// A single command offered by a remote device via the RunCommand plugin.
#[derive(Debug, Clone)]
pub struct RemoteCommand {
    pub key: String,
    pub name: String,
    pub command: String,
}

pub enum CoreEvent {
    DeviceDiscovered(Device),
    DevicePaired((DeviceId, Device)),
    DevicePairCancelled(DeviceId),
    DevicePairStateChanged((DeviceId, PairState)),
    PacketReceived {
        device: DeviceId,
        packet: ProtocolPacket,
    },
    SendPacket {
        device: DeviceId,
        packet: ProtocolPacket,
    },

    SendPaylod {
        device: DeviceId,
        packet: ProtocolPacket,
        payload: Box<dyn AsyncRead + Sync + Send + Unpin>,
        payload_size: u64,
    },
    Error(String),
}

#[derive(Debug)]
pub enum AppEvent {
    Broadcasting,
    Disconnect(DeviceId),
    Pair(DeviceId),
    AcceptPairing(DeviceId),
    RejectPairing(DeviceId),
    Ping((DeviceId, String)),
    Unpair(DeviceId),
    SendFiles((DeviceId, Vec<String>)),
    MprisAction((DeviceId, String, MprisAction)),
    SendMprisRequest((DeviceId, MprisRequest)),
    SendPacket(DeviceId, ProtocolPacket),
    /// Queue a packet and report whether a live transport writer accepted it.
    SendPacketWithReply(
        DeviceId,
        ProtocolPacket,
        tokio::sync::oneshot::Sender<Result<(), String>>,
    ),
    PushLocalCommands(DeviceId),
    SetPluginEnabled {
        device_id: DeviceId,
        plugin_id: String,
        enabled: bool,
    },
}

#[derive(Debug, Clone)]
pub enum ConnectionEvent {
    ClipboardReceived(String),
    ClipboardConnectReceived {
        content: String,
        timestamp: Option<u64>,
    },
    Connected((DeviceId, Device)),
    DevicePaired((DeviceId, Device)),
    Disconnected(DeviceId),
    StateUpdated(DeviceState),
    PairStateChanged((DeviceId, PairState)),
    Mpris((DeviceId, Mpris)),
    SmsMessages(SmsMessages),
    ContactsReceived(HashMap<String, String>),
    /// A shared file was fully saved. Payload is (device_id, local path).
    FileReceived((DeviceId, std::path::PathBuf)),
    UpdateTransferProgress(u8),
    /// Phone sent pair:true and is waiting for user decision.
    /// Payload is (device_id, device_name).
    PairingRequested((DeviceId, String)),
    /// Phone sent its command list via kdeconnect.runcommand.
    RunCommandListReceived((DeviceId, Vec<RemoteCommand>)),
    /// SFTP browse (mount/preflight) failed for a device. Payload is
    /// (device_id, user-facing message).
    SftpBrowseFailed((DeviceId, String)),
    /// SFTP mount state changed: true right after a successful mount,
    /// false after an unmount (manual or automatic on disconnect).
    SftpMountStateChanged((DeviceId, bool)),
    /// A full-resolution MMS attachment finished downloading to local
    /// cache. Payload is (device_id, filename/unique_identifier, saved
    /// path) — filename doubles as the correlation key since upstream
    /// KDE Connect's `attachment_file` response carries no part_id.
    SmsAttachmentReceived((DeviceId, String, std::path::PathBuf)),
    /// Phone -> base64-encoded photo, extracted from the PHOTO property of
    /// vcards already being parsed for `ContactsReceived`. Kept as base64
    /// the whole way to the UI (same as SMS thumbnails) rather than
    /// decoding here only to re-encode for the D-Bus signal/cache.
    ContactPhotosReceived(HashMap<String, String>),
}
