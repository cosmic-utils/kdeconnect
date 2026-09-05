use crate::messages::{self, Message};
use crate::models::{Device, NowPlaying};
use cosmic::app::Core;
use cosmic::iced::core::text::Wrapping;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, Row, icon, settings, space::horizontal, text};
use cosmic::{Element, theme, widget};
use std::collections::HashMap;

const APPLET_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the popup view using the real application Core so popup_container
/// has proper applet context, theme, and sizing.
pub fn create_popup_view<'a>(
    core: &'a Core,
    devices: &'a HashMap<String, Device>,
    expanded_device: Option<&'a String>,
    pairing_requests: Option<&'a HashMap<String, String>>,
    error_banner: Option<&'a String>,
    now_playing: &'a HashMap<String, NowPlaying>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;

    let mut content = widget::Column::new().padding(spacing.space_s);

    let about_icon = widget::button::icon(widget::icon::from_name("help-about-symbolic"))
        .on_press(Message::SwitchPage(messages::Page::About));
    let settings_icon = widget::button::icon(widget::icon::from_name("application-menu-symbolic"))
        .on_press(Message::OpenSettings);

    // Header
    content = content.push(
        Row::new()
            .push(about_icon)
            .push(horizontal())
            .push(settings_icon)
            .align_y(Alignment::Center),
    );

    content = content
        .push(widget::divider::horizontal::default())
        .spacing(spacing.space_xxs);

    // Dismissible error banner — surfaces failures (e.g. browse-device
    // preflight checks) that used to be silently dropped.
    if let Some(message) = error_banner {
        content = content.push(
            widget::container(
                Row::new()
                    .push(widget::text(message).size(12).width(Length::Fill))
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
    }

    // Pairing requests — sourced from the applet's live pairing_requests map,
    // not from Device.pairing_requests which is never populated via D-Bus.
    if let Some(requests) = pairing_requests {
        if !requests.is_empty() {
            content = content.push(
                widget::text(fl!("pairing-requests"))
                    .size(14)
                    .font(cosmic::font::bold()),
            );

            let mut sorted: Vec<(&String, &String)> = requests.iter().collect();
            sorted.sort_by(|a, b| a.1.cmp(b.1));

            for (device_id, device_name) in sorted {
                let device_id_accept = device_id.clone();
                let device_id_reject = device_id.clone();

                let request_card = widget::container(
                    widget::Column::new()
                        .push(
                            Row::new()
                                .push(widget::icon::from_name("phone-symbolic").size(24))
                                .push(
                                    widget::Column::new()
                                        .push(widget::text(device_name).size(14))
                                        .push(widget::text(fl!("pairing-wants-to-pair")).size(11))
                                        .spacing(spacing.space_xxxs),
                                )
                                .spacing(spacing.space_s)
                                .align_y(Alignment::Center),
                        )
                        .push(widget::Space::new().height(Length::Fixed(spacing.space_xs as f32)))
                        .push(
                            Row::new()
                                .push(
                                    widget::button::suggested(fl!("pairing-accept"))
                                        .on_press(Message::AcceptPairing(device_id_accept))
                                        .width(Length::Fill),
                                )
                                .push(
                                    widget::button::destructive(fl!("pairing-reject"))
                                        .on_press(Message::RejectPairing(device_id_reject))
                                        .width(Length::Fill),
                                )
                                .spacing(spacing.space_xs),
                        )
                        .spacing(spacing.space_xs),
                )
                .padding(spacing.space_s)
                .class(cosmic::theme::Container::Card)
                .width(Length::Fill);

                content = content.push(request_card);
            }

            content = content.push(widget::divider::horizontal::default());
        }
    }

    // All paired devices — reachable and unreachable — sorted alphabetically
    let mut paired_devices: Vec<_> = devices.values().filter(|d| d.is_paired).collect();
    paired_devices.sort_by(|a, b| a.name.cmp(&b.name));

    if paired_devices.is_empty() {
        content = content.push(
            widget::container(widget::text(fl!("devices-none-paired")).size(15))
                .padding(spacing.space_m)
                .width(Length::Fill)
                .center_x(Length::Fill),
        );
    } else {
        content = content.push(
            widget::container(
                widget::text(fl!("devices-header"))
                    .font(cosmic::font::bold())
                    .size(15),
            )
            .padding(spacing.space_xs)
            .width(Length::Fill),
        );

        for device in paired_devices {
            content = content.push(create_device_card(device, &spacing, expanded_device));
        }
    }

    // Media section — one card per active, actually-controllable phone
    // player, at the bottom. Players that advertise none of the four
    // transport controls (e.g. some browser tabs' media notifications)
    // are skipped entirely rather than shown as a box of dead buttons.
    let mut players: Vec<(&String, &NowPlaying)> = now_playing
        .iter()
        .filter(|(_, p)| p.can_play || p.can_pause || p.can_go_next || p.can_go_previous)
        .collect();
    players.sort_by(|a, b| a.1.identity.cmp(&b.1.identity));

    if !players.is_empty() {
        content = content.push(widget::divider::horizontal::default());
        content = content.push(
            widget::text(fl!("media-header"))
                .size(14)
                .font(cosmic::font::bold()),
        );

        for (bus_name, player) in players {
            content = content.push(create_media_card(bus_name, player, &spacing));
        }
    }

    let popup_content = widget::container(widget::scrollable(content))
        .width(Length::Fixed(400.0))
        .max_height(700.0)
        .padding(spacing.space_xs);

    // Use the real Core so the popup has proper applet context and theme
    core.applet.popup_container(popup_content).into()
}

fn create_device_card<'a>(
    device: &'a Device,
    spacing: &cosmic::cosmic_theme::Spacing,
    expanded_device: Option<&'a String>,
) -> Element<'a, Message> {
    let is_expanded = expanded_device == Some(&device.id);
    let is_online = device.is_reachable;

    let mut quick_actions_list = widget::list_column();

    let mut menu_items = widget::Column::new();
    let mut name_row = Row::new();

    let phone_icon = widget::icon::from_name("smartphone-symbolic").size(42);

    name_row = name_row.push(phone_icon);

    let mut name_col = Column::new()
        .spacing(spacing.space_s)
        .padding(spacing.space_xxs)
        .push(
            widget::text(&device.name)
                .size(15)
                .font(cosmic::font::bold())
                .width(Length::Fill),
        );

    if !is_online {
        name_col = name_col.push(widget::text::title4(fl!("devices-offline")).size(12));
    } else {
        let mut under_row = widget::Row::new().spacing(spacing.space_xs);

        if let Some(level) = device.battery_level {
            under_row = under_row.push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(widget::icon::from_name(device.battery_icon()).size(16))
                    .push(widget::text(format!("{}%", level)).size(11)),
            );
        }

        if let Some(signal_icon) = device.signal_icon() {
            under_row = under_row.push(widget::icon::from_name(signal_icon).size(16));
        }
        name_col = name_col.push(under_row);
    }

    name_row = name_row.push(name_col).align_y(Alignment::Center);

    name_row = name_row.push(
        widget::button::icon(widget::icon::from_name(if is_expanded {
            "go-up-symbolic"
        } else {
            "go-down-symbolic"
        }))
        .on_press(Message::ToggleDeviceMenu(device.id.clone()))
        .class(cosmic::theme::Button::Icon),
    );

    quick_actions_list = quick_actions_list.add(name_row);

    let quick_action_btn =
        |icon: &str, action: String, msg: messages::Message| -> Element<'_, Message> {
            widget::button::custom(settings::item_row(vec![
                icon::from_name(icon).size(16).icon().into(),
                text::body(action)
                    .width(Length::Fill)
                    .wrapping(Wrapping::Word)
                    .into(),
            ]))
            .width(Length::Fill)
            .on_press(msg)
            .class(theme::Button::Link)
            .into()
        };

    if is_expanded && is_online {
        quick_actions_list = quick_actions_list.add(quick_action_btn(
            "notification-new-symbolic",
            fl!("quick-actions-ping"),
            Message::PingDevice(device.id.clone()),
        ));

        if device.has_findmyphone {
            quick_actions_list = quick_actions_list.add(quick_action_btn(
                "phone-symbolic",
                fl!("quick-actions-find-phone"),
                Message::RingDevice(device.id.clone()),
            ));
        }

        if device.has_clipboard {
            quick_actions_list = quick_actions_list.add(quick_action_btn(
                "edit-paste-symbolic",
                fl!("quick-actions-share-clipboard"),
                Message::ShareClipboard(device.id.clone()),
            ));
        }

        quick_actions_list = quick_actions_list.add(quick_action_btn(
            "mail-message-new-symbolic",
            fl!("quick-actions-sms"),
            Message::SendSMS(device.id.clone()),
        ));

        if device.has_share || device.has_sftp {
            if device.has_share {
                quick_actions_list = quick_actions_list.add(quick_action_btn(
                    "document-send-symbolic",
                    fl!("quick-actions-send-file"),
                    Message::SendFiles(device.id.clone()),
                ));

                if device.share_progress.is_some_and(|p| p > 0) {
                    quick_actions_list =
                        quick_actions_list.add(device.share_progress.map(|progress| {
                            widget::progress_bar::determinate_linear(progress as f32 / 100.0)
                        }));
                }
            }

            if device.has_sftp {
                let mut item_row = Vec::with_capacity(4);
                item_row.push(
                    icon::from_name(if !(device.is_mounted) {
                        "folder-symbolic"
                    } else {
                        "folder-open-symbolic"
                    })
                    .size(16)
                    .icon()
                    .into(),
                );
                item_row.push(
                    text::body(fl!("quick-actions-browse-device"))
                        .width(Length::Fill)
                        .wrapping(Wrapping::Word)
                        .into(),
                );

                if device.is_mounted {
                    item_row.push(horizontal().into());
                    item_row.push(
                        widget::button::icon(icon::from_name("media-eject-symbolic"))
                            .on_press(Message::UnmountDevice(device.id.clone()))
                            .class(cosmic::theme::Button::Link)
                            .into(),
                    );
                }

                quick_actions_list = quick_actions_list.add(
                    widget::button::custom(settings::item_row(item_row))
                        .width(Length::Fill)
                        .on_press(Message::BrowseDevice(device.id.clone()))
                        .class(theme::Button::Link),
                );
            }
        }

        if !device.run_commands.is_empty() {
            for (key, name) in &device.run_commands {
                let key = key.clone();
                quick_actions_list = quick_actions_list.add(quick_action_btn(
                    "system-run-symbolic",
                    name.to_owned(),
                    Message::ExecuteRunCommand(device.id.clone(), key),
                ));
            }
        }
    }

    menu_items = menu_items.push(quick_actions_list);

    menu_items.into()
}

