mod avatar;
mod emoji;
mod utils;
mod views;

pub mod actions;
pub mod app;
pub mod dbus;
pub mod models;

pub use actions::SmsMessage;
pub use app::SmsWindow;

/// Run the SMS window application.
///
/// This is the real entry point of the separate `cosmic-ext-connect-sms`
/// binary (see cosmic-ext-connect-sms.rs) — it's not unused. The
/// `#[allow(dead_code)]` below exists only because this crate builds
/// several binaries (the panel applet, settings, and this one) sharing one
/// library, and the compiler can't see across binary targets: checking the
/// library on its own, or building a different binary that never calls
/// this, makes it look orphaned even though it's actively in use whenever
/// cosmic-ext-connect-sms itself is built.
#[allow(dead_code)]
pub fn run(device_id: String, device_name: String) -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default();
    cosmic::app::run::<SmsWindow>(settings, (device_id, device_name))
}
