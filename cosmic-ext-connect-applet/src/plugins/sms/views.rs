//! UI view implementations for the SMS window.

use cosmic::iced::alignment::Horizontal::{self};
use cosmic::iced::widget::scrollable;
use cosmic::iced::{self, Alignment, Length};
use cosmic::widget::space::horizontal;
use cosmic::widget::{self};
use cosmic::{Element, font, theme};

use super::actions::SmsMessage;
use super::app::SmsWindow;
use super::emoji::{EmojiCategory, is_emoji_char};
use super::models::Conversation;
use super::utils::{format_timestamp, phone_numbers_match};

/// Max characters shown in the conversation-list preview before truncating
/// with an ellipsis, so every row takes up the same amount of space
/// regardless of how long the underlying message actually is.
const PREVIEW_MAX_CHARS: usize = 50;

/// Truncates by character count (not bytes, so multi-byte emoji aren't cut
/// mid-codepoint) and appends an ellipsis if anything was cut. Doesn't try
/// to avoid splitting multi-codepoint emoji sequences (e.g. ZWJ-joined
/// family emoji) right at the boundary — a rare, low-stakes cosmetic edge
/// case for a preview string, not worth pulling in a grapheme-segmentation
/// dependency for.
fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

/// Renders text as one paragraph made of spans, switching to `EMOJI_FONT`
/// only for characters classified as emoji so they get a font with color
/// glyphs without forcing the surrounding words onto an emoji-only font
/// (which has no Latin glyphs at all). Built on iced's rich-text/span API
/// instead of a Row of separate Text widgets, so it wraps as a single
/// paragraph rather than overflowing on long messages.
///
/// Takes `s` as a plain borrow with its own short-lived scope and copies
/// each span's text into an owned `String` rather than slicing `s`
/// directly — negligible cost at chat-message length, and it means the
/// caller can pass something built on the fly (e.g. a truncated preview)
/// without that temporary having to outlive the returned `Element`.
fn mixed_emoji_text<'a, M: 'a>(s: &str, size: u16) -> Element<'a, M> {
    use cosmic::iced::widget::text::Span;
    use cosmic::iced::widget::{rich_text, span};

    let mut spans: Vec<Span<'a, (), iced::Font>> = Vec::new();
    let mut run_start = 0;
    let mut run_is_emoji = false;
    let mut first_run = true;

    let push_run =
        |spans: &mut Vec<Span<'a, (), iced::Font>>, start: usize, end: usize, is_emoji: bool| {
            if start == end {
                return;
            }
            let sp = span(s[start..end].to_string());
            spans.push(if is_emoji { sp.font(EMOJI_FONT) } else { sp });
        };

    for (i, ch) in s.char_indices() {
        let is_emoji = is_emoji_char(ch);
        if first_run {
            run_is_emoji = is_emoji;
            first_run = false;
        } else if is_emoji != run_is_emoji {
            push_run(&mut spans, run_start, i, run_is_emoji);
            run_start = i;
            run_is_emoji = is_emoji;
        }
    }
    push_run(&mut spans, run_start, s.len(), run_is_emoji);

    rich_text::<(), M, cosmic::Theme, cosmic::Renderer>(spans)
        .size(size as f32)
        .into()
}

/// Stable ID for the conversations list scrollable, used to scroll it programmatically.
pub static CONVERSATIONS_SCROLLABLE_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(cosmic::widget::Id::unique);

/// ID for the conversion search input, used to focus it when show up
pub static CONVERSATIONS_SEARCH_INPUT_ID: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(cosmic::widget::Id::unique);

/// Thread panel (messages + input)
pub(crate) fn view_thread<'a>(app: &'a SmsWindow, thread_id: String) -> Element<'a, SmsMessage> {
    let Some(conv) = app.conversations.iter().find(|c| c.thread_id == thread_id) else {
        return widget::container(widget::text(fl!("sms-conversation-not-found")).size(14))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    };

    let spacing = cosmic::theme::active().cosmic().spacing;

    let mut content = widget::Column::new();

    // Header
    content = content.push(view_thread_header(app, conv, &spacing));
    content = content.push(widget::divider::horizontal::default());
    // Messages
    content = content.push(view_messages_list(app, &spacing));
    // Input
    content = content.push(view_message_input(app, &spacing));

    widget::container(content)
        .padding(spacing.space_xxs)
        .max_width(1000)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .align_x(Horizontal::Center)
        .into()
}

