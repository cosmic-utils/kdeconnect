// #[allow(dead_code)] = Placeholder for code that will be used once features are fully integrated

use crate::models::Device;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    Noop,
    PopupClosed(cosmic::iced::window::Id),
    SwitchPage(Page),
    RefreshDevices,
    DevicesUpdated(Vec<Device>),
    ToggleDeviceMenu(String),

    // From About page links open
    OpenRepository,
    OpenSupport,
    OpenLicense,

    // Device actions
    PingDevice(String),
    PairDevice(String),
    UnpairDevice(String),
    RingDevice(String),
    BrowseDevice(String),
    UnmountDevice(String),
    BrowseDeviceFailed(String),
    DismissError,
    SendFiles(String),
    SendSMS(String),
    ShareClipboard(String),
    ShareText(String),
    ShareUrl(String),
    UpdateTransferProgress(u8),

    ClipboardSendFinished {
        device_id: String,
        result: Result<(), String>,
    },

    // Battery and connectivity updates — patch device in place without full refresh
    BatteryUpdated(String, i32, bool), // device_id, level, is_charging
    ConnectivityUpdated(String, i32),  // device_id, signal_strength

    // Advanced features
    RemoteInput(String),
    LockDevice(String),
    PresenterMode(String),
    UseAsMonitor(String),
    OpenSettings,

    // Pairing
    AcceptPairing(String),
    RejectPairing(String),
    PairingRequestReceived(String, String), // device_id, device_name

    // Delayed refresh for post-pairing updates
    DelayedRefresh,

    // MPRIS events from phone - store as JSON value to avoid direct dependency
    MprisReceived(String, serde_json::Value), // device_id, mpris_data

    // Media section — read straight from each phone's MPRIS D-Bus service.
    MprisSnapshot(HashMap<String, crate::models::NowPlaying>), // bus_name -> state
    MprisPlayPause(String),                                    // bus_name
    MprisNext(String),                                         // bus_name
    MprisPrevious(String),                                     // bus_name

    // Run Command
    RequestRunCommands(String),          // device_id
    RunCommandsReceived(String, String), // device_id, commands_json
    ExecuteRunCommand(String, String),   // device_id, key

    // SMS unread indicator for the quick-actions menu — device_id -> has_unread
    UnreadSmsUpdated(HashMap<String, bool>),
}

#[derive(Debug, Clone, Default)]
pub enum Page {
    #[default]
    Dashboard,
    About,
}
