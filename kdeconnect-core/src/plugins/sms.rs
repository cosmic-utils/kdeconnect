use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::event::ConnectionEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsMessages {
    pub messages: Vec<SmsMessage>,
    pub version: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsMessage {
    #[serde(rename = "_id")]
    pub id: i64,
    #[serde(default)]
    pub addresses: Vec<SmsAddress>,
    #[serde(default)]
    pub attachments: Vec<SmsAttachment>,
    #[serde(default)]
    pub body: String,
    pub date: i64,
    #[serde(rename = "type", default)]
    pub message_type: i32,
    #[serde(default)]
    pub read: i32,
    pub thread_id: i64,
    #[serde(default)]
    pub sub_id: Option<i32>,
    #[serde(default)]
    pub event: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsAddress {
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsAttachment {
    pub part_id: i64,
    #[serde(default)]
    pub mime_type: String,
    pub encoded_thumbnail: Option<String>,
    pub unique_identifier: Option<String>,
}

impl SmsAttachment {
    /// Decodes the base64 thumbnail preview, if one was sent. Only a
    /// preview — full-resolution attachments need a separate
    /// `request_attachment`/`attachment_file` round trip, not yet
    /// implemented.
    pub fn decode_thumbnail(&self) -> Option<Vec<u8>> {
        use base64::{Engine as _, engine::general_purpose};
        general_purpose::STANDARD
            .decode(self.encoded_thumbnail.as_ref()?)
            .ok()
    }
}

impl SmsMessages {
    pub async fn received_packet(&self, tx: mpsc::UnboundedSender<ConnectionEvent>) {
        let event = ConnectionEvent::SmsMessages(self.clone());
        let _ = tx.send(event);
    }
}

/// Outgoing request for the full-resolution file behind one MMS
/// attachment thumbnail. `unique_identifier` doubles as the filename the
/// phone will use in its `attachment_file` response — confirmed against
/// upstream kdeconnect-kde, whose response carries no part_id at all, so
/// that's the only way to correlate the two.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsRequestAttachment {
    pub part_id: i64,
    pub unique_identifier: String,
}

impl SmsRequestAttachment {
    pub fn into_packet(self) -> crate::protocol::ProtocolPacket {
        crate::protocol::ProtocolPacket::new(
            crate::protocol::PacketType::SmsRequestAttachment,
            serde_json::to_value(self).expect("failed to serialize packet body"),
        )
    }
}

/// Incoming notice that a full-resolution attachment is ready. The actual
/// bytes arrive over the separate payload-transfer socket described by
/// `PacketPayloadTransferInfo`, not in this packet's body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsAttachmentFile {
    pub filename: String,
}

impl SmsAttachmentFile {
    /// Downloads the payload into this device's attachment cache
    /// directory and returns the saved path, keyed by filename — same
    /// convention upstream uses.
    pub async fn receive(
        &self,
        device: &crate::device::Device,
        info: &crate::protocol::PacketPayloadTransferInfo,
    ) -> anyhow::Result<std::path::PathBuf> {
        let cache_dir = attachments_dir(&device.device_id.0);
        tokio::fs::create_dir_all(&cache_dir).await?;
        let dest = cache_dir.join(&self.filename);

        let mut remote_addr = device.address;
        remote_addr.set_port(info.port);

        crate::transport::receive_payload(&device.device_id, &remote_addr, &dest).await?;
        Ok(dest)
    }
}

/// Per-device cache directory for downloaded MMS attachments — a sibling
/// of the existing `sms_cache.json`/`contacts_cache.json` directory.
pub fn attachments_dir(device_id: &str) -> std::path::PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join(crate::config::CONFIG_DIR)
        .join(device_id)
        .join("attachments")
}

/// One outgoing attachment, base64-encoded and ready to embed inline in
/// the `kdeconnect.sms.request` packet body. Unlike receiving, sending an
/// attachment needs no separate payload-transfer socket — upstream
/// kdeconnect-kde's `AttachmentContainer` format embeds it directly in
/// the packet JSON (confirmed against `smsplugin.h`'s doc comment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsOutgoingAttachment {
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "base64EncodedFile")]
    pub base64_encoded_file: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

impl SmsOutgoingAttachment {
    /// Reads and base64-encodes a file from disk for sending.
    pub async fn from_path(path: &std::path::Path) -> anyhow::Result<Self> {
        use base64::{Engine as _, engine::general_purpose};

        let bytes = tokio::fs::read(path).await?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_string());

        Ok(Self {
            file_name,
            base64_encoded_file: general_purpose::STANDARD.encode(&bytes),
            mime_type: guess_mime_type(path),
        })
    }
}

/// Minimal extension-based MIME guess covering the types phones actually
/// attach to MMS (images/video/audio). A dedicated crate would be
/// overkill for this — falls back to a generic binary type for anything
/// unrecognized. Public so callers building a local preview (e.g. the
/// just-sent attachment echo) can guess the same way without duplicating
/// the match list.
pub fn guess_mime_type(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "3gp" => "video/3gpp",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "amr" => "audio/amr",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Builds the outgoing `kdeconnect.sms.request` packet, encoding any
/// attachments inline. Shared by both the varlink and D-Bus `SendSms`
/// handlers so the encoding logic isn't duplicated between them.
/// Attachments that fail to read are skipped (logged), not fatal — the
/// rest of the message still sends.
pub async fn build_send_packet(
    phone_number: &str,
    message: &str,
    attachment_paths: &[String],
) -> crate::protocol::ProtocolPacket {
    let mut attachments = Vec::with_capacity(attachment_paths.len());
    for path in attachment_paths {
        match SmsOutgoingAttachment::from_path(std::path::Path::new(path)).await {
            Ok(a) => attachments.push(a),
            Err(e) => tracing::warn!("[sms] failed to read attachment '{}': {}", path, e),
        }
    }

    let mut body = serde_json::json!({
        "sendSms": true,
        "addresses": [{ "address": phone_number }],
        "messageBody": message,
        "version": 2,
    });

    if !attachments.is_empty() {
        body["attachments"] = serde_json::to_value(attachments).unwrap_or_default();
    }

    crate::protocol::ProtocolPacket::new(crate::protocol::PacketType::SmsRequest, body)
}

/// True if any conversation in `messages_json` has a message newer than
/// what's recorded as "seen" for its thread in `last_seen` — or, for a
/// thread with no entry in `last_seen` at all (never opened in any
/// session), true if the phone itself reports any message in it unread.
///
/// Shared by both the SMS window (which also tracks read state live,
/// in-memory, while a thread is open) and the panel applet (which only
/// needs this one summary bool for its unread badge) so the grouping
/// logic isn't duplicated between the two processes.
pub fn has_unread(messages_json: &str, last_seen: &std::collections::HashMap<String, i64>) -> bool {
    let Ok(data) = serde_json::from_str::<SmsMessages>(messages_json) else {
        return false;
    };

    let mut latest_by_thread: std::collections::HashMap<i64, (i64, bool)> =
        std::collections::HashMap::new();
    for msg in &data.messages {
        let entry = latest_by_thread
            .entry(msg.thread_id)
            .or_insert((msg.date, msg.read == 0));
        if msg.date > entry.0 {
            *entry = (msg.date, msg.read == 0);
        } else if msg.date == entry.0 && msg.read == 0 {
            entry.1 = true;
        }
    }

    latest_by_thread
        .into_iter()
        .any(
            |(thread_id, (date, phone_unread))| match last_seen.get(&thread_id.to_string()) {
                Some(&seen_at) => date > seen_at,
                None => phone_unread,
            },
        )
}
