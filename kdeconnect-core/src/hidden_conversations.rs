//! Persists per-device "deleted" SMS conversation (thread) IDs.
//!
//! KDE Connect's SMS plugin is a read/send relay, not a sync-capable
//! mirror — the protocol has no delete packet at all (confirmed against
//! the upstream kdeconnect-kde plugin list: messages, request,
//! request_conversation(s), request_attachment, attachment_file — nothing
//! else). So "deleting" a conversation can only ever mean hiding it from
//! our own view; it will not be deleted from the phone, and new incoming
//! messages on a hidden thread are also dropped rather than reviving it.
//! This module just remembers which thread IDs to keep hiding, so a
//! delete survives a window restart or a fresh re-fetch from the phone
//! instead of the conversation reappearing.
//!
//! Stored as a JSON array of thread ID strings at:
//!   ~/.config/kdeconnect/{device_id}_hidden_conversations.json

use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::CONFIG_DIR;

fn config_path(device_id: &str) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(CONFIG_DIR)
        .join(format!("{}_hidden_conversations.json", device_id))
}

/// Load the set of hidden thread IDs for a device. Synchronous and tiny —
/// meant to be called once at window startup, before any conversations
/// have been processed, so there's no window where a previously-deleted
/// conversation could flash back on screen while an async load is still in
/// flight. Returns an empty set if no file exists yet.
pub fn load_hidden(device_id: &str) -> HashSet<String> {
    std::fs::read_to_string(config_path(device_id))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Persist the set of hidden thread IDs for a device.
pub async fn save_hidden(device_id: &str, hidden: &HashSet<String>) {
    let path = config_path(device_id);
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match serde_json::to_string(hidden) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, json).await {
                tracing::warn!(
                    "[hidden_conversations] failed to save for {}: {}",
                    device_id,
                    e
                );
            }
        }
        Err(e) => tracing::warn!("[hidden_conversations] serialize failed: {}", e),
    }
}
