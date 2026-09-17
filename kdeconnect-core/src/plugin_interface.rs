use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncWriteExt},
    sync::{RwLock, mpsc},
};
use tracing::{debug, info, warn};

use crate::{
    GLOBAL_CONFIG,
    device::Device,
    event::{ConnectionEvent, CoreEvent},
    filetransfer::{TransferAdapter, send_progress},
    plugins::{
        self,
        battery::Battery,
        clipboard::Clipboard,
        connectivity_report::ConnectivityReport,
        mpris::{Mpris, MprisRequest},
        sftp::SftpInfo,
        systemvolume::SystemVolumeRequest,
        telephony::TelephonyPacket,
    },
    protocol::{PacketPayloadTransferInfo, PacketType, ProtocolPacket},
    transport::prepare_listener_for_payload,
};

/// Maps a PacketType to the logical plugin ID used in settings.
/// Returns None for core packets (Identity, Pair) that are never gated.
pub(crate) fn packet_plugin_id(pt: &PacketType) -> Option<&'static str> {
    match pt {
        PacketType::Battery | PacketType::BatteryRequest => Some("battery"),
        PacketType::Clipboard | PacketType::ClipboardConnect => Some("clipboard"),
        PacketType::ConnectivityReport | PacketType::ConnectivityReportRequest => {
            Some("connectivity_report")
        }
        PacketType::ContactsResponseUidsTimestamps
        | PacketType::ContactsResponseVcards
        | PacketType::ContactsRequestAllUidsTimestamps
        | PacketType::ContactsRequestVcardsByUid => Some("contacts"),
        PacketType::FindMyPhoneRequest => Some("findmyphone"),
        PacketType::Mpris | PacketType::MprisRequest => Some("mpris"),
        PacketType::Notification
        | PacketType::NotificationAction
        | PacketType::NotificationReply
        | PacketType::NotificationRequest => Some("notification"),
        PacketType::Ping => Some("ping"),
        PacketType::RunCommand | PacketType::RunCommandRequest => Some("runcommand"),
        PacketType::ShareRequest | PacketType::ShareRequestUpdate => Some("share"),
        PacketType::SmsMessages
        | PacketType::SmsRequest
        | PacketType::SmsRequestConversations
        | PacketType::SmsRequestConversation
        | PacketType::SmsAttachmentFile
        | PacketType::SmsRequestAttachment => Some("sms"),
        PacketType::SystemVolume | PacketType::SystemVolumeRequest => Some("systemvolume"),
        PacketType::Telephony | PacketType::TelephonyRequestMute => Some("telephony"),
        // Core / unmanaged packets are never gated
        PacketType::Identity
        | PacketType::Pair
        | PacketType::Lock
        | PacketType::LockRequest
        | PacketType::MousePadEcho
        | PacketType::MousePadKeyboardState
        | PacketType::MousePadRequest
        | PacketType::Presenter
        | PacketType::Sftp
        | PacketType::SftpRequest
        | PacketType::Unknown(_) => None,
    }
}

#[derive(Clone)]
pub struct PluginRegistry {
    /// device_id.0 → set of disabled plugin IDs
    disabled: Arc<RwLock<HashMap<String, HashSet<String>>>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            disabled: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Replace the disabled set for a device (called on connect and on toggle).
    pub async fn set_device_disabled(&self, device_id: &str, disabled: HashSet<String>) {
        self.disabled
            .write()
            .await
            .insert(device_id.to_string(), disabled);
    }

    /// Returns true if the plugin is currently enabled for this device.
    pub async fn is_plugin_enabled(&self, device_id: &str, plugin_id: &str) -> bool {
        let guard = self.disabled.read().await;
        guard
            .get(device_id)
            .map(|set| !set.contains(plugin_id))
            .unwrap_or(true)
    }

