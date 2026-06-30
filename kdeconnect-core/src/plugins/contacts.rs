use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::info;

use crate::event::ConnectionEvent;

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

pub fn parse_vcard(content: &str) -> (Option<String>, Vec<String>) {
    let mut name: Option<String> = None;
    let mut phones: Vec<String> = Vec::new();

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
        }
    }

    (name, phones)
}

/// Parses a `kdeconnect.contacts.response_uids_timestamps` packet body
/// and returns the list of UIDs.
pub fn parse_uids_timestamps(body: &Value) -> Vec<String> {
    let uids: Vec<String> = match body.get("uids").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        None => return vec![],
    };
    info!("[contacts] received {} UIDs from phone", uids.len());
    uids
}

/// Parses a `kdeconnect.contacts.response_vcards` packet body and emits
/// a `ConnectionEvent::ContactsReceived` with a phone → name map.
pub fn parse_vcards_and_emit(body: &Value, tx: &mpsc::UnboundedSender<ConnectionEvent>) {
    let uids: Vec<String> = match body.get("uids").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        None => {
            tracing::warn!("[contacts] response_vcards missing 'uids' field");
            return;
        }
    };

    let mut contacts: HashMap<String, String> = HashMap::new();

    for uid in &uids {
        if let Some(vcard_str) = body.get(uid).and_then(|v| v.as_str()) {
            let (name, phones) = parse_vcard(vcard_str);
            if let Some(name) = name {
                for phone in phones {
                    contacts.insert(phone, name.clone());
                }
            }
        }
    }

    info!(
        "[contacts] parsed {} contacts from {} vCards",
        contacts.len(),
        uids.len()
    );
    let _ = tx.send(ConnectionEvent::ContactsReceived(contacts));
}

/// Builds the body for a `kdeconnect.contacts.request_vcards_by_uid` packet.
pub fn build_vcards_request(uids: Vec<String>) -> Value {
    serde_json::json!({ "uids": uids })
}