/// One card per active phone media player: album art (if downloaded),
/// title/artist, and prev/play-pause/next controls that call straight
/// through to the player's own MPRIS D-Bus service.
fn create_media_card<'a>(
    bus_name: &'a str,
    player: &'a NowPlaying,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Element<'a, Message> {
    let art: Element<'a, Message> = if let Some(ref path) = player.art_path {
        widget::image(widget::image::Handle::from_path(path))
            .width(Length::Fixed(64.0))
            .height(Length::Fixed(64.0))
            .into()
    } else {
        widget::icon::from_name("audio-x-generic-symbolic")
            .size(64)
            .into()
    };

    let title = player.title.clone();
    let artist = player.artist.clone().unwrap_or_default();

    let mut info_col = widget::Column::new()
        .spacing(2)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    // Always label which app this card controls — with metadata present that's
    // a small caption above the song title; without it (e.g. a browser tab
    // whose media session doesn't report a track) it's the only line shown,
    // so the card never just reads as an empty box.
    if let Some(ref title) = title {
        info_col = info_col.push(widget::text(player.identity.clone()).size(10));
        info_col = info_col.push(
            widget::text(title.clone())
                .size(13)
                .font(cosmic::font::bold()),
        );
    } else {
        info_col = info_col.push(
            widget::text(player.identity.clone())
                .size(13)
                .font(cosmic::font::bold()),
        );
    }
    info_col = info_col.push_maybe((!artist.is_empty()).then(|| widget::text(artist).size(11)));

    let play_icon = if player.is_playing {
        "media-playback-pause-symbolic"
    } else {
        "media-playback-start-symbolic"
    };

    let mut prev_btn =
        widget::button::icon(widget::icon::from_name("media-skip-backward-symbolic"));
    if player.can_go_previous {
        prev_btn = prev_btn.on_press(Message::MprisPrevious(bus_name.to_string()));
    }

    // Some phone-side players (browser tabs in particular) advertise a media
    // session with no working transport controls at all — leave the button
    // disabled rather than pretend it does something.
    let mut play_pause_btn =
        widget::button::icon(widget::icon::from_name(play_icon)).class(cosmic::theme::Button::Icon);
    if player.can_play || player.can_pause {
        play_pause_btn = play_pause_btn.on_press(Message::MprisPlayPause(bus_name.to_string()));
    }

    let mut next_btn = widget::button::icon(widget::icon::from_name("media-skip-forward-symbolic"))
        .class(cosmic::theme::Button::Icon);
    if player.can_go_next {
        next_btn = next_btn.on_press(Message::MprisNext(bus_name.to_string()));
    }

    let controls = Row::new()
        .push(prev_btn)
        .push(play_pause_btn)
        .push(next_btn)
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center);

    let top_row = Row::new()
        .push(art)
        .push(widget::Space::new().width(Length::Fill))
        .push(controls)
        .align_y(Alignment::Center);

    widget::container(
        widget::Column::new()
            .push(top_row)
            .push(info_col)
            .spacing(spacing.space_s),
    )
    .padding(spacing.space_s)
    .class(cosmic::theme::Container::Card)
    .width(Length::Fill)
    .into()
}

