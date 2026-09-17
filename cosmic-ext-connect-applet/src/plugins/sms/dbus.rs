use anyhow::Result;
use kdeconnect_dbus_client::KdeConnectClient;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::models::{Conversation, Message, MessageAttachment};

lazy_static::lazy_static! {
    static ref SMS_CLIENT: Arc<Mutex<Option<Arc<KdeConnectClient>>>> = Arc::new(Mutex::new(None));
}

/// Tries a varlink call first, falling back to `None` if the socket isn't
/// reachable (caller then uses the D-Bus client). This window is a separate
/// process from the panel applet, so it keeps its own lightweight attempt
/// rather than sharing `backend.rs`'s cached address — these are infrequent,
/// user-triggered actions, not a hot polling path, so skipping the cache is
/// the simpler tradeoff.
async fn via_varlink<F, Fut, T>(f: F) -> Option<T>
where
    F: FnOnce(kdeconnect_varlink::iface::VarlinkClient) -> Fut,
    Fut: std::future::Future<Output = Result<T, kdeconnect_varlink::Error>>,
{
    let addr = kdeconnect_varlink::socket_address();
    match varlink::AsyncConnection::with_address(&addr).await {
        Ok(conn) => match f(kdeconnect_varlink::iface::VarlinkClient::new(conn)).await {
            Ok(v) => Some(v),
            Err(e) => {
                warn!("varlink call failed, falling back to D-Bus: {:?}", e);
                None
            }
        },
        Err(_) => None,
    }
}

pub async fn initialize() -> Result<()> {
    debug!("SMS D-Bus initialize()");
    let client = KdeConnectClient::new().await?;
    *SMS_CLIENT.lock().await = Some(Arc::new(client));
    info!("SMS D-Bus client initialized");
    Ok(())
}

