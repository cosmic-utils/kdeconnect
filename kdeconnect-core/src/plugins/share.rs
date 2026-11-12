use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    device::Device,
    event::ConnectionEvent,
    protocol::{PacketPayloadTransferInfo, PacketType, ProtocolPacket},
    transport::receive_payload,
};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum ShareRequest {
    File(ShareRequestFile),
    Text { text: String },
    Url { url: String },
}

impl Default for ShareRequest {
    fn default() -> Self {
        ShareRequest::Text {
            text: "Hello From COSMIC Desktop".to_string(),
        }
    }
}

impl ShareRequest {
    pub async fn share_files(content: Vec<String>) -> anyhow::Result<Vec<(Self, String)>> {
        if content.is_empty() {
            return Ok(vec![]);
        }

        let mut requests = Vec::with_capacity(content.len());

        for file in content.iter() {
            let pathbuf = PathBuf::from_str(file)?;

            if !pathbuf.exists() {
                continue;
            }

            let filename = pathbuf
                .file_name()
                .unwrap()
                .to_str()
                .expect("OsString conversion")
                .to_string();

            let body = ShareRequestFile {
                filename,
                open: Some(false),
            };

            requests.push((Self::File(body), pathbuf.to_str().unwrap().to_string()));
        }

        Ok(requests)
    }

    pub async fn receive_share(
        &self,
        device: &Device,
        info: Option<&PacketPayloadTransferInfo>,
        payload_size: Option<u64>,
        events: mpsc::UnboundedSender<ConnectionEvent>,
    ) -> anyhow::Result<()> {
        match self {
            ShareRequest::File(f) => {
                let path = self
                    .handle_file_request(f, device, info, payload_size)
                    .await?;
                let _ = events.send(ConnectionEvent::FileReceived((
                    device.device_id.clone(),
                    path,
                )));
                Ok(())
            }
            ShareRequest::Text { text } => {
                let text = text.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = notify_rust::Notification::new()
                        .appname("KDE Connect")
                        .summary("Text received from phone")
                        .body(&text)
                        .show();
                })
                .await
                .ok();
                Ok(())
            }
            ShareRequest::Url { url } => {
                let url = url.clone();
                tokio::task::spawn_blocking(move || {
                    if std::process::Command::new("xdg-open")
                        .arg(&url)
                        .spawn()
                        .is_err()
                    {
                        let _ = notify_rust::Notification::new()
                            .appname("KDE Connect")
                            .summary("URL received from phone")
                            .body(&url)
                            .show();
                    }
                })
                .await
                .ok();
                Ok(())
            }
        }
    }

    async fn handle_file_request(
        &self,
        request: &ShareRequestFile,
        device: &Device,
        info: Option<&PacketPayloadTransferInfo>,
        payload_size: Option<u64>,
    ) -> anyhow::Result<PathBuf> {
        validate_filename(&request.filename)?;
        let download_dir = dirs::download_dir().unwrap_or_else(|| {
            warn!("[share] cannot find Downloads dir, falling back to /tmp");
            PathBuf::from("/tmp")
        });
        tokio::fs::create_dir_all(&download_dir).await?;
        let temporary = tempfile::Builder::new()
            .prefix(".kdeconnect-")
            .tempfile_in(&download_dir)?;
        if let Some(info) = info {
            let mut remote_addr = device.address;
            remote_addr.set_port(info.port);
            receive_payload(
                &device.device_id,
                &remote_addr,
                &temporary.path().to_path_buf(),
                payload_size,
            )
            .await?;
        } else {
            anyhow::ensure!(payload_size == Some(0), "Missing file payload information");
        }
        let dest = persist_received(temporary, &download_dir, &request.filename)?;
        info!("[share] saved '{}' to {:?}", request.filename, dest);

        let dest_display = dest.display().to_string();
        let filename = request.filename.clone();
        tokio::task::spawn_blocking(move || {
            let _ = notify_rust::Notification::new()
                .appname("KDE Connect")
                .summary(&format!("File received: {}", filename))
                .body(&dest_display)
                .show();
        })
        .await
        .ok();

        Ok(dest)
    }
}

fn validate_filename(filename: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !filename.is_empty()
            && filename != "."
            && filename != ".."
            && !filename.contains(['/', '\\', '\0']),
        "Invalid shared filename"
    );
    Ok(())
}

fn persist_received(
    mut temporary: tempfile::NamedTempFile,
    directory: &Path,
    filename: &str,
) -> anyhow::Result<PathBuf> {
    let path = Path::new(filename);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    for index in 0u64.. {
        let name = if index == 0 {
            filename.to_owned()
        } else {
            format!("{stem} ({index}){extension}")
        };
        let dest = directory.join(name);
        match temporary.persist_noclobber(&dest) {
            Ok(_) => return Ok(dest),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.file;
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn received_files_reject_paths_and_keep_existing_files() {
        for name in [
            "",
            ".",
            "..",
            "../photo.jpg",
            "/photo.jpg",
            "dir\\photo.jpg",
            "a\0b",
        ] {
            assert!(validate_filename(name).is_err());
        }
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("photo.jpg"), b"original").unwrap();
        let mut received = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        received.write_all(b"received").unwrap();
        let saved = persist_received(received, directory.path(), "photo.jpg").unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("photo.jpg")).unwrap(),
            b"original"
        );
        assert_eq!(saved, directory.path().join("photo (1).jpg"));
        assert_eq!(std::fs::read(saved).unwrap(), b"received");
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ShareRequestFile {
    pub filename: String,
    pub open: Option<bool>,
}

impl ShareRequest {
    pub async fn send_file(
        &self,
        writer: &mpsc::UnboundedSender<ProtocolPacket>,
        payload_size: u64,
        payload_transfer_info: Option<PacketPayloadTransferInfo>,
    ) {
        let packet = ProtocolPacket::new_with_payload(
            PacketType::ShareRequest,
            serde_json::to_value(self.clone()).expect("failed serialize packet body"),
            payload_size,
            payload_transfer_info,
        );

        let _ = writer.send(packet);
    }
}
