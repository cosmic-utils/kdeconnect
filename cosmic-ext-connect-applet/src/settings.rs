#[macro_use]
extern crate cosmic_ext_connect_applet;

use cosmic::cosmic_config::{ConfigGet, ConfigSet};
use cosmic::widget::nav_bar;
use cosmic::widget::space::horizontal;
use cosmic::{
    Action, Application, ApplicationExt, Element, Task,
    app::Core,
    iced::{Alignment, Length, Subscription},
    widget,
};
use cosmic::{Apply, theme};
use cosmic_ext_connect_applet::{backend, models::Device};
use futures::StreamExt as _;
use std::collections::HashMap;

/// A desktop command stored as JSON: {id, name, command}
type LocalCommand = serde_json::Value;

const RUN_COMMANDS_CONFIG_ID: &str = "io.github.hepp3n.kdeconnect";
const RUN_COMMANDS_CONFIG_VERSION: u64 = 1;
const RUN_COMMANDS_CONFIG_KEY: &str = "run_commands";

fn run_commands_config() -> Option<cosmic::cosmic_config::Config> {
    cosmic::cosmic_config::Config::new(RUN_COMMANDS_CONFIG_ID, RUN_COMMANDS_CONFIG_VERSION).ok()
}

fn load_run_commands() -> Vec<LocalCommand> {
    run_commands_config()
        .and_then(|cfg| cfg.get::<Vec<LocalCommand>>(RUN_COMMANDS_CONFIG_KEY).ok())
        .unwrap_or_default()
}

fn save_run_commands(commands: &[LocalCommand]) {
    // Write to cosmic_config for applet persistence across sessions.
    if let Some(cfg) = run_commands_config() {
        let _ = cfg.set(RUN_COMMANDS_CONFIG_KEY, commands);
    }
    // dirs::config_dir() already reads XDG_CONFIG_HOME (sandboxed under Flatpak)
    // and falls back to ~/.config outside it.
    let path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(kdeconnect_core::config::CONFIG_DIR)
        .join("runcommand.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(commands) {
        let _ = std::fs::write(path, json);
    }
}

// ---------------------------------------------------------------------------
// Plugin metadata
// ---------------------------------------------------------------------------

struct PluginInfo {
    id: &'static str,
    name: String,
    description: String,
    icon: &'static str,
}

