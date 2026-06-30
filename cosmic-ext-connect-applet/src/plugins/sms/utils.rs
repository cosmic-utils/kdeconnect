//! Utility functions for phone number handling, timestamp formatting, and VCard parsing.

// #[allow(dead_code)] = Placeholder for code that will be used once features are fully integrated

#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

/// Formats a Unix timestamp (in milliseconds) to a human-readable relative time.
pub fn format_timestamp(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let diff = now - (timestamp / 1000);

    match diff {
        d if d < 60 => "Just now".to_string(),
        d if d < 3600 => format!("{} min ago", d / 60),
        d if d < 86400 => format!("{} hours ago", d / 3600),
        d if d < 604800 => format!("{} days ago", d / 86400),
        _ => "More than a week ago".to_string(),
    }
}

/// Returns the current time in milliseconds since Unix epoch.
#[inline]
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Normalizes a phone number by removing all non-digit characters.
pub fn normalize_phone_number(phone: &str) -> String {
    phone.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Checks if two phone numbers match, handling various formats intelligently.
///
/// Handles:
/// - Exact matches
/// - US country code (+1) differences  
/// - Last 7 digit matches for international numbers
pub fn phone_numbers_match(phone1: &str, phone2: &str) -> bool {
    let norm1 = normalize_phone_number(phone1);
    let norm2 = normalize_phone_number(phone2);

    // Exact match
    if norm1 == norm2 {
        return true;
    }

    // Handle US country code (+1) prefix
    if norm1.len() == 10 && norm2.len() == 11 && norm2.starts_with('1') {
        return norm1 == &norm2[1..];
    }
    if norm2.len() == 10 && norm1.len() == 11 && norm1.starts_with('1') {
        return norm2 == &norm1[1..];
    }

    // Check last 7 digits for international numbers
    if norm1.len() >= 7 && norm2.len() >= 7 {
        let last7_1 = &norm1[norm1.len().saturating_sub(7)..];
        let last7_2 = &norm2[norm2.len().saturating_sub(7)..];
        if last7_1 == last7_2 {
            return true;
        }
    }

    false
}

pub fn truncate_message(s: &str, max_len: usize) -> String {
    let mut char_count = 0;
    let mut byte_end = s.len();
    for (i, _) in s.char_indices() {
        if char_count == max_len {
            byte_end = i;
            break;
        }
        char_count += 1;
    }
    if char_count < max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..byte_end])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phone_number_normalization() {
        assert_eq!(normalize_phone_number("+1 (555) 123-4567"), "15551234567");
        assert_eq!(normalize_phone_number("555.123.4567"), "5551234567");
    }

    #[test]
    fn test_phone_numbers_match() {
        assert!(phone_numbers_match("5551234567", "5551234567"));
        assert!(phone_numbers_match("5551234567", "15551234567"));
        assert!(phone_numbers_match("+1-555-123-4567", "5551234567"));
    }
}
