//! KDE Connect D-Bus Service Daemon

use anyhow::Result;
use tracing::info;

mod clipboard;
mod dbus_interface;

#[tokio::main]
async fn main() -> Result<()> {
    use tracing_subscriber::prelude::*;

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    if std::env::var("KDECONNECT_LOG_FILE").is_ok()
        && std::path::Path::new("/.flatpak-info").exists()
    {
        let log_dir = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let _ = std::fs::create_dir_all(&log_dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("service.log"))
            .expect("failed to open service.log");
        let (non_blocking, _guard) = tracing_appender::non_blocking(file);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(non_blocking);
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .with(file_layer)
            .init();
        std::mem::forget(_guard);
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .init();
    }

    info!("KDE Connect service starting");

    // Single-instance guard: request the well-known D-Bus name before touching
    // any sockets or config files. DoNotQueue means a second instance exits
    // immediately rather than racing on port binding or cert/key generation.
    // The connection is scoped so it drops (releasing the name) before
    // KdeConnectService::new() acquires it on its own connection — otherwise
    // the two request_name calls on different connections would deadlock.
    {
        let guard_conn = zbus::Connection::session().await?;
        match guard_conn
            .request_name_with_flags(
                "io.github.hepp3n.kdeconnect",
                zbus::fdo::RequestNameFlags::DoNotQueue.into(),
            )
            .await
        {
            Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {
                info!("Single-instance guard passed");
            }
            Ok(_) => {
                info!("Another instance is already running — exiting");
                return Ok(());
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    } // guard_conn drops here, name is released for KdeConnectService::new()

    let service = dbus_interface::KdeConnectService::new().await?;
    info!("D-Bus service started on io.github.hepp3n.kdeconnect");

    service.run().await?;

    std::process::exit(0);
}