fn implemented_plugins() -> &'static [PluginInfo] {
    use std::sync::LazyLock;
    static PLUGINS: LazyLock<Vec<PluginInfo>> = LazyLock::new(|| {
        vec![
            PluginInfo {
                id: "battery",
                name: fl!("plugin-battery-name"),
                description: fl!("plugin-battery-desc"),
                icon: "battery-symbolic",
            },
            PluginInfo {
                id: "clipboard",
                name: fl!("plugin-clipboard-name"),
                description: fl!("plugin-clipboard-desc"),
                icon: "edit-paste-symbolic",
            },
            PluginInfo {
                id: "connectivity_report",
                name: fl!("plugin-connectivity-name"),
                description: fl!("plugin-connectivity-desc"),
                icon: "network-cellular-symbolic",
            },
            PluginInfo {
                id: "contacts",
                name: fl!("plugin-contacts-name"),
                description: fl!("plugin-contacts-desc"),
                icon: "x-office-address-book-symbolic",
            },
            PluginInfo {
                id: "findmyphone",
                name: fl!("plugin-findmyphone-name"),
                description: fl!("plugin-findmyphone-desc"),
                icon: "audio-speakers-symbolic",
            },
            PluginInfo {
                id: "mpris",
                name: fl!("plugin-mpris-name"),
                description: fl!("plugin-mpris-desc"),
                icon: "media-playback-start-symbolic",
            },
            PluginInfo {
                id: "notification",
                name: fl!("plugin-notifications-name"),
                description: fl!("plugin-notifications-desc"),
                icon: "preferences-system-notifications-symbolic",
            },
            PluginInfo {
                id: "ping",
                name: fl!("plugin-ping-name"),
                description: fl!("plugin-ping-desc"),
                icon: "network-transmit-receive-symbolic",
            },
            PluginInfo {
                id: "runcommand",
                name: fl!("plugin-runcommand-name"),
                description: fl!("plugin-runcommand-desc"),
                icon: "utilities-terminal-symbolic",
            },
            PluginInfo {
                id: "share",
                name: fl!("plugin-share-name"),
                description: fl!("plugin-share-desc"),
                icon: "document-send-symbolic",
            },
            PluginInfo {
                id: "sms",
                name: fl!("plugin-sms-name"),
                description: fl!("plugin-sms-desc"),
                icon: "mail-message-new-symbolic",
            },
            PluginInfo {
                id: "systemvolume",
                name: fl!("plugin-systemvolume-name"),
                description: fl!("plugin-systemvolume-desc"),
                icon: "audio-volume-high-symbolic",
            },
            PluginInfo {
                id: "telephony",
                name: fl!("plugin-telephony-name"),
                description: fl!("plugin-telephony-desc"),
                icon: "phone-symbolic",
            },
        ]
    });
    &PLUGINS
}

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    AvailableDevices,
    DeviceProfile(String),
    Commands,
    AddCommand,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub enum QuickMessages {
    Ping(String),
    FindMyPhone(String),
    ShareCliphboard(String),
    SMS(String),
    SendFiles(String),
    BrowseDevice(String),
    UmountDevice(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    DevicesLoaded(Vec<Device>),
    /// Fired when persisted plugin states are read back for a device.
    /// Payload is (device_id, list-of-disabled-plugin-ids).
    PluginStatesLoaded(String, Vec<String>),
    TogglePlugin(String, bool),
    Refresh,
    PairDevice(String),
    UnpairDevice(String),
    /// Fired by the D-Bus event subscription whenever a device connects or pairs.
    ServiceEvent(kdeconnect_dbus_client::ServiceEvent),
    // Run Quick Action
    RunQuickAction(QuickMessages),
    BrowseDeviceFailed(String),
    // Run Command management
    OpenDeviceProfile,
    OpenCommandsTab,
    OpenCommandAddTab,
    RunCommandsLoaded(Vec<LocalCommand>),
    NewRunCommandName(String),
    NewRunCommandCommand(String),
    AddRunCommand,
    DeleteRunCommand(String),
    // DismissBanner by clearing field
    DismissError,
}

// ---------------------------------------------------------------------------
// Application model
// ---------------------------------------------------------------------------

pub struct SettingsApp {
    core: Core,
    nav: nav_bar::Model,
    active_tab: Tab,
    devices: Vec<Device>,
    selected_device: Option<String>,
    /// device_id → (plugin_id → enabled)
    plugin_states: HashMap<String, HashMap<String, bool>>,
    pairing_in_progress: HashMap<String, bool>,
    /// Desktop commands manageable from the Run Command section
    run_commands: Vec<LocalCommand>,
    new_cmd_name: String,
    new_cmd_command: String,
    // banner message
    message_banner: Option<String>,
}

impl SettingsApp {
    fn plugin_enabled(&self, plugin_id: &str) -> bool {
        self.selected_device
            .as_ref()
            .and_then(|did| self.plugin_states.get(did))
            .and_then(|map| map.get(plugin_id))
            .copied()
            .unwrap_or(true)
    }

    fn default_plugin_map() -> HashMap<String, bool> {
        implemented_plugins()
            .iter()
            .map(|p| (p.id.to_string(), true))
            .collect()
    }

    fn load_plugin_states_task(device_id: String) -> Task<Action<Message>> {
        Task::perform(
            async move {
                let disabled = backend::get_disabled_plugins(device_id.clone()).await;
                (device_id, disabled)
            },
            |(did, disabled)| Action::App(Message::PluginStatesLoaded(did, disabled)),
        )
    }

    fn refresh_devices_task() -> Task<Action<Message>> {
        Task::perform(async { backend::fetch_devices().await }, |devices| {
            Action::App(Message::DevicesLoaded(devices))
        })
    }
}

impl Application for SettingsApp {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "io.github.hepp3n.kdeconnect.settings";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Action<Self::Message>>) {
        let nav = nav_bar::Model::default();

        let mut app = Self {
            core,
            nav,
            active_tab: Tab::AvailableDevices,
            devices: vec![],
            selected_device: None,
            plugin_states: HashMap::new(),
            pairing_in_progress: HashMap::new(),
            run_commands: Vec::new(),
            new_cmd_name: String::new(),
            new_cmd_command: String::new(),
            message_banner: None,
        };

        app.core.window.header_title = fl!("settings-title").into();

        app.update_nav_model();

        let title_task =
            app.set_window_title(fl!("settings-title"), app.core.main_window_id().unwrap());

        let load_task = Task::perform(
            async {
                if let Err(e) = backend::initialize().await {
                    eprintln!("[settings] backend init failed: {:?}", e);
                }
                backend::fetch_devices().await
            },
            |devices| Action::App(Message::DevicesLoaded(devices)),
        );

        let cmds_task = Task::perform(async { load_run_commands() }, |cmds| {
            Action::App(Message::RunCommandsLoaded(cmds))
        });

        (app, Task::batch(vec![title_task, load_task, cmds_task]))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // Subscribe to D-Bus service events so the UI updates immediately when
        // a device connects, pairs, or disconnects — no polling needed.
        Subscription::run(|| {
            async_stream::stream! {
                let mut stream = backend::event_stream().await;
                while let Some(event) = stream.next().await {
                    yield Message::ServiceEvent(event);
                }
            }
        })
    }

    fn nav_model(&self) -> Option<&nav_bar::Model> {
        Some(&self.nav)
    }

    fn on_nav_select(&mut self, id: nav_bar::Id) -> Task<cosmic::Action<Self::Message>> {
        // Activate the page in the model.
        self.nav.activate(id);

        match self.nav.data::<Tab>(id) {
            Some(tab) => {
                // if we found a device, switch to Tab::DeviceProfile
                self.active_tab = tab.to_owned();

                return match tab {
                    Tab::AvailableDevices => {
                        self.selected_device = None;
                        Self::refresh_devices_task()
                    }
                    Tab::DeviceProfile(id) => {
                        self.selected_device = Some(id.to_string());

                        // Show defaults immediately, then load persisted state
                        self.plugin_states
                            .entry(id.clone())
                            .or_insert_with(Self::default_plugin_map);

                        Self::load_plugin_states_task(id.clone())
                    }
                    _ => Task::none(),
                };
            }

            None => {
                // Make sure AvailableDevices is def
                self.active_tab = Tab::AvailableDevices;
                self.selected_device = None;

                Self::refresh_devices_task()
            }
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Action<Self::Message>> {
        match message {
            Message::DevicesLoaded(devices) => {
                if let Some(sel) = self.selected_device.clone() {
                    // Clear selection if the device is gone or no longer paired.
                    let still_paired = devices.iter().any(|d| d.id == sel && d.is_paired);

                    if !still_paired {
                        self.plugin_states.remove(&sel);
                        self.selected_device = None;
                    }
                }

                if self.selected_device.is_none() {
                    self.selected_device =
                        devices.iter().find(|d| d.is_paired).map(|d| d.id.clone());
                }

                for d in &devices {
                    if d.is_paired {
                        self.pairing_in_progress.remove(&d.id);
                    }
                }

                self.devices = devices;

                self.update_nav_model();
            }

            Message::PluginStatesLoaded(device_id, disabled) => {
                let mut map = Self::default_plugin_map();

                for pid in disabled {
                    map.insert(pid, false);
                }

                self.plugin_states.insert(device_id, map);
            }

            Message::TogglePlugin(plugin_id, enabled) => {
                if let Some(ref device_id) = self.selected_device.clone() {
                    self.plugin_states
                        .entry(device_id.clone())
                        .or_insert_with(Self::default_plugin_map)
                        .insert(plugin_id.clone(), enabled);

                    let did = device_id.clone();
                    let pid = plugin_id;
                    return Task::perform(
                        async move {
                            if let Err(e) = backend::set_plugin_enabled(did, pid, enabled).await {
                                eprintln!("[settings] set_plugin_enabled failed: {:?}", e);
                            }
                        },
                        |_| Action::App(Message::Refresh),
                    );
                }
            }

            Message::Refresh => {
                self.update_nav_model();
                return Self::refresh_devices_task();
            }

            Message::PairDevice(device_id) => {
                self.pairing_in_progress.insert(device_id.clone(), true);
                let id = device_id;
                return Task::perform(
                    async move {
                        backend::pair_device(id).await.ok();
                        backend::fetch_devices().await
                    },
                    |devices| Action::App(Message::DevicesLoaded(devices)),
                );
            }

            Message::UnpairDevice(device_id) => {
                if self.selected_device.as_deref() == Some(&device_id) {
                    self.selected_device = None;
                }
                self.plugin_states.remove(&device_id);
                let id = device_id;
                return Task::perform(
                    async move {
                        backend::unpair_device(id).await.ok();
                        backend::fetch_devices().await
                    },
                    |devices| Action::App(Message::DevicesLoaded(devices)),
                );
            }

            // D-Bus service event — refresh only when the device list/state
            // actually changes (connect, disconnect, pair). Battery, clipboard,
            // SMS, etc. don't affect the settings display so we skip them to
            // avoid continuous view rebuilds during scrolling.
            Message::ServiceEvent(event) => {
                let needs_refresh = matches!(
                    event,
                    kdeconnect_dbus_client::ServiceEvent::DeviceConnected(..)
                        | kdeconnect_dbus_client::ServiceEvent::DeviceDisconnected(..)
                        | kdeconnect_dbus_client::ServiceEvent::DevicePaired(..)
                );
                if needs_refresh {
                    return Self::refresh_devices_task();
                }
            }
            Message::OpenDeviceProfile => {
                if let Some(ref id) = self.selected_device {
                    self.active_tab = Tab::DeviceProfile(id.clone());
                }
            }
            Message::OpenCommandsTab => {
                self.active_tab = Tab::Commands;
            }
            Message::OpenCommandAddTab => {
                self.active_tab = Tab::AddCommand;
            }
            Message::RunQuickAction(action) => match action {
                QuickMessages::Ping(id) => {
                    let id = id.clone();
                    return Task::perform(
                        async move {
                            backend::ping_device(id).await.ok();
                        },
                        |_| cosmic::action::app(Message::Refresh),
                    );
                }
                QuickMessages::FindMyPhone(id) => {
                    let id = id.clone();
                    return Task::perform(
                        async move {
                            backend::ring_device(id).await.ok();
                        },
                        |_| cosmic::action::app(Message::Refresh),
                    );
                }
                QuickMessages::ShareCliphboard(id) => {
                    let id = id.clone();
                    return Task::perform(
                        async move {
                            backend::share_clipboard(id)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        move |_| cosmic::action::app(Message::Refresh),
                    );
                }
                QuickMessages::SMS(id) => {
                    let id = id.clone();
                    let device_name: Option<String> = self
                        .devices
                        .iter()
                        .find(|d| d.id == id)
                        .and_then(|d| Some(d.name.clone()));

                    if let Some(device_name) = device_name {
                        // Spawn in a thread so the process::Command doesn't block the executor
                        std::thread::spawn(move || {
                            match std::process::Command::new("cosmic-ext-connect-sms")
                                .arg(&id)
                                .arg(&device_name)
                                .spawn()
                            {
                                Ok(_) => tracing::info!("cosmic-ext-connect-sms launched"),
                                Err(e) => tracing::error!(
                                    "Failed to launch cosmic-ext-connect-sms: {:?}",
                                    e
                                ),
                            }
                        });
                    }
                }
                QuickMessages::SendFiles(id) => {
                    let id = id.clone();
                    return Task::perform(
                        async move {
                            let files = cosmic_ext_connect_applet::portal::pick_files(
                                &fl!("file-picker-title"),
                                true,
                                None,
                            )
                            .await;
                            if !files.is_empty() {
                                backend::send_files(id, files).await.ok();
                            }
                        },
                        |_| cosmic::action::app(Message::Refresh),
                    );
                }
                QuickMessages::BrowseDevice(id) => {
                    let id = id.clone();
                    return Task::perform(
                        async move { backend::browse_device_filesystem(id).await },
                        |result| match result {
                            Ok(()) => cosmic::action::app(Message::Refresh),
                            Err(e) => {
                                cosmic::action::app(Message::BrowseDeviceFailed(e.to_string()))
                            }
                        },
                    );
                }
                QuickMessages::UmountDevice(id) => {
                    let id = id.clone();
                    return Task::perform(
                        async move { backend::browse_device_filesystem(id).await },
                        |result| match result {
                            Ok(()) => cosmic::action::app(Message::Refresh),
                            Err(e) => {
                                cosmic::action::app(Message::BrowseDeviceFailed(e.to_string()))
                            }
                        },
                    );
                }
            },
            Message::BrowseDeviceFailed(failure) => {
                self.message_banner = Some(failure);
            }
            Message::DismissError => {
                self.message_banner = None;
            }
            Message::RunCommandsLoaded(cmds) => {
                self.run_commands = cmds;
            }
            Message::NewRunCommandName(s) => {
                self.new_cmd_name = s;
            }
            Message::NewRunCommandCommand(s) => {
                self.new_cmd_command = s;
            }
            Message::AddRunCommand => {
                let name = self.new_cmd_name.trim().to_string();
                let cmd = self.new_cmd_command.trim().to_string();
                if !name.is_empty() && !cmd.is_empty() {
                    self.run_commands.push(serde_json::json!({
                        "id": uuid::Uuid::new_v4().to_string(),
                        "name": name,
                        "command": cmd,
                    }));
                    self.new_cmd_name.clear();
                    self.new_cmd_command.clear();
                    save_run_commands(&self.run_commands);

                    self.active_tab = Tab::Commands;

                    if let Some(device_id) = self.selected_device.clone() {
                        return Task::perform(
                            async move { backend::push_local_commands(device_id).await },
                            |_| Action::App(Message::Refresh),
                        );
                    }
                }
            }
            Message::DeleteRunCommand(id) => {
                self.run_commands.retain(|c| c["id"].as_str() != Some(&id));
                save_run_commands(&self.run_commands);
                if let Some(device_id) = self.selected_device.clone() {
                    return Task::perform(
                        async move { backend::push_local_commands(device_id).await },
                        |_| Action::App(Message::Refresh),
                    );
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let spacing = cosmic::theme::active().cosmic().spacing;

        let content: Element<'_, Message> = match &self.active_tab {
            Tab::DeviceProfile(_) => self.view_plugin_panel(&spacing),
            Tab::Commands => self.view_run_commands_section(&spacing),
            Tab::AddCommand => self.view_add_commands_section(&spacing),
            Tab::AvailableDevices => self.view_available_devices(&spacing),
        };

        content.into()
    }
}

// ---------------------------------------------------------------------------
// View helpers
// ---------------------------------------------------------------------------

impl SettingsApp {
    fn update_nav_model(&mut self) {
        let previous_active = self.nav.active();

        let mut nav_model = widget::segmented_button::ModelBuilder::default();

        nav_model = nav_model.insert(|b| {
            b.text(fl!("settings-tab-available"))
                .data::<Tab>(Tab::AvailableDevices)
                .icon(widget::icon::from_name("list-add-symbolic"))
                .activate()
        });

        if !self.devices.is_empty() {
            for d in &self.devices {
                if d.is_paired && d.is_reachable {
                    nav_model = nav_model.insert(|b| {
                        b.divider_above()
                            .icon(widget::icon::from_name("smartphone-symbolic"))
                            .text(d.name.clone())
                            .data::<Tab>(Tab::DeviceProfile(d.id.clone()))
                    });
                }
            }
        };

        self.nav = nav_model.build();
        self.nav.activate(previous_active);
    }

    fn view_plugin_panel_quick_actions<'a>(
        &'a self,
        device: &Device,
        spacing: &cosmic::cosmic_theme::Spacing,
    ) -> Element<'a, Message> {
        let quick_action_button =
            |icon: &str, action: String, msg: Message| -> Element<'a, Message> {
                widget::button::custom(
                    widget::row(vec![
                        widget::icon::from_name(icon).icon().into(),
                        widget::text(action).into(),
                    ])
                    .align_y(Alignment::Center)
                    .spacing(spacing.space_xxs)
                    .padding([spacing.space_xxxs, spacing.space_xs]),
                )
                .on_press(msg)
                .class(theme::Button::Suggested)
                .into()
            };

        let mut buttons: Vec<Element<'a, Message>> = vec![];
        // ping
        if self.plugin_enabled("ping") {
            buttons.push(quick_action_button(
                "notification-new-symbolic",
                fl!("quick-actions-ping"),
                Message::RunQuickAction(QuickMessages::Ping(device.id.to_string())),
            ));
        };
        // find phone
        if self.plugin_enabled("findmyphone") {
            buttons.push(quick_action_button(
                "phone-symbolic",
                fl!("quick-actions-find-phone"),
                Message::RunQuickAction(QuickMessages::FindMyPhone(device.id.to_string())),
            ))
        };
        // share clipboard
        if self.plugin_enabled("clipboard") {
            buttons.push(quick_action_button(
                "edit-paste-symbolic",
                fl!("quick-actions-share-clipboard"),
                Message::RunQuickAction(QuickMessages::ShareCliphboard(device.id.to_string())),
            ))
        };
        // sms window
        if self.plugin_enabled("sms") {
            buttons.push(quick_action_button(
                "mail-message-new-symbolic",
                fl!("quick-actions-sms"),
                Message::RunQuickAction(QuickMessages::SMS(device.id.to_string())),
            ))
        };
        // send file
        if self.plugin_enabled("share") {
            buttons.push(quick_action_button(
                "document-send-symbolic",
                fl!("quick-actions-send-file"),
                Message::RunQuickAction(QuickMessages::SendFiles(device.id.to_string())),
            ))
        };
        // browse device
        if self.plugin_enabled("share") {
            buttons.push(quick_action_button(
                if !(device.is_mounted) {
                    "folder-symbolic"
                } else {
                    "folder-open-symbolic"
                },
                fl!("quick-actions-browse-device"),
                if !(device.is_mounted) {
                    Message::RunQuickAction(QuickMessages::BrowseDevice(device.id.to_string()))
                } else {
                    Message::RunQuickAction(QuickMessages::UmountDevice(device.id.to_string()))
                },
            ));
        };

        widget::flex_row::flex_row(buttons)
            .spacing(spacing.space_xxs)
            .width(Length::Fill)
            .into()
    }

    fn view_plugin_panel<'a>(
        &'a self,
        spacing: &cosmic::cosmic_theme::Spacing,
    ) -> Element<'a, Message> {
        let mut col = widget::Column::new()
            .spacing(spacing.space_s)
            .padding(spacing.space_s)
            .width(Length::Fill);

        if let Some(ref device_id) = self.selected_device {
            if let Some(device) = self.devices.iter().find(|d| &d.id == device_id) {
                let unpair_id = device_id.clone();
                let paired = device.is_paired;

                let mut device_row = widget::Row::new();

                let phone_icon = widget::icon::from_name("smartphone-symbolic").size(42);

                device_row = device_row.push(phone_icon);

                let mut name_col = widget::Column::new()
                    .spacing(spacing.space_s)
                    .push(widget::text::title3(&device.name).width(Length::Fill));

                let mut under_row = widget::Row::new().spacing(spacing.space_xs);

                if let Some(level) = device.battery_level {
                    under_row = under_row.push(
                        widget::Row::new()
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .push(widget::icon::from_name(device.battery_icon()))
                            .push(widget::text(format!("{}%", level))),
                    );
                }

                if let Some(signal_icon) = device.signal_icon() {
                    under_row = under_row.push(widget::icon::from_name(signal_icon));
                }

                name_col = name_col.push(under_row);

                device_row = device_row.push(name_col).align_y(Alignment::Center);

                if paired {
                    device_row = device_row.push(
                        widget::button::destructive(fl!("paired-devices-unpair"))
                            .on_press(Message::UnpairDevice(unpair_id)),
                    );
                }

                col = col.push(device_row);

                // Dismissible error banner — surfaces failures (e.g. browse-device
                // preflight checks) that used to be silently dropped.
                if let Some(ref message) = self.message_banner {
                    col = col.push(
                        widget::container(
                            widget::Row::new()
                                .push(widget::text(message).width(Length::Fill))
                                .push(
                                    widget::button::icon(
                                        widget::icon::from_name("window-close-symbolic").handle(),
                                    )
                                    .on_press(Message::DismissError),
                                )
                                .spacing(spacing.space_xs)
                                .align_y(Alignment::Center),
                        )
                        .padding(spacing.space_s)
                        .style(|_: &cosmic::Theme| cosmic::widget::container::Style {
                            border: cosmic::iced::Border {
                                color: cosmic::iced::Color::from_rgb(0.8, 0.2, 0.2),
                                width: 1.5,
                                radius: 8.0.into(),
                            },
                            ..Default::default()
                        })
                        .class(cosmic::theme::Container::Card)
                        .width(Length::Fill),
                    );
                };

                col = col.push(self.view_plugin_panel_quick_actions(&device, spacing));
                col = col.push(widget::divider::horizontal::default());

                if self.selected_device.is_none() {
                    col = col.push(
                        widget::container(widget::text(fl!("paired-plugins-hint")))
                            .padding(spacing.space_l),
                    );
                    return widget::scrollable(col).height(Length::Fill).into();
                }

                if paired {
                    if self.plugin_enabled("runcommand") {
                        col = col.push(
                            widget::settings::section().add(
                                widget::settings::item::builder(fl!("run-commands-manage-header"))
                                    .icon(widget::icon::from_name("utilities-terminal-symbolic"))
                                    .control(widget::button::icon(widget::icon::from_name(
                                        "go-next-symbolic",
                                    )))
                                    .apply(widget::container)
                                    .align_x(Alignment::Center)
                                    .class(theme::Container::List)
                                    .width(Length::Fill)
                                    .apply(widget::button::custom)
                                    .padding(0)
                                    .class(theme::Button::Transparent)
                                    .on_press(Message::OpenCommandsTab)
                                    .width(Length::Fill),
                            ),
                        );
                    };

                    for plugin in implemented_plugins() {
                        let enabled = self.plugin_enabled(plugin.id);
                        let plugin_id = plugin.id.to_string();

                        col = col.push(
                            widget::settings::section().add(
                                widget::settings::item::builder(&plugin.name)
                                    .description(&plugin.description)
                                    .icon(widget::icon::from_name(plugin.icon).icon())
                                    .toggler(enabled, move |enabled| {
                                        Message::TogglePlugin(plugin_id.clone(), enabled)
                                    }),
                            ),
                        );
                    }
                }
            }
        }

        widget::scrollable(col).height(Length::Fill).into()
    }

    fn view_available_devices<'a>(
        &'a self,
        spacing: &cosmic::cosmic_theme::Spacing,
    ) -> Element<'a, Message> {
        let available: Vec<&Device> = self
            .devices
            .iter()
            .filter(|d| !d.is_paired && d.is_reachable)
            .collect();

        let mut col = widget::Column::new()
            .spacing(spacing.space_s)
            .padding(spacing.space_s)
            .width(Length::Fill);

        col = col.push(
            widget::Row::new()
                .spacing(spacing.space_s)
                .align_y(Alignment::Center)
                .push(widget::text::title3(fl!("available-devices-header")).width(Length::Fill))
                .push(
                    widget::button::standard(fl!("settings-scan-again")).on_press(Message::Refresh),
                ),
        );
        col = col.push(widget::divider::horizontal::default());
        col = col.push(widget::text(fl!("available-devices-hint")));

        if available.is_empty() {
            col = col.push(
                widget::container(
                    widget::Column::new()
                        .spacing(spacing.space_s)
                        .push(widget::icon::from_name("network-offline-symbolic").size(48))
                        .push(
                            widget::text::title4(fl!("available-devices-none"))
                                .font(cosmic::font::bold()),
                        )
                        .push(widget::text(fl!("available-devices-none-hint")))
                        .align_x(Alignment::Center),
                )
                .padding([spacing.space_xl, spacing.space_m])
                .width(Length::Fill)
                .align_x(Alignment::Center),
            );
        } else {
            for device in &available {
                let device_id = device.id.clone();
                let in_progress = *self.pairing_in_progress.get(&device.id).unwrap_or(&false);

                let card = widget::Row::new()
                    .spacing(spacing.space_m)
                    .align_y(Alignment::Center)
                    .push(widget::icon::from_name(device.device_icon()).size(42))
                    .push(
                        widget::Column::new()
                            .spacing(2)
                            .push(
                                widget::text::caption_heading(&device.name)
                                    .font(cosmic::font::bold()),
                            )
                            .push(widget::text(&device.id))
                            .width(Length::Fill),
                    )
                    .push(if in_progress {
                        widget::button::standard(fl!("available-devices-pairing"))
                    } else {
                        widget::button::suggested(fl!("available-devices-pair"))
                            .on_press(Message::PairDevice(device_id))
                    });

                col = col.push(
                    widget::container(card)
                        .padding([spacing.space_s, spacing.space_m])
                        .class(cosmic::theme::Container::Card)
                        .width(Length::Fill),
                );
            }
        }

        widget::scrollable(col).height(Length::Fill).into()
    }

    fn view_run_commands_section<'a>(
        &'a self,
        spacing: &cosmic::cosmic_theme::Spacing,
    ) -> Element<'a, Message> {
        let column = widget::column(vec![
            previous_button(fl!("settings-device-profile"), Message::OpenDeviceProfile),
            {
                let mut section =
                    widget::settings::section().title(fl!("run-commands-manage-header"));

                for cmd in &self.run_commands {
                    let name = cmd["name"].as_str().unwrap_or("");
                    let command = cmd["command"].as_str().unwrap_or("");
                    let delete_id = cmd["id"].as_str().unwrap_or("").to_string();

                    let cmd_col = widget::Column::new()
                        .width(Length::Fill)
                        .push(widget::text::caption_heading(name).font(cosmic::font::bold()))
                        .push(widget::text::caption(command));

                    section = section.add(widget::settings::item_row(vec![
                        cmd_col.into(),
                        widget::button::icon(widget::icon::from_name("user-trash-symbolic"))
                            .on_press(Message::DeleteRunCommand(delete_id))
                            .into(),
                    ]));
                }

                section = section.add(widget::settings::item_row(vec![
                    horizontal().into(),
                    widget::button::suggested(fl!("run-commands-add-button"))
                        .on_press(Message::OpenCommandAddTab)
                        .into(),
                ]));

                section.into()
            },
        ])
        .padding(spacing.space_s)
        .spacing(spacing.space_xxs);

        widget::scrollable(column).into()
    }

    fn view_add_commands_section<'a>(
        &'a self,
        spacing: &cosmic::cosmic_theme::Spacing,
    ) -> Element<'a, Message> {
        let mut col = widget::Column::new()
            .spacing(spacing.space_xs)
            .padding(spacing.space_s)
            .push(previous_button(
                fl!("run-commands-manage-header"),
                Message::OpenCommandsTab,
            ));

        let section = widget::settings::section()
            .add(
                widget::text_input(fl!("run-commands-name-placeholder"), &self.new_cmd_name)
                    .on_input(Message::NewRunCommandName)
                    .width(Length::Fill),
            )
            .add(
                widget::text_input(
                    fl!("run-commands-command-placeholder"),
                    &self.new_cmd_command,
                )
                .on_input(Message::NewRunCommandCommand)
                .width(Length::Fill),
            )
            .add(
                widget::button::suggested(fl!("run-commands-add-button"))
                    .on_press(Message::AddRunCommand),
            );

        col = col.push(section);

        widget::container(col)
            .class(cosmic::theme::Container::Background)
            .width(Length::Fill)
            .into()
    }
}

fn previous_button<'a>(parent_page: String, on_press: Message) -> Element<'a, Message> {
    widget::button::icon(widget::icon::from_name("go-previous-symbolic"))
        .extra_small()
        .padding(0)
        .label(parent_page)
        .spacing(4)
        .class(widget::button::ButtonClass::Link)
        .on_press(on_press)
        .into()
}

fn main() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(800.0, 600.0));
    cosmic::app::run::<SettingsApp>(settings, ())
}