/// Contacts Tab
pub fn view_contacts(app: &SmsWindow) -> Element<'_, SmsMessage> {
    let spacing = cosmic::theme::active().cosmic().spacing;

    widget::container(
        widget::Column::new()
            .spacing(spacing.space_xxs)
            .push(contacts_header(app))
            .push(contacts_info(app)),
    )
        .padding(spacing.space_xxs)
        .max_width(1000)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .align_x(Horizontal::Center)
        .into()
}

/// Contacts Tab
pub fn contacts_header<'a>(app: &'a SmsWindow) -> Element<'a, SmsMessage> {
    widget::container(
        widget::search_input(fl!("sms-new-chat-prompt"), &app.search_contact_query)
            .on_input(SmsMessage::UpdateSearchContact),
    )
    .width(Length::Fill)
    .into()
}

/// Filter contacts list with searched query
/// Or return whole list
fn filtered_contacts<'a>(app: &'a SmsWindow) -> Vec<(String, String)> {
    app.contacts
        .iter()
        .filter(|c| contact_match_query(c, &app.search_contact_query))
        .map(|c| c.clone())
        .collect()
}

/// Check if given query result in any contact by number/name
fn contact_match_query(contact: &(String, String), query: &str) -> bool {
    contact.0.to_lowercase().contains(&query.to_lowercase())
        || contact.1.to_lowercase().contains(&query.to_lowercase())
}

fn contacts_info<'a>(app: &'a SmsWindow) -> Element<'a, SmsMessage> {
    let mut list = widget::list_column();

    for contact in filtered_contacts(app) {
        let mut section = widget::settings::section();

        let photo = get_contact_photo(app, &contact.0);

        section = section.add(widget::settings::item_row(vec![
            view_contact_avatar(photo, 42.0),
            widget::Column::new()
                .push(widget::text::caption_heading(
                    get_contact_name(app, &contact.0).unwrap_or_default(),
                ))
                .push(widget::text::caption(contact.0.clone()))
                .into(),
            horizontal().into(),
            widget::button::icon(widget::icon::from_name("mail-message-new-symbolic")).into(),
        ]));

        list = list.add(section);
    }

    widget::container(widget::scrollable(list)).into()
}

fn view_thread_header<'a>(
    app: &'a SmsWindow,
    conv: &'a Conversation,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Element<'a, SmsMessage> {
    let photo = get_contact_photo(app, &conv.phone_number);

    let display_name =
        get_contact_name(app, &conv.phone_number).unwrap_or_else(|| conv.phone_number.clone());

    let search_icon = widget::button::icon(widget::icon::from_name("system-search-symbolic"))
        .on_press(SmsMessage::ToggleConversationSearch);

    widget::container(
        widget::column::Column::new()
            .spacing(spacing.space_xs)
            .push(
                widget::Row::new()
                    .spacing(spacing.space_xxs)
                    .push(view_contact_avatar(photo, 40.0))
                    .push(
                        widget::Column::new()
                            .push(widget::text::title4(display_name))
                            .push(widget::text::caption(&conv.phone_number)),
                    )
                    .push(horizontal())
                    .push(search_icon)
                    .align_y(Alignment::Center),
            )
            .push_maybe(if app.search_field_active {
                Some(
                    widget::search_input(fl!("sms-search-placeholder"), &app.conversation_query)
                        .on_input(SmsMessage::ConversationLookup)
                        .id(CONVERSATIONS_SEARCH_INPUT_ID.clone()),
                )
            } else {
                None
            }),
    )
    .padding([0, 0, spacing.space_xs, 0])
    .width(Length::Fill)
    .into()
}

fn view_messages_list<'a>(
    app: &'a SmsWindow,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Element<'a, SmsMessage> {
    let mut messages_column = widget::Column::new()
        .spacing(spacing.space_m)
        .padding(spacing.space_m);

    if app.messages.is_empty() {
        messages_column = messages_column.push(
            widget::container(
                widget::Column::new()
                    .push(widget::text(fl!("sms-waiting-for-messages")).size(14))
                    .push(widget::text(fl!("sms-messages-will-appear")).size(12))
                    .spacing(spacing.space_xs)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding(spacing.space_xl),
        );
    } else {
        if app.conversation_query.len() >= 3 {
            for msg in &app.filtered_messages {
                let position = app
                    .messages
                    .iter()
                    .position(|m| m.id == msg.id)
                    .unwrap_or_default();
                messages_column =
                    messages_column.push(view_message_bubble(app, msg, spacing, position));
            }
        } else {
            for (position, msg) in app.messages.iter().enumerate() {
                messages_column =
                    messages_column.push(view_message_bubble(app, msg, spacing, position));
            }
        }
    }

    widget::scrollable(messages_column)
        .id(CONVERSATIONS_SCROLLABLE_ID.clone())
        .height(Length::Fill)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new().anchor(scrollable::Anchor::End),
        ))
        .into()
}