    pub async fn dispatch(
        &self,
        device: Device,
        packet: ProtocolPacket,
        core_tx: mpsc::UnboundedSender<CoreEvent>,
        tx: mpsc::UnboundedSender<ConnectionEvent>,
        mpris_tx: mpsc::UnboundedSender<ConnectionEvent>,
    ) {
        // Gate on plugin enabled state before doing any work.
        if let Some(plugin_id) = packet_plugin_id(&packet.packet_type) {
            if !self.is_plugin_enabled(&device.device_id.0, plugin_id).await {
                debug!(
                    "[plugin_registry] packet {:?} skipped — plugin '{}' disabled for {}",
                    packet.packet_type, plugin_id, device.device_id
                );
                return;
            }
        }

        let body = packet.body.clone();
        info!("[dispatch] packet type: {:?}", packet.packet_type);
        let connection_tx = tx;
        let mpris_connection_tx = mpris_tx;
        let payload_info = packet.payload_transfer_info;

        match packet.packet_type {
            PacketType::Identity => {
                debug!("Skipping identity packet");
            }
            PacketType::Pair => {
                debug!("Skipping pair packet");
            }
            PacketType::Battery => {
                if let Ok(battery) = serde_json::from_value::<Battery>(body) {
                    battery.received_packet(connection_tx).await;
                }
            }
            PacketType::BatteryRequest => {
                debug!("BatteryRequest received — not implemented, ignoring");
            }
            PacketType::SmsAttachmentFile => {
                if let Ok(attachment_file) =
                    serde_json::from_value::<plugins::sms::SmsAttachmentFile>(body)
                    && let Some(payload_info) = payload_info
                {
                    let connection_tx = connection_tx.clone();
                    // Spawn so the event loop is not blocked while the
                    // payload (a photo or video) downloads.
                    tokio::spawn(async move {
                        let filename = attachment_file.filename.clone();
                        match attachment_file.receive(&device, &payload_info).await {
                            Ok(path) => {
                                let _ = connection_tx.send(ConnectionEvent::SmsAttachmentReceived(
                                    (device.device_id.clone(), filename, path),
                                ));
                            }
                            Err(e) => warn!("[sms] attachment receive failed: {}", e),
                        }
                    });
                }
            }
            PacketType::SmsMessages => {
                debug!("Received SmsMessages packet");
                if let Ok(sms_messages) =
                    serde_json::from_value::<plugins::sms::SmsMessages>(body.clone())
                {
                    info!(
                        "Received SMS messages packet with {} messages",
                        sms_messages.messages.len()
                    );
                    debug!(
                        "Successfully parsed {} SMS messages",
                        sms_messages.messages.len()
                    );
                    sms_messages.received_packet(connection_tx).await;
                } else {
                    warn!("Failed to parse SMS messages packet: {:?}", body);
                }
            }
            PacketType::ContactsResponseUidsTimestamps => {
                debug!("Received ContactsResponseUidsTimestamps");
                if let Some(uids_val) = body.get("uids").and_then(|v| v.as_array()) {
                    let uids: Vec<String> = uids_val
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    if !uids.is_empty() {
                        debug!("Requesting vcards for {} UIDs", uids.len());
                        let packet = ProtocolPacket::new(
                            PacketType::ContactsRequestVcardsByUid,
                            serde_json::json!({ "uids": uids }),
                        );
                        let _ = core_tx.send(CoreEvent::SendPacket {
                            device: device.device_id.clone(),
                            packet,
                        });
                    }
                }
            }
            PacketType::ContactsResponseVcards => {
                debug!("Received ContactsResponseVcards");
                let mut contacts: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                let mut photos: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                let mut vcard_count = 0usize;
                let mut remote_photo_count = 0usize;
                if let Some(uids_val) = body.get("uids").and_then(|v| v.as_array()) {
                    for uid_val in uids_val {
                        if let Some(uid) = uid_val.as_str() {
                            if let Some(vcard_str) = body.get(uid).and_then(|v| v.as_str()) {
                                vcard_count += 1;
                                let (name_opt, phones, photo) = parse_vcard(vcard_str);
                                if let Some(name) = name_opt {
                                    for phone in &phones {
                                        contacts.insert(phone.clone(), name.clone());
                                    }
                                }
                                match photo {
                                    VcardPhoto::Inline(b64) => {
                                        for phone in &phones {
                                            photos.insert(phone.clone(), b64.clone());
                                        }
                                    }
                                    VcardPhoto::Remote(url) => {
                                        remote_photo_count += 1;
                                        debug!(
                                            "[contacts] photo referenced by remote URL, not fetched: {}",
                                            url
                                        );
                                    }
                                    VcardPhoto::Unrecognized | VcardPhoto::None => {}
                                }
                            }
                        }
                    }
                }
                debug!("Parsed {} phone->name contact entries", contacts.len());
                debug!(
                    "[contacts] {} vcards: {} photos decoded inline, {} referenced remote URLs (not fetched)",
                    vcard_count,
                    photos.len(),
                    remote_photo_count
                );
                if !contacts.is_empty() {
                    let _ = connection_tx.send(ConnectionEvent::ContactsReceived(contacts));
                }
                if !photos.is_empty() {
                    debug!("Parsed {} contact photos", photos.len());
                    let _ = connection_tx.send(ConnectionEvent::ContactPhotosReceived(photos));
                }
            }
            PacketType::Clipboard => {
                if let Ok(clipboard) = serde_json::from_value::<Clipboard>(body) {
                    clipboard.received_packet(connection_tx).await;
                }
            }
            PacketType::ConnectivityReport => {
                if let Ok(connectivity_rep) = serde_json::from_value::<ConnectivityReport>(body) {
                    connectivity_rep.received_packet(connection_tx).await;
                }
            }
            PacketType::ClipboardConnect => {
                if let Ok(clipboard) = serde_json::from_value::<Clipboard>(body) {
                    // The service owns the local clipboard timestamp and makes
                    // the same new-enough decision as upstream KDE Connect.
                    clipboard.received_connect_packet(connection_tx).await;
                }
            }
            PacketType::MousePadKeyboardState => {
                if let Ok(keyboard_state) =
                    serde_json::from_value::<plugins::mousepad::KeyboardState>(body)
                {
                    debug!("{:?}", keyboard_state);
                }
            }
            PacketType::Mpris => {
                if let Ok(mpris_packet) = serde_json::from_value::<Mpris>(body) {
                    info!("Received MPRIS packet: {:?}", mpris_packet);

                    if let Mpris::TransferringArt {
                        ref player,
                        ref album_art_url,
                        transferring_album_art: true,
                    } = mpris_packet
                        && let Some(info) = payload_info
                    {
                        let device = device.clone();
                        let player = player.clone();
                        let album_art_url = album_art_url.clone();
                        let art_tx = mpris_connection_tx.clone();
                        tokio::spawn(async move {
                            match plugins::mpris::download_album_art(
                                &device,
                                &player,
                                &album_art_url,
                                &info,
                            )
                            .await
                            {
                                Ok(local_path) => {
                                    let ready = Mpris::TransferringArt {
                                        player,
                                        album_art_url: local_path,
                                        transferring_album_art: false,
                                    };
                                    let _ = art_tx.send(ConnectionEvent::Mpris((
                                        device.device_id.clone(),
                                        ready,
                                    )));
                                }
                                Err(e) => warn!("[mpris] album art download failed: {}", e),
                            }
                        });
                    }

                    let mpris_event =
                        ConnectionEvent::Mpris((device.device_id.clone(), mpris_packet));
                    let _ = connection_tx.send(mpris_event.clone());
                    let _ = mpris_connection_tx.send(mpris_event);
                }
            }
            PacketType::MprisRequest => {
                if let Ok(mpris_request) = serde_json::from_value::<MprisRequest>(body) {
                    mpris_request.received_packet(&device, core_tx).await;
                }
            }
            PacketType::Notification => {
                debug!("Received notification packet");
                info!("Notification body: {:?}", body);
                if let Ok(notification) =
                    serde_json::from_value::<plugins::notification::Notification>(body)
                {
                    notification.received_packet(&device, core_tx).await;
                }
            }
            PacketType::Ping => {
                if let Ok(ping) = serde_json::from_value::<plugins::ping::Ping>(body) {
                    ping.received_packet(&device, core_tx).await;
                }
            }
            PacketType::RunCommand => {
                if let Ok(run_command) =
                    serde_json::from_value::<plugins::run_command::RunCommand>(body)
                {
                    run_command.received_packet(&device, core_tx).await;
                }
            }
            PacketType::RunCommandRequest => {
                if let Ok(run_command_request) =
                    serde_json::from_value::<plugins::run_command::RunCommandRequest>(body)
                {
                    run_command_request.received_packet(&device, core_tx).await;
                }
            }
            PacketType::ShareRequest => {
                if let Ok(share_request) =
                    serde_json::from_value::<plugins::share::ShareRequest>(body)
                    && let Some(payload_info) = payload_info
                {
                    // Spawn so the event loop is not blocked during the
                    // notification dialog wait + network payload download.
                    tokio::spawn(async move {
                        if let Err(e) = share_request.receive_share(&device, &payload_info).await {
                            warn!("[share] receive_share failed: {}", e);
                        }
                    });
                }
            }
            PacketType::SystemVolumeRequest => {
                if let Ok(req) = serde_json::from_value::<SystemVolumeRequest>(body) {
                    req.handle(&device).await;
                }
            }
            PacketType::Sftp => {
                if let Ok(info) = serde_json::from_value::<SftpInfo>(body) {
                    let device_id = device.device_id.clone();
                    let device_name = device.name.clone();
                    let connection_tx = connection_tx.clone();
                    tokio::spawn(async move {
                        match info.browse(&device_id.0, &device_name).await {
                            Ok(_) => {
                                let _ = connection_tx.send(ConnectionEvent::SftpMountStateChanged(
                                    (device_id, true),
                                ));
                            }
                            Err(e) => {
                                warn!("[sftp] browse failed: {}", e);
                                let _ = connection_tx.send(ConnectionEvent::SftpBrowseFailed((
                                    device_id,
                                    e.to_string(),
                                )));
                            }
                        }
                    });
                } else {
                    warn!("[sftp] failed to parse kdeconnect.sftp packet");
                }
            }
            PacketType::Telephony => {
                if let Ok(pkt) = serde_json::from_value::<TelephonyPacket>(body) {
                    pkt.received_packet(&device, core_tx).await;
                }
            }
            PacketType::TelephonyRequestMute => {
                debug!("TelephonyRequestMute received — no action needed on desktop");
            }
            _ => {
                debug!(
                    "No plugin found to handle packet type: {:?}",
                    packet.packet_type
                );
            }
        }
    }

