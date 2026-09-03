use std::{
    fmt::Display,
    os::unix::fs::MetadataExt as _,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{fs::File, io::AsyncRead};

pub const PROTOCOL_VERSION: usize = 8;

fn serialize_packet_type<S>(pt: &PacketType, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let s: String = pt.to_string();
    serializer.serialize_str(&s)
}

fn deserialize_packet_type<'de, D>(deserializer: D) -> Result<PacketType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = String::deserialize(deserializer)?;
    Ok(PacketType::from(s))
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum PacketType {
    Battery,
    BatteryRequest,
    Clipboard,
    ClipboardConnect,
    ConnectivityReport,
    ConnectivityReportRequest,
    ContactsRequestAllUidsTimestamps,
    ContactsRequestVcardsByUid,
    ContactsResponseUidsTimestamps,
    ContactsResponseVcards,
    FindMyPhoneRequest,
    Lock,
    LockRequest,
    MousePadEcho,
    MousePadKeyboardState,
    MousePadRequest,
    Mpris,
    MprisRequest,
    Notification,
    NotificationAction,
    NotificationReply,
    NotificationRequest,
    Identity,
    Pair,
    Ping,
    Presenter,
    RunCommand,
    RunCommandRequest,
    Sftp,
    SftpRequest,
    ShareRequest,
    ShareRequestUpdate,
    SmsAttachmentFile,
    SmsMessages,
    SmsRequest,
    SmsRequestAttachment,
    SmsRequestConversation,
    SmsRequestConversations,
    SystemVolume,
    SystemVolumeRequest,
    Telephony,
    TelephonyRequestMute,
    /// Any packet type not recognized by this implementation.
    /// Stored so it can be logged and gracefully ignored rather than panicking.
    Unknown(String),
}

/// Canonical wire string for each known packet type. Single source of truth
/// for both parsing (`From<String>`) and serializing (`Display`) — previously
/// these were two separately hand-maintained 40-arm matches that could (and
/// did) drift out of sync with each other.
const PACKET_TYPES: &[(&str, PacketType)] = &[
    ("kdeconnect.battery", PacketType::Battery),
    ("kdeconnect.battery.request", PacketType::BatteryRequest),
    ("kdeconnect.clipboard", PacketType::Clipboard),
    ("kdeconnect.clipboard.connect", PacketType::ClipboardConnect),
    (
        "kdeconnect.connectivity_report",
        PacketType::ConnectivityReport,
    ),
    (
        "kdeconnect.connectivity_report.request",
        PacketType::ConnectivityReportRequest,
    ),
    (
        "kdeconnect.contacts.request_all_uids_timestamps",
        PacketType::ContactsRequestAllUidsTimestamps,
    ),
    (
        "kdeconnect.contacts.request_vcards_by_uid",
        PacketType::ContactsRequestVcardsByUid,
    ),
    (
        "kdeconnect.contacts.response_uids_timestamps",
        PacketType::ContactsResponseUidsTimestamps,
    ),
    (
        "kdeconnect.contacts.response_vcards",
        PacketType::ContactsResponseVcards,
    ),
    (
        "kdeconnect.findmyphone.request",
        PacketType::FindMyPhoneRequest,
    ),
    ("kdeconnect.lock", PacketType::Lock),
    ("kdeconnect.lock.request", PacketType::LockRequest),
    ("kdeconnect.mousepad.echo", PacketType::MousePadEcho),
    (
        "kdeconnect.mousepad.keyboardstate",
        PacketType::MousePadKeyboardState,
    ),
    ("kdeconnect.mousepad.request", PacketType::MousePadRequest),
    ("kdeconnect.mpris", PacketType::Mpris),
    ("kdeconnect.mpris.request", PacketType::MprisRequest),
    ("kdeconnect.notification", PacketType::Notification),
    (
        "kdeconnect.notification.action",
        PacketType::NotificationAction,
    ),
    (
        "kdeconnect.notification.reply",
        PacketType::NotificationReply,
    ),
    (
        "kdeconnect.notification.request",
        PacketType::NotificationRequest,
    ),
    ("kdeconnect.identity", PacketType::Identity),
    ("kdeconnect.pair", PacketType::Pair),
    ("kdeconnect.ping", PacketType::Ping),
    ("kdeconnect.presenter", PacketType::Presenter),
    ("kdeconnect.runcommand", PacketType::RunCommand),
    (
        "kdeconnect.runcommand.request",
        PacketType::RunCommandRequest,
    ),
    ("kdeconnect.sftp", PacketType::Sftp),
    ("kdeconnect.sftp.request", PacketType::SftpRequest),
    ("kdeconnect.share.request", PacketType::ShareRequest),
    (
        "kdeconnect.share.request.update",
        PacketType::ShareRequestUpdate,
    ),
    (
        "kdeconnect.sms.attachment_file",
        PacketType::SmsAttachmentFile,
    ),
    ("kdeconnect.sms.messages", PacketType::SmsMessages),
    ("kdeconnect.sms.request", PacketType::SmsRequest),
    (
        "kdeconnect.sms.request_attachment",
        PacketType::SmsRequestAttachment,
    ),
    (
        "kdeconnect.sms.request_conversation",
        PacketType::SmsRequestConversation,
    ),
    (
        "kdeconnect.sms.request_conversations",
        PacketType::SmsRequestConversations,
    ),
    ("kdeconnect.systemvolume", PacketType::SystemVolume),
    (
        "kdeconnect.systemvolume.request",
        PacketType::SystemVolumeRequest,
    ),
    ("kdeconnect.telephony", PacketType::Telephony),
    (
        "kdeconnect.telephony.request_mute",
        PacketType::TelephonyRequestMute,
    ),
];

/// Legacy/alternate wire strings accepted on parse but never emitted.
const PACKET_TYPE_ALIASES: &[(&str, PacketType)] = &[(
    "kdeconnect.contacts.response_all_uids_timestamps",
    PacketType::ContactsResponseUidsTimestamps,
)];

impl From<String> for PacketType {
    fn from(value: String) -> Self {
        if let Some(entry) = PACKET_TYPES.iter().find(|entry| entry.0 == value.as_str()) {
            return entry.1.clone();
        }
        if let Some(entry) = PACKET_TYPE_ALIASES
            .iter()
            .find(|entry| entry.0 == value.as_str())
        {
            return entry.1.clone();
        }
        tracing::debug!("Unknown packet type received: {}", value);
        PacketType::Unknown(value)
    }
}

impl Display for PacketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let PacketType::Unknown(s) = self {
            return write!(f, "{}", s);
        }
        let s = PACKET_TYPES
            .iter()
            .find(|entry| entry.1 == *self)
            .map(|entry| entry.0)
            .unwrap_or("");
        write!(f, "{}", s)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProtocolPacket {
    pub id: Option<u128>,
    #[serde(rename = "type")]
    #[serde(
        serialize_with = "serialize_packet_type",
        deserialize_with = "deserialize_packet_type"
    )]
    pub packet_type: PacketType,
    pub body: Value,
    #[serde(rename = "payloadSize")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_size: Option<u64>,
    #[serde(rename = "payloadTransferInfo")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_transfer_info: Option<PacketPayloadTransferInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PacketPayloadTransferInfo {
    pub port: u16,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Desktop,
    Laptop,
    Phone,
    Tablet,
    Tv,
}