/// Renders one MMS attachment. Three states, and — unlike before — none
/// of them render nothing, since a thumbnail-less attachment used to
/// produce a blank bubble (the bug this replaces):
/// - already downloaded + image → the full-resolution image (loaded by
///   path, so iced caches it instead of us re-reading the file), plus a
///   Save button to copy it out via the native "Save As" dialog
/// - already downloaded + video → an "Open" button (iced can't render
///   video inline) alongside the same Save button
/// - not downloaded yet → the thumbnail preview if the phone sent one,
///   otherwise a generic photo/video placeholder card — either way
///   clickable to request the full file, *if* the phone gave it a
///   `unique_identifier` to request by. Video in particular routinely has
///   no thumbnail at all in this protocol, so the placeholder path is the
///   common case for video, not a rare fallback.
fn view_attachment<'a>(
    attachment: &'a super::models::MessageAttachment,
) -> Option<Element<'a, SmsMessage>> {
    if let Some(path) = &attachment.full_path {
        let save_button =
            widget::button::icon(widget::icon::from_name("document-save-symbolic").handle())
                .on_press(SmsMessage::SaveAttachment(path.clone()));

        return Some(if attachment.is_video() {
            widget::Row::new()
                .push(
                    widget::button::standard(fl!("sms-open-attachment"))
                        .on_press(SmsMessage::OpenAttachment(path.clone())),
                )
                .push(save_button)
                .spacing(4)
                .into()
        } else {
            widget::Column::new()
                .push(
                    widget::image(cosmic::widget::image::Handle::from_path(path))
                        .width(Length::Fixed(220.0)),
                )
                .push(
                    widget::Row::new()
                        .push(widget::space::horizontal())
                        .push(save_button),
                )
                .spacing(4)
                .into()
        });
    }

    let preview: Element<'a, SmsMessage> = match attachment.thumbnail.as_deref() {
        Some(thumbnail) if !thumbnail.is_empty() => widget::image(
            cosmic::widget::image::Handle::from_bytes(thumbnail.to_vec()),
        )
        .width(Length::Fixed(220.0))
        .into(),
        _ => view_attachment_placeholder(attachment),
    };

    Some(match &attachment.unique_identifier {
        Some(uid) => widget::mouse_area(preview)
            .on_press(SmsMessage::RequestFullAttachment {
                part_id: attachment.part_id,
                unique_identifier: uid.clone(),
            })
            .into(),
        None => preview,
    })
}

/// Generic icon + label card shown in place of a thumbnail when the phone
/// didn't send one. Video routinely has no thumbnail in this protocol at
/// all, so this is the normal video appearance, not a rare error state.
fn view_attachment_placeholder<'a>(
    attachment: &super::models::MessageAttachment,
) -> Element<'a, SmsMessage> {
    let (icon_name, label) = if attachment.is_video() {
        ("video-x-generic-symbolic", fl!("sms-attachment-video"))
    } else if attachment.mime_type.starts_with("image/") {
        ("image-x-generic-symbolic", fl!("sms-attachment-photo"))
    } else {
        ("mail-attachment-symbolic", fl!("sms-attachment-generic"))
    };

    widget::container(
        widget::Column::new()
            .push(widget::icon::from_name(icon_name).icon().size(48))
            .push(widget::text(label).size(12))
            .spacing(4)
            .align_x(Alignment::Center),
    )
    .width(Length::Fixed(220.0))
    .padding(16)
    .align_x(Alignment::Center)
    .class(cosmic::theme::Container::List)
    .into()
}