    /// Send a packet that carries a binary payload (file / album art).
    ///
    /// The packet is enqueued immediately so the phone knows which port to
    /// connect to. The actual TLS accept + byte copy is spawned as a
    /// background task so the event loop is never blocked.
    pub async fn send_payload(
        &self,
        packet: ProtocolPacket,
        device_writer: &mpsc::UnboundedSender<ProtocolPacket>,
        mut payload: TransferAdapter<impl AsyncRead + Sync + Send + Unpin + 'static>,
        payload_size: u64,
    ) {
        info!("preparing payload transfer");

        let free_listener = match prepare_listener_for_payload().await {
            Ok(l) => l,
            Err(e) => {
                warn!("cannot find free port: {}", e);
                return;
            }
        };

        let addr = match free_listener.local_addr() {
            Ok(a) => a,
            Err(e) => {
                warn!("cannot get local addr for payload listener: {}", e);
                return;
            }
        };

        debug!("payload listener bound on {}", addr);
        let payload_transfer_info = Some(PacketPayloadTransferInfo { port: addr.port() });
        let body = packet.body.clone();

        // Enqueue the packet with the port info NOW — the phone needs this to
        // know where to connect. This is non-blocking (channel send).
        match packet.packet_type {
            PacketType::Mpris => {
                if let Ok(mpris) = serde_json::from_value::<plugins::mpris::Mpris>(body) {
                    debug!("got mpris packet, sending info.");
                    let _ = mpris
                        .send_art(device_writer, payload_size, payload_transfer_info)
                        .await;
                }
            }
            PacketType::ShareRequest => {
                if let Ok(share_request) =
                    serde_json::from_value::<plugins::share::ShareRequest>(body)
                {
                    debug!("got share request packet, sending info.");
                    let _ = share_request
                        .send_file(device_writer, payload_size, payload_transfer_info)
                        .await;
                }
            }
            _ => {
                warn!(
                    "[payload] No plugin found to handle packet type: {:?}",
                    packet.packet_type
                );
                return;
            }
        }

        // Spawn the accept + copy so the event loop stays responsive for the
        // entire duration of the file transfer.
        let server_config = GLOBAL_CONFIG.get().unwrap().key_store.server_config.clone();
        tokio::spawn(async move {
            let (incoming, peer_addr) = match free_listener.accept().await {
                Ok(res) => res,
                Err(e) => {
                    warn!("[payload] accepting connection failed: {}", e);
                    return;
                }
            };
            debug!("[payload] incoming connection from {}", peer_addr);

            let mut stream = match tokio_rustls::TlsAcceptor::from(server_config)
                .accept(incoming)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    warn!("[payload] TLS handshake failed: {}", e);
                    return;
                }
            };

            debug!("[payload] TLS accepted, copying payload");
            let _ = tokio::io::copy(&mut payload, &mut stream).await;
            let _ = stream.flush().await;
            let _ = stream.shutdown().await;
            // Guarantee a final 100% progress event so the UI always clears
            // regardless of file size or interval timing.
            send_progress(100, payload.notify_tx.clone());
            info!("[payload] successfully sent payload to {}", peer_addr);
        });
    }

    pub async fn send(
        &self,
        device: Device,
        packet: ProtocolPacket,
        core_tx: mpsc::UnboundedSender<CoreEvent>,
    ) {
        let body = packet.body.clone();
        let core_event = core_tx;

        match packet.packet_type {
            PacketType::Ping => {
                if let Ok(ping) = serde_json::from_value::<plugins::ping::Ping>(body) {
                    ping.send_packet(&device, core_event).await;
                }
            }
            PacketType::MprisRequest => {
                if let Ok(mpris_request) = serde_json::from_value::<MprisRequest>(body) {
                    mpris_request.send_packet(&device, core_event).await;
                }
            }
            _ => {
                warn!(
                    "No plugin found to handle packet type: {:?}",
                    packet.packet_type
                );
            }
        }
    }
}