/// Builds About page view
pub fn about_view<'a>(core: &'a Core) -> Element<'static, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;

    let mut content = widget::Column::new().padding(spacing.space_xs);

    let back_button = widget::button::custom(settings::item_row(vec![
        icon::from_name("go-previous-symbolic")
            .size(16)
            .icon()
            .into(),
        text::body("Back")
            .width(Length::Fill)
            .wrapping(Wrapping::Word)
            .into(),
    ]))
    .on_press(messages::Message::SwitchPage(messages::Page::Dashboard))
    .class(theme::Button::Link);

    // Header
    content = content.push(back_button);

    // Center area
    // Icon
    content = content.push(
        widget::container(
            widget::icon::from_name("io.github.hepp3n.kdeconnect")
                .prefer_svg(true)
                .size(64),
        )
        .center_x(Length::Fill),
    );
    // Applet name
    content = content.push(
        widget::container(widget::text::title3(fl!("applet-title")).align_x(Alignment::Center))
            .center_x(Length::Fill),
    );
    // Applet author
    content = content.push(
        widget::container(widget::text::caption_heading("heppen").align_x(Alignment::Center))
            .center_x(Length::Fill),
    );

    // Applet version
    content = content.push(
        widget::container(widget::button::standard(APPLET_VERSION))
            .padding([spacing.space_xxs, 0, 0, 0])
            .center_x(Length::Fill),
    );

    // Links section
    content = content
        .push(widget::text::body("Links"))
        .spacing(spacing.space_xxs);

    let links = widget::settings::section()
        .add(
            widget::list::button(
                widget::row::with_capacity(3)
                    .align_y(Alignment::Center)
                    .push(widget::text::body("Repository").width(Length::Fill))
                    .push(widget::icon::from_name("link-symbolic").icon()),
            )
            .on_press(messages::Message::OpenRepository),
        )
        .add(
            widget::list::button(
                widget::row::with_capacity(3)
                    .align_y(Alignment::Center)
                    .push(widget::text::body("Support").width(Length::Fill))
                    .push(widget::icon::from_name("link-symbolic").icon()),
            )
            .on_press(messages::Message::OpenSupport),
        );

    content = content.push(links);

    // License
    content = content
        .push(widget::text::body("License"))
        .spacing(spacing.space_xxs);

    let license = widget::settings::section().add(
        widget::list::button(
            widget::row::with_capacity(3)
                .align_y(Alignment::Center)
                .push(widget::text::body("GPL-3.0 only").width(Length::Fill))
                .push(widget::icon::from_name("link-symbolic").icon()),
        )
        .on_press(messages::Message::OpenLicense),
    );

    content = content.push(license);

    // Description
    content = content.push(widget::text::body(fl!("applet-description")));

    let popup_content = widget::container(widget::scrollable(content))
        .width(Length::Fixed(400.0))
        .max_height(700.0)
        .padding(spacing.space_xs);

    // Use the real Core so the popup has proper applet context and theme
    core.applet.popup_container(popup_content).into()
}
