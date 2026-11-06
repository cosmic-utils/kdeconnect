use std::time::Instant;

use pin_project::pin_project;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc::{self},
    time::{Duration, Interval, interval},
};

use crate::event::ConnectionEvent;

#[pin_project]
pub(crate) struct TransferAdapter<R: AsyncRead> {
    #[pin]
    inner: R,
    transfer_interval: Interval,
    transfer_bytes: usize,
    total_size: u64,
    processed_percent: u8,
    pub(crate) notify_tx: mpsc::UnboundedSender<ConnectionEvent>,
}

impl<R: AsyncRead> TransferAdapter<R> {
    pub fn new(
        inner: R,
        total_size: u64,
        connection_tx: mpsc::UnboundedSender<ConnectionEvent>,
    ) -> Self {
        Self {
            inner,
            transfer_interval: interval(Duration::from_millis(100)),
            transfer_bytes: 0,
            total_size,
            processed_percent: 0,
            notify_tx: connection_tx,
        }
    }
}

impl<R: AsyncRead> AsyncRead for TransferAdapter<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.project();
        let before = buf.filled().len();
        let result = this.inner.poll_read(cx, buf);
        let filled_len = buf.filled().len() - before;

        *this.transfer_bytes += filled_len;
        *this.processed_percent =
            calculate_progress(*this.transfer_bytes as f64, *this.total_size as f64);

        // Emit 100% immediately on EOF (zero-byte read) so small/fast
        // transfers always surface a completion update regardless of the
        // 100 ms ticker cadence.
        if matches!(result, std::task::Poll::Ready(Ok(()))) && filled_len == 0 {
            send_progress(100, this.notify_tx.clone());
            return result;
        }

        match this.transfer_interval.poll_tick(cx) {
            std::task::Poll::Pending => {}
            std::task::Poll::Ready(_) => {
                send_progress(*this.processed_percent, this.notify_tx.clone());
            }
        }

        result
    }
}

fn calculate_progress(transferred: f64, total: f64) -> u8 {
    if total > 0.0 && transferred > 0.0 {
        (transferred / total * 100.0).round().min(100.0) as u8
    } else {
        0
    }
}

pub(crate) fn send_progress(percent: u8, notify_tx: mpsc::UnboundedSender<ConnectionEvent>) {
    let _ = notify_tx.send(ConnectionEvent::UpdateTransferProgress(percent));
}

/// One file transfer. `state` is transferring, completed or failed;
/// `direction` is upload or download from the desktop's perspective.
#[derive(Debug, Clone, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct TransferInfo {
    pub transfer_id: String,
    pub device_id: String,
    pub filename: String,
    pub direction: String,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    /// The final local path, populated after a downloaded file is published.
    pub path: String,
    pub state: String,
    pub error: String,
}

pub(crate) struct FileTransfer {
    progress: TransferInfo,
    events: mpsc::UnboundedSender<ConnectionEvent>,
    last_update: Instant,
}

impl FileTransfer {
    pub fn new(
        device_id: &str,
        filename: &str,
        direction: &str,
        total_bytes: u64,
        events: mpsc::UnboundedSender<ConnectionEvent>,
    ) -> Self {
        let transfer = Self {
            progress: TransferInfo {
                transfer_id: uuid::Uuid::new_v4().to_string(),
                device_id: device_id.to_owned(),
                filename: filename.to_owned(),
                direction: direction.to_owned(),
                bytes_transferred: 0,
                total_bytes,
                path: String::new(),
                state: "transferring".into(),
                error: String::new(),
            },
            events,
            last_update: Instant::now(),
        };
        transfer.report();
        transfer
    }

    fn report(&self) {
        let _ = self
            .events
            .send(ConnectionEvent::FileTransferProgress(self.progress.clone()));
        let percent = if self.progress.state == "transferring" {
            ((u128::from(self.progress.bytes_transferred) * 100)
                / u128::from(self.progress.total_bytes.max(1)))
            .min(99) as u8
        } else {
            100
        };
        send_progress(percent, self.events.clone());
    }

    fn advance(&mut self, bytes: u64) {
        self.progress.bytes_transferred += bytes;
        if self.last_update.elapsed() >= Duration::from_millis(100) {
            self.report();
            self.last_update = Instant::now();
        }
    }

    pub fn finish<T>(&mut self, result: &anyhow::Result<T>) {
        match result {
            Ok(_) => self.progress.state = "completed".into(),
            Err(error) => {
                self.progress.state = "failed".into();
                self.progress.error = error.to_string();
            }
        }
        self.report();
    }

    pub fn set_path(&mut self, path: &std::path::Path) {
        self.progress.path = path.to_string_lossy().into_owned();
    }
}

pub(crate) async fn copy_payload(
    input: impl AsyncRead + Unpin,
    output: &mut (impl AsyncWrite + Unpin),
    expected_size: Option<u64>,
    mut progress: Option<&mut FileTransfer>,
) -> anyhow::Result<()> {
    let mut input = input.take(expected_size.unwrap_or(u64::MAX));
    let mut buffer = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    loop {
        let bytes =
            tokio::time::timeout(Duration::from_secs(30), input.read(&mut buffer)).await??;
        if bytes == 0 {
            break;
        }
        tokio::time::timeout(Duration::from_secs(30), output.write_all(&buffer[..bytes])).await??;
        received += bytes as u64;
        if let Some(progress) = progress.as_deref_mut() {
            progress.advance(bytes as u64);
        }
    }
    anyhow::ensure!(
        expected_size.is_none_or(|size| received == size),
        "Incomplete payload: received {received} bytes, expected {expected_size:?}"
    );
    tokio::time::timeout(Duration::from_secs(30), output.flush()).await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_files_are_identifiable_and_truncation_fails() {
        let (events, mut received) = mpsc::unbounded_channel();
        let mut first = FileTransfer::new("device-a", "same.txt", "download", 4, events.clone());
        let mut second = FileTransfer::new("device-a", "same.txt", "upload", 3, events);
        assert_ne!(first.progress.transfer_id, second.progress.transfer_id);
        let partial = copy_payload(&b"abc"[..], &mut Vec::new(), Some(4), Some(&mut first)).await;
        first.finish(&partial);
        let complete = copy_payload(&b"abc"[..], &mut Vec::new(), Some(3), Some(&mut second)).await;
        second.finish(&complete);
        let mut finished = Vec::new();
        while let Ok(event) = received.try_recv() {
            if let ConnectionEvent::FileTransferProgress(progress) = event
                && progress.state != "transferring"
            {
                finished.push(progress);
            }
        }
        assert_eq!(finished.len(), 2);
        assert_eq!(finished[0].device_id, "device-a");
        assert_eq!(finished[0].filename, "same.txt");
        assert_eq!(finished[0].bytes_transferred, 3);
        assert_eq!(finished[0].state, "failed");
        assert!(!finished[0].error.is_empty());
        assert_eq!(finished[1].direction, "upload");
        assert_eq!(finished[1].bytes_transferred, 3);
        assert_eq!(finished[1].state, "completed");
    }
}