/// Wait up to 10s for the client to be ready, then return it.
pub async fn get_client() -> Option<Arc<KdeConnectClient>> {
    for i in 0..100 {
        {
            let guard = SMS_CLIENT.lock().await;
            if let Some(c) = guard.as_ref() {
                debug!("SMS D-Bus client ready after {}*100ms", i);
                return Some(c.clone());
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    warn!("SMS D-Bus client initialization timed out");
    None
}

pub async fn fetch_conversations(device_id: &str) {
    debug!("fetch_conversations device={}", device_id);
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    if via_varlink(|c| async move { c.request_conversations(id).call().await })
        .await
        .is_some()
    {
        debug!("request_conversations sent via varlink");
        return;
    }
    let Some(client) = get_client().await else {
        return;
    };
    match client.request_conversations(device_id).await {
        Ok(_) => debug!("request_conversations sent"),
        Err(e) => error!("request_conversations failed: {:?}", e),
    }
}

pub async fn request_conversation_messages(device_id: &str, thread_id: &str) {
    debug!(
        "request_conversation device={} thread={}",
        device_id, thread_id
    );
    let tid = thread_id.parse::<i64>().unwrap_or(0);
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    if via_varlink(|c| async move { c.request_conversation(id, tid).call().await })
        .await
        .is_some()
    {
        debug!("request_conversation sent via varlink");
        return;
    }
    let Some(client) = get_client().await else {
        return;
    };
    match client.request_conversation(device_id, tid).await {
        Ok(_) => debug!("request_conversation sent"),
        Err(e) => error!("request_conversation failed: {:?}", e),
    }
}

pub async fn send_sms(
    device_id: &str,
    phone_number: &str,
    message: &str,
    attachments: Vec<String>,
) {
    debug!(
        "send_sms to={} device={} attachments={}",
        phone_number,
        device_id,
        attachments.len()
    );
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    let phone = phone_number.to_string();
    let msg = message.to_string();
    let files = attachments.clone();
    if via_varlink(|c| async move { c.send_sms(id, phone, msg, files).call().await })
        .await
        .is_some()
    {
        debug!("send_sms sent via varlink");
        return;
    }
    let Some(client) = get_client().await else {
        return;
    };
    match client
        .send_sms(device_id, phone_number, message, attachments)
        .await
    {
        Ok(_) => debug!("send_sms OK"),
        Err(e) => error!("send_sms failed: {:?}", e),
    }
}

/// Requests the full-resolution file for one MMS attachment. Fire-and-forget
/// — the result arrives later as a `SmsAttachmentReceived` D-Bus signal,
/// same as every other async SMS event.
pub async fn request_sms_attachment(device_id: &str, part_id: i64, unique_identifier: &str) {
    debug!(
        "request_sms_attachment device={} part_id={}",
        device_id, part_id
    );
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    let uid = unique_identifier.to_string();
    if via_varlink(|c| async move { c.request_sms_attachment(id, part_id, uid).call().await })
        .await
        .is_some()
    {
        debug!("request_sms_attachment sent via varlink");
        return;
    }
    let Some(client) = get_client().await else {
        return;
    };
    match client
        .request_sms_attachment(device_id, part_id, unique_identifier)
        .await
    {
        Ok(_) => debug!("request_sms_attachment sent"),
        Err(e) => error!("request_sms_attachment failed: {:?}", e),
    }
}

pub async fn fetch_contacts(device_id: &str) {
    debug!("fetch_contacts device={}", device_id);
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    if via_varlink(|c| async move { c.request_contacts(id).call().await })
        .await
        .is_some()
    {
        debug!("request_contacts sent via varlink");
        return;
    }
    let Some(client) = get_client().await else {
        return;
    };
    match client.request_contacts(device_id).await {
        Ok(_) => debug!("request_contacts sent"),
        Err(e) => error!("request_contacts failed: {:?}", e),
    }
}

pub async fn get_cached_contacts(device_id: &str) -> std::collections::HashMap<String, String> {
    debug!("get_cached_contacts device={}", device_id);
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    if let Some(reply) =
        via_varlink(|c| async move { c.get_cached_contacts(id).call().await }).await
    {
        return serde_json::from_str(&reply.json).unwrap_or_default();
    }

    let Some(client) = get_client().await else {
        return std::collections::HashMap::new();
    };
    // The D-Bus interface returns a JSON string; deserialize it here.
    match client.get_cached_contacts(device_id).await {
        Ok(contacts_json) => {
            let map: std::collections::HashMap<String, String> =
                serde_json::from_str(&contacts_json).unwrap_or_default();
            debug!("got {} cached contacts", map.len());
            map
        }
        Err(e) => {
            error!("get_cached_contacts failed: {:?}", e);
            std::collections::HashMap::new()
        }
    }
}

/// Fetches cached contact photos and decodes them once here (phone →
/// raw image bytes), same as SMS thumbnails — so views.rs never has to
/// decode base64 on every render.
pub async fn get_cached_contact_photos(
    device_id: &str,
) -> std::collections::HashMap<String, Vec<u8>> {
    debug!("get_cached_contact_photos device={}", device_id);
    use kdeconnect_varlink::iface::VarlinkClientInterface;

    let decode = |json: &str| -> std::collections::HashMap<String, Vec<u8>> {
        let map: std::collections::HashMap<String, String> =
            serde_json::from_str(json).unwrap_or_default();
        map.into_iter()
            .filter_map(|(phone, b64)| {
                kdeconnect_core::contacts::decode_photo(&b64).map(|bytes| (phone, bytes))
            })
            .collect()
    };

    let id = device_id.to_string();
    if let Some(reply) =
        via_varlink(|c| async move { c.get_cached_contact_photos(id).call().await }).await
    {
        return decode(&reply.json);
    }

    let Some(client) = get_client().await else {
        return std::collections::HashMap::new();
    };
    match client.get_cached_contact_photos(device_id).await {
        Ok(photos_json) => {
            let map = decode(&photos_json);
            debug!("got {} cached contact photos", map.len());
            map
        }
        Err(e) => {
            error!("get_cached_contact_photos failed: {:?}", e);
            std::collections::HashMap::new()
        }
    }
}

pub async fn get_cached_sms(device_id: &str) -> Option<String> {
    debug!("get_cached_sms device={}", device_id);
    use kdeconnect_varlink::iface::VarlinkClientInterface;
    let id = device_id.to_string();
    if let Some(reply) = via_varlink(|c| async move { c.get_cached_sms(id).call().await }).await {
        return if reply.json.is_empty() {
            None
        } else {
            Some(reply.json)
        };
    }

    let Some(client) = get_client().await else {
        return None;
    };
    match client.get_cached_sms(device_id).await {
        Ok(json) if !json.is_empty() => {
            debug!("got cached SMS ({} bytes)", json.len());
            Some(json)
        }
        Ok(_) => {
            debug!("no SMS cache found");
            None
        }
        Err(e) => {
            error!("get_cached_sms failed: {:?}", e);
            None
        }
    }
}

pub fn parse_sms_messages(messages_json: &str) -> (Vec<Message>, Vec<Conversation>) {
    use std::collections::HashMap;

    let raw: serde_json::Value = match serde_json::from_str(messages_json) {
        Ok(v) => v,
        Err(e) => {
            error!("SMS JSON parse failed: {:?}", e);
            return (vec![], vec![]);
        }
    };

    let raw_messages = raw
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();

    // Deserialize message-by-message rather than the whole array at once.
    // A single unusual entry — e.g. a read receipt, reaction, or other
    // event row a phone's RCS client can write into the same message
    // store KDE Connect's SmsHelper reads from — shouldn't take down the
    // rest of an otherwise-normal conversation. Skip and log, don't fail.
    let sms_messages: Vec<kdeconnect_core::plugins::sms::SmsMessage> = raw_messages
        .into_iter()
        .filter_map(|item| match serde_json::from_value(item) {
            Ok(msg) => Some(msg),
            Err(e) => {
                debug!("Skipping one SMS entry with unexpected shape: {:?}", e);
                None
            }
        })
        .collect();

    debug!("parsed {} SMS messages", sms_messages.len());

    let messages: Vec<Message> = sms_messages
        .iter()
        .map(|msg| {
            let address = msg
                .addresses
                .first()
                .map(|a| a.address.clone())
                .unwrap_or_default();
            Message {
                id: msg.id.to_string(),
                thread_id: msg.thread_id.to_string(),
                address,
                body: msg.body.clone(),
                date: msg.date,
                attachments: msg
                    .attachments
                    .iter()
                    .map(|a| MessageAttachment {
                        part_id: a.part_id,
                        unique_identifier: a.unique_identifier.clone(),
                        mime_type: a.mime_type.clone(),
                        thumbnail: a.decode_thumbnail(),
                        full_path: None,
                    })
                    .collect(),
                type_: msg.message_type,
                read: msg.read == 1,
            }
        })
        .collect();

    let mut groups: HashMap<String, Vec<&Message>> = HashMap::new();
    for msg in &messages {
        groups.entry(msg.thread_id.clone()).or_default().push(msg);
    }

    let conversations: Vec<Conversation> = groups
        .into_iter()
        .map(|(thread_id, mut msgs)| {
            msgs.sort_by(|a, b| b.date.cmp(&a.date));
            let last = msgs.first().unwrap();
            Conversation {
                thread_id,
                phone_number: last.address.clone(),
                last_message: last.body.clone(),
                timestamp: last.date,
                unread: msgs.iter().any(|m| !m.read),
                contact_name: String::new(),
            }
        })
        .collect();

    (messages, conversations)
}