fn view_message_bubble<'a>(
    app: &'a SmsWindow,
    msg: &'a super::models::Message,
    spacing: &cosmic::cosmic_theme::Spacing,
    position: usize,
) -> Element<'a, SmsMessage> {
    let is_sent = msg.is_sent();
    let mut message_content = widget::Column::new().spacing(spacing.space_xxs);

    // Show sender label only for received messages
    if !is_sent {
        let phone_number =
            get_current_conversation_phone(app).unwrap_or_else(|| msg.address.clone());

        let sender_label = get_contact_name(app, &phone_number).unwrap_or(phone_number);

        message_content = message_content.push(widget::text(sender_label).font(font::bold()));
    }

    for attachment in &msg.attachments {
        if let Some(element) = view_attachment(attachment) {
            message_content = message_content.push(element);
        }
    }

    message_content = message_content
        .push(mixed_emoji_text(&msg.body, 14))
        .push(widget::container(
            widget::row::Row::new()
                .align_y(Alignment::Center)
                .push_maybe(if app.search_field_active {
                    Some(
                        widget::button::icon(widget::icon::from_name("go-jump-symbolic"))
                            .on_press(SmsMessage::ScrolltoMessage(position)),
                    )
                } else {
                    None
                })
                .push(widget::text(format_timestamp(msg.date)).size(11)),
        ))
        .padding(spacing.space_s);

    let message_bubble = if is_sent {
        widget::container(message_content)
            .class(cosmic::theme::Container::custom(move |theme| {
                iced::widget::container::Style {
                    background: Some(iced::Background::Color(
                        theme.cosmic().accent_color().into(),
                    )),
                    border: iced::Border {
                        radius: iced::Radius::from(8.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }))
            .max_width(500)
    } else {
        widget::container(message_content)
            .class(cosmic::theme::Container::custom(move |theme| {
                iced::widget::container::Style {
                    background: Some(iced::Background::Color(
                        theme.cosmic().primary_component_color().into(),
                    )),
                    border: iced::Border {
                        radius: iced::Radius::from(8.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }))
            .max_width(500)
    };

    if is_sent {
        widget::Row::new()
            .push(widget::space::horizontal())
            .push(message_bubble)
            .width(Length::Fill)
            .into()
    } else {
        widget::Row::new()
            .push(message_bubble)
            .width(Length::Fill)
            .into()
    }
}

fn view_message_input<'a>(
    app: &'a SmsWindow,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Element<'a, SmsMessage> {
    let message_placeholder = fl!("sms-message-placeholder");

    let input_row = widget::Row::new()
        .push(
            widget::button::icon(widget::icon::from_name("face-smile-symbolic").handle())
                .on_press(SmsMessage::ToggleEmojiPicker),
        )
        .push(
            widget::button::icon(widget::icon::from_name("mail-attachment-symbolic").handle())
                .on_press(SmsMessage::PickAttachment),
        )
        .push(
            widget::text_input(message_placeholder, &app.message_input)
                .on_input(SmsMessage::UpdateInput)
                .on_submit(|_| SmsMessage::SendMessage)
                .width(Length::Fill),
        )
        .push(widget::button::suggested(fl!("sms-send")).on_press(SmsMessage::SendMessage))
        .spacing(spacing.space_xxs)
        .align_y(Alignment::Center);

    let mut col = widget::Column::new().spacing(spacing.space_xs);

    if app.show_emoji_picker {
        col = col.push(view_emoji_picker(app, spacing));
    }
    if !app.pending_attachments.is_empty() {
        col = col.push(view_pending_attachments(app, spacing));
    }

    col = col.push(input_row);

    widget::container(col)
        .padding(spacing.space_xxs)
        .class(theme::Container::Primary)
        .into()
}

/// Staged outgoing attachments, shown as a row of removable chips above
/// the compose bar. Just the filename — no preview thumbnail, since these
/// are local files we haven't sent yet (not worth a thumbnail decode for
/// something this transient).
fn view_pending_attachments<'a>(
    app: &'a SmsWindow,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Element<'a, SmsMessage> {
    let mut row = widget::Row::new().spacing(spacing.space_xs);
    for (index, path) in app.pending_attachments.iter().enumerate() {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());

        row = row.push(
            widget::container(
                widget::Row::new()
                    .push(widget::text(name).size(12))
                    .push(
                        widget::button::icon(
                            widget::icon::from_name("window-close-symbolic").handle(),
                        )
                        .on_press(SmsMessage::RemovePendingAttachment(index)),
                    )
                    .spacing(spacing.space_xxs)
                    .align_y(Alignment::Center),
            )
            .class(cosmic::theme::Container::Secondary)
            .padding(spacing.space_xxs),
        );
    }
    widget::scrollable(row)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new(),
        ))
        .into()
}

/// Emoji picker panel: category tabs + a scrollable grid of emoji for the
/// selected category. Stays open after inserting an emoji so multiple can
/// be picked in a row.
// cosmic-text's font fallback can resolve a handful of codepoints (✈️⚽💡❤️,
// some smileys) to a non-color font that happens to also cover them, instead
// of the color emoji font, which renders them as outlines tinted by the
// button's text color. Pinning the glyph to the color emoji font avoids that.
const EMOJI_FONT: iced::Font = iced::Font::with_name("Noto Color Emoji");

fn view_emoji_picker<'a>(
    app: &'a SmsWindow,
    spacing: &cosmic::cosmic_theme::Spacing,
) -> Element<'a, SmsMessage> {
    let mut tabs = widget::Row::new()
        .spacing(spacing.space_xxs)
        .width(Length::Fill);
    for category in EmojiCategory::all() {
        let is_active = app.emoji_category == category;
        let tab = widget::button::custom(
            widget::text(category.label())
                .font(EMOJI_FONT)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        )
        .padding(spacing.space_xxs)
        .width(Length::Fill)
        .on_press(SmsMessage::SelectEmojiCategory(category));
        tabs = tabs.push(if is_active {
            tab.class(cosmic::theme::Button::Suggested)
        } else {
            tab.class(cosmic::theme::Button::Text)
        });
    }

    let mut grid = widget::Column::new().spacing(spacing.space_xxs);
    for row_emojis in app.emoji_category.emojis().chunks(8) {
        let mut row = widget::Row::new()
            .spacing(spacing.space_xxs)
            .width(Length::Fill);
        for emoji in row_emojis {
            row = row.push(
                widget::button::custom(
                    widget::text(*emoji)
                        .font(EMOJI_FONT)
                        .width(Length::Fill)
                        .align_x(Alignment::Center),
                )
                .padding(spacing.space_xxs)
                .width(Length::Fill)
                .on_press(SmsMessage::InsertEmoji(emoji.to_string())),
            );
        }
        grid = grid.push(row);
    }

    widget::container(
        widget::Column::new()
            .spacing(spacing.space_xs)
            .padding(spacing.space_s)
            .push(tabs)
            .push(widget::divider::horizontal::light())
            .push(
                widget::scrollable(grid.width(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fixed(160.0)),
            ),
    )
    .class(cosmic::theme::Container::Card)
    .width(Length::Fill)
    .into()
}

// Helper functions

fn get_contact_name(app: &SmsWindow, phone_number: &str) -> Option<String> {
    app.contacts
        .iter()
        .find(|(contact_phone, _)| phone_numbers_match(phone_number, contact_phone))
        .map(|(_, name)| name.clone())
}

fn get_contact_photo<'a>(
    app: &'a SmsWindow,
    phone_number: &str,
) -> Option<&'a super::avatar::Avatar> {
    app.contact_photos
        .iter()
        .find(|(contact_phone, _)| phone_numbers_match(phone_number, contact_phone))
        .map(|(_, photo)| photo)
}

