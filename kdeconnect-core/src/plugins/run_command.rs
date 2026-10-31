use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::CONFIG_DIR;
use crate::protocol::{PacketType, ProtocolPacket};

// ---------------------------------------------------------------------------
// Local command store
// ---------------------------------------------------------------------------

/// A command defined on this desktop that the phone can trigger.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LocalCommand {
    /// UUID used as the map key in the KDE Connect packet.
    pub id: String,
    pub name: String,
    pub command: String,
}

// dirs::config_dir() already reads XDG_CONFIG_HOME (sandboxed under Flatpak)
// and falls back to ~/.config outside it.
fn commands_config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(CONFIG_DIR)
        .join("runcommand.json")
}

fn load_local_commands() -> Vec<LocalCommand> {
    match std::fs::read_to_string(commands_config_path()) {
        Ok(json_str) => serde_json::from_str::<Vec<LocalCommand>>(&json_str).unwrap_or_default(),
        Err(_) => vec![],
    }
}

fn save_local_commands(commands: &[LocalCommand]) {
    let path = commands_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&commands) {
        let _ = std::fs::write(&path, json);
    }
}

// ---------------------------------------------------------------------------
// Protocol structs
// ---------------------------------------------------------------------------

/// Incoming `kdeconnect.runcommand` from the phone (rare — phone also has
/// its own commands, but the primary direction is desktop -> phone).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub struct RunCommand {
    #[serde(rename = "commandList")]
    pub command_list: String,
    #[serde(rename = "canAddCommand", default)]
    pub can_add_command: bool,
}

/// Incoming `kdeconnect.runcommand.request` from the phone:
///   - `key` -> execute the named command on this desktop
///   - `requestCommandList` -> send our command list back
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RunCommandRequest {
    pub key: Option<String>,
    #[serde(rename = "requestCommandList")]
    pub request_command_list: Option<bool>,
}

// ---------------------------------------------------------------------------
// Packet handlers
// ---------------------------------------------------------------------------

impl RunCommand {
    /// Phone sent us an updated command list — save any new commands to disk.
    pub async fn received_packet(
        &self,
        device: &crate::device::Device,
        core_tx: mpsc::UnboundedSender<crate::event::CoreEvent>,
    ) {
        // Parse the commandList JSON string into individual commands.
        let incoming: std::collections::HashMap<String, serde_json::Value> =
            match serde_json::from_str(&self.command_list) {
                Ok(map) => map,
                Err(e) => {
                    warn!(
                        "[runcommand] failed to parse commandList from {}: {}",
                        device.device_id, e
                    );
                    return;
                }
            };

        if incoming.is_empty() {
            return;
        }

        // Merge into existing commands — add any that aren't already present by id.
        let mut commands = load_local_commands();
        let mut changed = false;
        for (key, val) in &incoming {
            let name = val["name"].as_str().unwrap_or("").to_string();
            let command = val["command"].as_str().unwrap_or("").to_string();
            if name.is_empty() || command.is_empty() {
                continue;
            }
            if !commands.iter().any(|c| c.id == *key) {
                commands.push(LocalCommand {
                    id: key.clone(),
                    name,
                    command,
                });
                changed = true;
            }
        }

        if changed {
            save_local_commands(&commands);
            info!(
                "[runcommand] saved {} new command(s) from {}",
                incoming.len(),
                device.device_id
            );
            // Re-send updated list back to phone to confirm.
            send_command_list(&device.device_id, core_tx).await;
        }
    }
}

impl RunCommandRequest {
    pub async fn received_packet(
        &self,
        device: &crate::device::Device,
        core_tx: mpsc::UnboundedSender<crate::event::CoreEvent>,
    ) {
        if let Some(key) = &self.key {
            // Phone is asking us to execute a local command by its UUID key.
            let commands = load_local_commands();
            if let Some(cmd) = commands.iter().find(|c| c.id == *key) {
                info!("[runcommand] executing '{}': {}", cmd.name, cmd.command);
                let result = if std::env::var("FLATPAK_ID").is_ok() {
                    std::process::Command::new("flatpak-spawn")
                        .arg("--host")
                        .arg("sh")
                        .arg("-c")
                        .arg(&cmd.command)
                        .spawn()
                } else {
                    std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmd.command)
                        .spawn()
                };
                if let Err(e) = result {
                    warn!("[runcommand] failed to spawn '{}': {}", cmd.name, e);
                }
            } else {
                warn!(
                    "[runcommand] unknown key '{}' from {}",
                    key, device.device_id
                );
            }
        } else if self.request_command_list == Some(true) {
            // Phone is asking for our current command list.
            send_command_list(&device.device_id, core_tx).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Outgoing — send our command list to the phone
// ---------------------------------------------------------------------------

/// Build and send a `kdeconnect.runcommand` packet to the phone containing
/// all locally defined commands and `canAddCommand: true`.
///
/// Called on device connect (kdeconnect-core/src/lib.rs) and when the phone
/// requests the list via `requestCommandList: true`.
pub async fn send_command_list(
    device_id: &crate::device::DeviceId,
    core_tx: mpsc::UnboundedSender<crate::event::CoreEvent>,
) {
    let commands = load_local_commands();

    // commandList must be a *JSON-encoded string*, not a nested object.
    // Format: "{\"<uuid>\": {\"name\": \"...\", \"command\": \"...\"}, ...}"
    let mut map = serde_json::Map::new();
    for cmd in &commands {
        map.insert(
            cmd.id.clone(),
            serde_json::json!({ "name": cmd.name, "command": cmd.command }),
        );
    }
    let command_list_str =
        serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".to_string());

    info!(
        "[runcommand] sending {} command(s) to {}",
        commands.len(),
        device_id
    );

    let packet = ProtocolPacket::new(
        PacketType::RunCommand,
        serde_json::json!({
            "commandList": command_list_str,
            "canAddCommand": true,
        }),
    );

    let _ = core_tx.send(crate::event::CoreEvent::SendPacket {
        device: device_id.clone(),
        packet,
    });
}