impl Display for DeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceType::Desktop => write!(f, "desktop"),
            DeviceType::Laptop => write!(f, "laptop"),
            DeviceType::Phone => write!(f, "phone"),
            DeviceType::Tablet => write!(f, "tablet"),
            DeviceType::Tv => write!(f, "tv"),
        }
    }
}

impl ProtocolPacket {
    pub fn new(t: PacketType, body: Value) -> Self {
        Self {
            id: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            ),
            packet_type: t,
            body,
            payload_size: None,
            payload_transfer_info: None,
        }
    }

    pub fn new_with_payload(
        t: PacketType,
        body: Value,
        payload_size: u64,
        payload_transfer_info: Option<PacketPayloadTransferInfo>,
    ) -> Self {
        Self {
            payload_size: Some(payload_size),
            payload_transfer_info,
            ..Self::new(t, body)
        }
    }

    pub fn from_raw(raw: &[u8]) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(raw)?)
    }

    pub fn as_raw(&self) -> anyhow::Result<Vec<u8>> {
        let str = serde_json::to_string(self)?;
        Ok(format!("{}\n", str).as_bytes().to_vec())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub device_id: String,
    pub device_name: String,
    pub device_type: DeviceType,
    pub incoming_capabilities: Vec<String>,
    pub outgoing_capabilities: Vec<String>,
    pub protocol_version: usize,
    pub tcp_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub struct Pair {
    pub pair: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

impl Pair {
    pub fn new(pair: bool) -> Self {
        let timestamp = pair.then(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });
        Pair { pair, timestamp }
    }
}

pub struct DeviceFile<S: AsyncRead + Sync + Send + Unpin> {
    pub buf: S,
    pub size: u64,
}

pub struct DevicePayload<S: AsyncRead + Sync + Send + Unpin> {
    pub buf: S,
    pub size: u64,
}

impl<S: AsyncRead + Sync + Send + Unpin> From<DeviceFile<S>> for DevicePayload<S> {
    fn from(file: DeviceFile<S>) -> Self {
        Self {
            buf: file.buf,
            size: file.size,
        }
    }
}

impl DeviceFile<File> {
    pub async fn try_from_tokio(file: File) -> anyhow::Result<Self> {
        file.sync_all().await?;
        let metadata = file.metadata().await?;
        Ok(DeviceFile {
            buf: file,
            size: metadata.size().try_into().map_err(std::io::Error::other)?,
        })
    }

    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path: &Path = path.as_ref();
        Self::try_from_tokio(File::open(path).await?).await
    }
}