/// Avatar for a contact: their photo if we have one, otherwise a generic
/// placeholder — never blank, never square. The photo's roundness is
/// baked into its pixels already (see `avatar::make_circular`) rather
/// than relying on container clipping, which didn't actually mask image
/// content to a rounded shape in testing. The placeholder reuses the
/// same background+radius technique as the unread-conversation dot
/// elsewhere in this file, which *is* proven to render rounded — a
/// rounded quad with no background/border to actually show is why the
/// placeholder looked square before.
fn view_contact_avatar<'a>(
    photo: Option<&'a super::avatar::Avatar>,
    size: f32,
) -> Element<'a, SmsMessage> {
    if let Some(avatar) = photo {
        return widget::image(cosmic::widget::image::Handle::from_rgba(
            avatar.width,
            avatar.height,
            avatar.rgba.clone(),
        ))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into();
    }

    widget::container(
        widget::icon::from_name("avatar-default-symbolic")
            .icon()
            .size((size * 0.6) as u16),
    )
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .class(cosmic::theme::Container::custom(move |theme| {
        iced::widget::container::Style {
            background: Some(iced::Background::Color(
                theme.cosmic().bg_component_color().into(),
            )),
            border: iced::Border {
                radius: iced::Radius::from(size / 2.0),
                ..Default::default()
            },
            ..Default::default()
        }
    }))
    .into()
}

fn get_current_conversation_phone(app: &SmsWindow) -> Option<String> {
    let thread_id = app.selected_thread.as_ref()?;
    app.conversations
        .iter()
        .find(|c| c.thread_id == *thread_id)
        .map(|c| c.phone_number.clone())
}

/// True if this conversation should show the unread indicator. Once a
/// thread has been opened in this app session, the phone's own read flag
/// is ignored in favor of comparing against the last message timestamp
/// the user actually saw — there's no protocol way to write "read" back
/// to the phone, so mirroring its flag forever would mean the badge never
/// clears just because you read it here. For threads never opened this
/// session, falls back to the phone-reported flag as a reasonable guess.
fn is_conversation_unread(app: &SmsWindow, conv: &Conversation) -> bool {
    match app.last_seen_timestamp.get(&conv.thread_id) {
        Some(&seen_at) => conv.timestamp > seen_at,
        None => conv.unread,
    }
}