fn decode_quoted_printable(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'='
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            if let Ok(s) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(byte) = u8::from_str_radix(s, 16) {
                    out.push(byte);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unfold_vcard_lines(content: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            // vCard 3.0/4.0 folded line
            current.push_str(line.trim_start());
        } else if current.ends_with('=') {
            // vCard 2.1 QP soft line break
            current.pop();
            current.push_str(line.trim_start());
        } else {
            if !current.is_empty() {
                lines.push(current.clone());
            }
            current = line.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Extracts a base64 photo payload from a vCard `PHOTO` property,
/// handling the formats real-world exports actually use:
/// - vCard 2.1/3.0: `PHOTO;ENCODING=BASE64;TYPE=JPEG:<data>`, or the
///   bare-token shorthand `PHOTO;BASE64;JPEG:<data>` some exporters
///   write instead of the full `ENCODING=` form
/// - vCard 4.0: `PHOTO:data:image/jpeg;base64,<data>` — the encoding
///   moved into the value itself as a data URI, no `ENCODING` param at
///   all, which the original implementation didn't account for
///
/// Remote `PHOTO;VALUE=uri:http://...` photos are detected but
/// deliberately not fetched. This is how Google-synced contacts
/// reference their profile photo — confirmed this isn't something
/// official KDE Connect resolves either: its contact sync writes plain
/// vcards consumed by KPeople, with no KAddressBook/Akonadi involved.
/// Richer Google photos on a real Plasma desktop come from a separate,
/// unrelated integration (`kaccounts-mobile`'s direct Google People API
/// sync) that fills the same KPeople merge point — not from anything
/// KDE Connect's vcard transfer does differently. Building an equivalent
/// would mean a full Google OAuth/People-API integration, well beyond
/// this feature; until then, `VcardPhoto::Remote` exists so the caller
/// can log a clear signal distinguishing "phone never sent a photo" from
/// "phone sent a link we don't follow".
fn extract_vcard_photo(prop_upper: &str, value: &str) -> VcardPhoto {
    if let Some(idx) = value.to_uppercase().find("BASE64,") {
        let data = &value[idx + "BASE64,".len()..];
        let stripped: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        if !stripped.is_empty() {
            return VcardPhoto::Inline(stripped);
        }
    }

    let has_base64_param = prop_upper
        .split(';')
        .skip(1)
        .any(|param| matches!(param, "BASE64" | "B" | "ENCODING=BASE64" | "ENCODING=B"));

    if has_base64_param {
        let stripped: String = value.chars().filter(|c| !c.is_whitespace()).collect();
        if !stripped.is_empty() {
            return VcardPhoto::Inline(stripped);
        }
    }

    let value_trim = value.trim();
    if value_trim.starts_with("http://") || value_trim.starts_with("https://") {
        return VcardPhoto::Remote(value_trim.to_string());
    }

    if !value_trim.is_empty() {
        debug!(
            "[contacts] PHOTO present but in an unrecognized format (params: {}, value starts: {:.30})",
            prop_upper, value_trim
        );
        return VcardPhoto::Unrecognized;
    }

    VcardPhoto::None
}

enum VcardPhoto {
    Inline(String),
    Remote(String),
    Unrecognized,
    None,
}

fn parse_vcard(content: &str) -> (Option<String>, Vec<String>, VcardPhoto) {
    let mut name: Option<String> = None;
    let mut phones: Vec<String> = Vec::new();
    let mut photo = VcardPhoto::None;

    for line in unfold_vcard_lines(content) {
        let line = line.trim().to_string();
        let (prop_part, value_raw) = match line.find(':') {
            Some(pos) => (&line[..pos], &line[pos + 1..]),
            None => continue,
        };
        let prop_upper = prop_part.to_uppercase();
        let is_qp = prop_upper.contains("ENCODING=QUOTED-PRINTABLE");
        let value = if is_qp {
            decode_quoted_printable(value_raw)
        } else {
            value_raw.trim().to_string()
        };
        let prop_name = prop_upper.split(';').next().unwrap_or("").trim();

        if prop_name == "FN" {
            name = Some(value.trim().to_string());
        } else if name.is_none() && prop_name == "N" {
            let parts: Vec<&str> = value.split(';').collect();
            if parts.len() >= 2 {
                let full = format!("{} {}", parts[1].trim(), parts[0].trim())
                    .trim()
                    .to_string();
                if !full.is_empty() {
                    name = Some(full);
                }
            }
        } else if prop_name == "TEL" {
            let phone = value.trim().to_string();
            if !phone.is_empty() {
                phones.push(phone);
            }
        } else if prop_name == "PHOTO" && matches!(photo, VcardPhoto::None) {
            // Keep the first PHOTO line's result — a vcard with multiple
            // PHOTO entries (e.g. a remote ref alongside a cached inline
            // copy) shouldn't let a later, worse one overwrite a usable
            // earlier one, or vice versa silently.
            photo = extract_vcard_photo(&prop_upper, &value);
        }
    }

    (name, phones, photo)
}
