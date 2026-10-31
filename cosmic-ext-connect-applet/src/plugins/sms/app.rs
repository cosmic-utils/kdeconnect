use async_stream::stream;
use cosmic::iced::widget::scrollable;
use cosmic::{
    Action, Application, ApplicationExt, Element, Task,
    app::Core,
    iced::{Length, Subscription},
    widget,
};
use futures::StreamExt;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

use super::actions::SmsMessage;
use super::avatar::{self, Avatar};
use super::dbus;
use super::emoji::EmojiCategory;
use super::models::{Conversation, Message, MessageAttachment, ProtocolEvent};
use super::utils;
use super::views;

pub struct SmsWindow {
    core: Core,
    pub device_id: String,
    #[allow(dead_code)]
    pub device_name: String,
    pub conversations: Vec<Conversation>,
    pub contacts: Vec<(String, String)>,
    /// Phone -> baked circular avatar, parsed from vcard PHOTO
    /// properties. Missing entry (not an empty one) means "no photo
    /// available, show the default placeholder" — see
    /// `views::view_contact_avatar`.
    pub contact_photos: HashMap<String, Avatar>,
    pub selected_thread: Option<String>,
    pub contact_idx: Option<usize>,
    pub messages: Vec<Message>,
    pub message_input: String,
    pub search_query: String,
    pub show_new_chat_dialog: bool,
    pub new_chat_phone_input: String,
    pub show_emoji_picker: bool,
    pub emoji_category: EmojiCategory,
    /// Thread IDs hidden via the delete flow — see hidden_conversations docs.
    pub hidden_conversations: std::collections::HashSet<String>,
    /// Thread ID pending delete confirmation, if the dialog is open.
    pub pending_delete_thread: Option<String>,
    /// Per-thread timestamp of the latest message the user has actually
    /// viewed in this app session. Drives the unread indicator instead of
    /// the phone's own read flag, since there's no protocol way to write
    /// "read" back to the phone — see SelectThread and MessageReceived for
    /// where this gets updated, and is_conversation_unread in views.rs for
    /// how it's combined with the phone-reported flag as a bootstrap value
    /// for threads never opened this session.
    pub last_seen_timestamp: HashMap<String, i64>,
    /// File paths staged via the attach button, sent (and cleared) with
    /// the next outgoing message.
    pub pending_attachments: Vec<String>,
}

impl Application for SmsWindow {
    type Executor = cosmic::executor::Default;
    type Flags = (String, String);
    type Message = SmsMessage;
    const APP_ID: &'static str = "io.github.hepp3n.kdeconnect.sms";

    fn core(&self) -> &Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Self::Flags) -> (Self, Task<Action<Self::Message>>) {
        let (device_id, device_name) = flags;
        info!("SMS window init device_id={}", device_id);

        let mut app = Self {
            core,
            device_id: device_id.clone(),
            device_name: device_name.clone(),
            conversations: Vec::new(),
            contacts: Vec::new(),
            contact_photos: HashMap::new(),
            selected_thread: None,
            contact_idx: Some(0),
            messages: Vec::new(),
            message_input: String::new(),
            search_query: String::new(),
            show_new_chat_dialog: false,
            new_chat_phone_input: String::new(),
            show_emoji_picker: false,
            emoji_category: EmojiCategory::Smileys,
            hidden_conversations: kdeconnect_core::hidden_conversations::load_hidden(&device_id),
            pending_delete_thread: None,
            last_seen_timestamp: kdeconnect_core::sms_read_state::load_last_seen(&device_id),
            pending_attachments: Vec::new(),
        };

        let title = fl!("sms-window-title", device = device_name.as_str());
        app.core.window.header_title = title.clone().into();

        let title_task = app.set_window_title(title, app.core.main_window_id().unwrap());

        (app, title_task)
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let device_id = self.device_id.clone();

        Subscription::batch(vec![
            // Safety net for state that only changes on the phone with no
            // corresponding push (e.g. reading a message there) — see the
            // read-state discussion this was added for. Reuses
            // LoadConversations, the same fire-and-forget request already
            // used at startup/reconnect; RefreshThread (its follow-up) is a
            // no-op, and the actual data still flows back through the same
            // event-stream path with its existing dedup/merge logic, so this
            // can't disrupt an open thread's scroll position or message list.
            // 45s: frequent enough to not feel stale, infrequent enough not
            // to nag the phone with requests it has to hit its SMS DB for.
            cosmic::iced::time::every(std::time::Duration::from_secs(45))
                .map(|_| SmsMessage::LoadConversations),
            Subscription::run_with(device_id, |device_id| {
                let device_id = device_id.clone();
                stream! {
                    info!("SMS event stream started for device={}", device_id);

                    if let Err(e) = dbus::initialize().await {
                        error!("SMS D-Bus init failed: {:?}", e);
                        std::future::pending::<()>().await;
                        return;
                    }

                    let Some(client) = dbus::get_client().await else {
                        warn!("SMS D-Bus no client available, stream idle");
                        std::future::pending::<()>().await;
                        return;
                    };

                    debug!("SMS event loop entering");

                    let cached_contacts = dbus::get_cached_contacts(&device_id).await;
                    if !cached_contacts.is_empty() {
                        debug!("yielding {} cached contacts at startup", cached_contacts.len());
                        yield SmsMessage::ContactsLoaded(cached_contacts);
                    }

                    let cached_photos = dbus::get_cached_contact_photos(&device_id).await;
                    if !cached_photos.is_empty() {
                        debug!("yielding {} cached contact photos at startup", cached_photos.len());
                        yield SmsMessage::ContactPhotosLoaded(cached_photos);
                    }

                    if let Some(cached_json) = dbus::get_cached_sms(&device_id).await {
                        debug!("yielding cached SMS at startup");
                        let (messages, conversations) = dbus::parse_sms_messages(&cached_json);
                        for msg in messages {
                            yield SmsMessage::ProtocolEventReceived(ProtocolEvent::MessageReceived(msg));
                        }
                        yield SmsMessage::ProtocolEventReceived(ProtocolEvent::ConversationsReceived(conversations));
                    }

                    loop {
                        debug!("SMS subscribing to events");
                        let mut event_stream = match client.listen_for_events().await {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("Failed to subscribe to SMS event stream: {:?}", e);
                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                continue;
                            }
                        };

                        // Subscribe FIRST, then request — contacts response is a
                        // fire-and-forget D-Bus signal; if we request before subscribing
                        // the signal arrives while nobody is listening and is lost.
                        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                        dbus::fetch_conversations(&device_id).await;
                        dbus::fetch_contacts(&device_id).await;

                        while let Some(event) = event_stream.next().await {
                            use kdeconnect_dbus_client::ServiceEvent;
                            match event {
                                ServiceEvent::SmsMessagesReceived(json) => {
                                    debug!("SmsMessagesReceived len={}", json.len());
                                    let (messages, conversations) = dbus::parse_sms_messages(&json);
                                    for msg in messages {
                                        yield SmsMessage::ProtocolEventReceived(
                                            ProtocolEvent::MessageReceived(msg)
                                        );
                                    }
                                    yield SmsMessage::ProtocolEventReceived(
                                        ProtocolEvent::ConversationsReceived(conversations)
                                    );
                                }
                                ServiceEvent::ContactsReceived(contacts) => {
                                    debug!("ContactsReceived {} entries", contacts.len());
                                    yield SmsMessage::ContactsLoaded(contacts);
                                }
                                ServiceEvent::SmsAttachmentReceived(filename, path) => {
                                    debug!("SmsAttachmentReceived {} -> {}", filename, path);
                                    yield SmsMessage::AttachmentReceived(filename, path.into());
                                }
                                ServiceEvent::ContactPhotosReceived(photos) => {
                                    debug!("ContactPhotosReceived {} entries", photos.len());
                                    let decoded: HashMap<String, Vec<u8>> = photos
                                        .into_iter()
                                        .filter_map(|(phone, b64)| {
                                            kdeconnect_core::contacts::decode_photo(&b64)
                                                .map(|bytes| (phone, bytes))
                                        })
                                        .collect();
                                    if !decoded.is_empty() {
                                        yield SmsMessage::ContactPhotosLoaded(decoded);
                                    }
                                }
                                _ => {}
                            }
                        }

                        warn!("SMS event stream ended, reconnecting in 1s");
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<Action<Self::Message>> {
        match message {
            SmsMessage::LoadConversations => {
                let device_id = self.device_id.clone();
                return cosmic::task::future(async move {
                    dbus::fetch_conversations(&device_id).await;
                    Action::App(SmsMessage::RefreshThread)
                });
            }
            SmsMessage::ConversationsLoaded(conversations) => {
                debug!("ConversationsLoaded: {}", conversations.len());
                self.conversations = conversations;
                self.update_conversation_names();
            }
            SmsMessage::ContactsLoaded(contacts) => {
                debug!("ContactsLoaded: {} contacts", contacts.len());
                let sorted = utils::sort_cached_contacts(contacts);
                self.contacts = sorted;
                self.update_conversation_names();
            }
            SmsMessage::ContactPhotosLoaded(photos) => {
                debug!(
                    "ContactPhotosLoaded: {} photos, baking off the UI thread",
                    photos.len()
                );
                return cosmic::task::future(async move {
                    let baked = tokio::task::spawn_blocking(move || {
                        photos
                            .into_iter()
                            .filter_map(|(phone, raw)| {
                                avatar::make_circular(&raw, avatar::BAKE_SIZE).map(|a| (phone, a))
                            })
                            .collect::<HashMap<String, Avatar>>()
                    })
                    .await
                    .unwrap_or_else(|e| {
                        warn!("avatar baking task panicked: {}", e);
                        HashMap::new()
                    });
                    Action::App(SmsMessage::AvatarsBaked(baked))
                });
            }
            SmsMessage::AvatarsBaked(baked) => {
                debug!("AvatarsBaked: {} avatars", baked.len());
                self.contact_photos.extend(baked);
            }
            SmsMessage::RequestFullAttachment {
                part_id,
                unique_identifier,
            } => {
                debug!("RequestFullAttachment part_id={}", part_id);
                let device_id = self.device_id.clone();
                return cosmic::task::future(async move {
                    dbus::request_sms_attachment(&device_id, part_id, &unique_identifier).await;
                    Action::None
                });
            }
            SmsMessage::AttachmentReceived(filename, path) => {
                debug!("AttachmentReceived {} -> {:?}", filename, path);
                for msg in &mut self.messages {
                    for attachment in &mut msg.attachments {
                        if attachment.unique_identifier.as_deref() == Some(filename.as_str()) {
                            attachment.full_path = Some(path.clone());
                        }
                    }
                }
            }
            SmsMessage::OpenAttachment(path) => {
                debug!("OpenAttachment {:?}", path);
                return cosmic::task::future(async move {
                    if let Err(e) = tokio::process::Command::new("xdg-open").arg(&path).spawn() {
                        warn!("failed to open attachment {:?}: {}", path, e);
                    }
                    Action::None
                });
            }
            SmsMessage::SaveAttachment(path) => {
                debug!("SaveAttachment {:?}", path);
                return cosmic::task::future(async move {
                    let suggested_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "attachment".to_string());

                    let Some(dest) =
                        crate::portal::save_file(fl!("sms-save-attachment-title"), suggested_name)
                            .await
                    else {
                        debug!("save attachment cancelled");
                        return Action::None;
                    };
                    let dest_path = std::path::PathBuf::from(dest);

                    match tokio::fs::copy(&path, &dest_path).await {
                        Ok(_) => {
                            info!("saved attachment to {:?}", dest_path);
                            let dest_display = dest_path.display().to_string();
                            tokio::task::spawn_blocking(move || {
                                let _ = notify_rust::Notification::new()
                                    .appname("KDE Connect")
                                    .summary(&fl!("sms-save-success-summary"))
                                    .body(&dest_display)
                                    .icon("emblem-ok-symbolic")
                                    .show();
                            });
                        }
                        Err(e) => {
                            warn!("failed to save attachment to {:?}: {}", dest_path, e);
                            tokio::task::spawn_blocking(move || {
                                let _ = notify_rust::Notification::new()
                                    .appname("KDE Connect")
                                    .summary(&fl!("sms-save-failed-summary"))
                                    .icon("dialog-error-symbolic")
                                    .show();
                            });
                        }
                    }
                    Action::None
                });
            }
            SmsMessage::PickAttachment => {
                return cosmic::task::future(async move {
                    let paths =
                        crate::portal::pick_files(fl!("sms-attach-picker-title"), true, None).await;
                    Action::App(SmsMessage::AttachmentsPicked(paths))
                });
            }
            SmsMessage::AttachmentsPicked(paths) => {
                debug!("AttachmentsPicked: {} file(s)", paths.len());
                self.pending_attachments.extend(paths);
            }
            SmsMessage::RemovePendingAttachment(index) => {
                if index < self.pending_attachments.len() {
                    self.pending_attachments.remove(index);
                }
            }
            SmsMessage::SelectThread(thread_id) => {
                debug!("SelectThread: {}", thread_id);
                if let Some(conv) = self.conversations.iter().find(|c| c.thread_id == thread_id) {
                    self.last_seen_timestamp
                        .insert(thread_id.clone(), conv.timestamp);
                }
                self.selected_thread = Some(thread_id.clone());
                self.messages.clear();
                let device_id = self.device_id.clone();
                let device_id2 = device_id.clone();
                let last_seen = self.last_seen_timestamp.clone();
                return Task::batch([
                    cosmic::task::future(async move {
                        dbus::request_conversation_messages(&device_id, &thread_id).await;
                        Action::App(SmsMessage::RefreshThread)
                    }),
                    cosmic::task::future(async move {
                        kdeconnect_core::sms_read_state::save_last_seen(&device_id2, &last_seen)
                            .await;
                        Action::None
                    }),
                ]);
            }
            SmsMessage::UpdateInput(input) => {
                self.message_input = input;
            }
            SmsMessage::UpdateSearch(query) => {
                self.search_query = query;
            }
            SmsMessage::SendMessage => {
                if self.message_input.trim().is_empty() && self.pending_attachments.is_empty() {
                    return Task::none();
                }
                let Some(thread_id) = self.selected_thread.clone() else {
                    return Task::none();
                };
                let Some(conv) = self.conversations.iter().find(|c| c.thread_id == thread_id)
                else {
                    return Task::none();
                };

                let device_id = self.device_id.clone();
                let phone = conv.phone_number.clone();
                let text = self.message_input.clone();
                let attachments = std::mem::take(&mut self.pending_attachments);
                let now = utils::now_millis();

                // We already have these files locally, so show them
                // immediately rather than waiting on a server round-trip —
                // this also covers the case where the phone never echoes
                // back a thumbnail for our own outgoing attachment.
                let echo_attachments: Vec<MessageAttachment> = attachments
                    .iter()
                    .map(|path| {
                        let path = std::path::PathBuf::from(path);
                        let mime_type = kdeconnect_core::plugins::sms::guess_mime_type(&path);
                        MessageAttachment {
                            part_id: 0,
                            unique_identifier: None,
                            mime_type,
                            thumbnail: None,
                            full_path: Some(path),
                        }
                    })
                    .collect();

                self.messages.push(Message {
                    id: format!("sending_{}", now),
                    thread_id: thread_id.clone(),
                    body: text.clone(),
                    address: phone.clone(),
                    date: now,
                    attachments: echo_attachments,
                    type_: 2,
                    read: true,
                });
                self.messages.sort_by_key(|m| m.date);
                self.message_input.clear();

                // Update the conversation preview and timestamp so it sorts to
                // the top of the list immediately without waiting for a server refresh.
                if let Some(conv) = self
                    .conversations
                    .iter_mut()
                    .find(|c| c.thread_id == thread_id)
                {
                    conv.last_message = text.clone();
                    conv.timestamp = now;
                }
                self.conversations
                    .sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

                // Scroll the conversation list to the top so the moved item is visible.
                let scroll_task = scrollable::scroll_to(
                    views::CONVERSATIONS_SCROLLABLE_ID.clone(),
                    scrollable::AbsoluteOffset {
                        x: Some(0.0),
                        y: Some(0.0),
                    },
                );

                return Task::batch(vec![
                    scroll_task.map(|_: cosmic::widget::Id| Action::App(SmsMessage::RefreshThread)),
                    cosmic::task::future(async move {
                        dbus::send_sms(&device_id, &phone, &text, attachments).await;
                        Action::App(SmsMessage::RefreshThread)
                    }),
                ]);
            }
            SmsMessage::RefreshThread => {}
            SmsMessage::ProtocolEventReceived(event) => {
                debug!(
                    "ProtocolEventReceived: {:?}",
                    std::mem::discriminant(&event)
                );
                self.handle_protocol_event(event);
            }
            SmsMessage::OpenNewChatDialog => {
                self.show_new_chat_dialog = true;
            }
            SmsMessage::CloseNewChatDialog => {
                self.show_new_chat_dialog = false;
                self.new_chat_phone_input.clear();
            }
            SmsMessage::UpdateNewChatPhone(phone) => {
                self.new_chat_phone_input = phone;
            }
            SmsMessage::SelectContactForNewChat(idx) => {
                self.new_chat_phone_input = self.contacts[idx].0.clone();
                self.contact_idx = Some(idx);
            }
            SmsMessage::CreateNewChat => {
                let phone = self.new_chat_phone_input.trim().to_string();
                if !phone.is_empty() {
                    let thread_id = format!("new_{}", utils::now_millis());
                    self.conversations.insert(
                        0,
                        Conversation {
                            thread_id: thread_id.clone(),
                            phone_number: phone,
                            contact_name: String::new(),
                            last_message: String::new(),
                            timestamp: utils::now_millis(),
                            unread: false,
                        },
                    );
                    self.show_new_chat_dialog = false;
                    self.new_chat_phone_input.clear();
                    return cosmic::task::message(Action::App(SmsMessage::SelectThread(thread_id)));
                }
            }
            SmsMessage::CloseWindow => std::process::exit(0),
            SmsMessage::ToggleEmojiPicker => {
                self.show_emoji_picker = !self.show_emoji_picker;
            }
            SmsMessage::SelectEmojiCategory(category) => {
                self.emoji_category = category;
            }
            SmsMessage::InsertEmoji(emoji) => {
                self.message_input.push_str(&emoji);
            }
            SmsMessage::RequestDeleteConversation(thread_id) => {
                self.pending_delete_thread = Some(thread_id);
            }
            SmsMessage::CancelDeleteConversation => {
                self.pending_delete_thread = None;
            }
            SmsMessage::ConfirmDeleteConversation => {
                let Some(thread_id) = self.pending_delete_thread.take() else {
                    return Task::none();
                };

                self.conversations.retain(|c| c.thread_id != thread_id);
                self.hidden_conversations.insert(thread_id.clone());

                if self.selected_thread.as_deref() == Some(thread_id.as_str()) {
                    self.selected_thread = None;
                    self.messages.clear();
                }

                let device_id = self.device_id.clone();
                let hidden = self.hidden_conversations.clone();
                return cosmic::task::future(async move {
                    kdeconnect_core::hidden_conversations::save_hidden(&device_id, &hidden).await;
                    Action::None
                });
            }
        }
        Task::none()
    }

    fn dialog(&self) -> Option<Element<'_, Self::Message>> {
        self.pending_delete_thread
            .as_ref()
            .map(|_| self.delete_confirm_dialog())
    }

    fn view(&self) -> Element<'_, Self::Message> {
        widget::container(views::view_main(self))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::Alignment::Center)
            .into()
    }
}

impl SmsWindow {
    /// Builds the delete-confirmation dialog shown via `Application::dialog()`.
    fn delete_confirm_dialog(&self) -> Element<'_, SmsMessage> {
        let display_name = self
            .pending_delete_thread
            .as_ref()
            .and_then(|tid| self.conversations.iter().find(|c| &c.thread_id == tid))
            .map(|c| {
                self.contacts
                    .iter()
                    .find(|(phone, _)| utils::phone_numbers_match(&c.phone_number, phone))
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| c.phone_number.clone())
            })
            .unwrap_or_default();

        widget::dialog()
            .title(fl!("sms-delete-confirm-title"))
            .body(fl!("sms-delete-confirm-body", name = display_name.as_str()))
            .primary_action(
                widget::button::destructive(fl!("sms-delete-confirm-action"))
                    .on_press(SmsMessage::ConfirmDeleteConversation),
            )
            .secondary_action(
                widget::button::standard(fl!("sms-delete-confirm-cancel"))
                    .on_press(SmsMessage::CancelDeleteConversation),
            )
            .into()
    }

    fn handle_protocol_event(&mut self, event: ProtocolEvent) {
        match event {
            ProtocolEvent::ConversationsReceived(conversations) => {
                debug!(
                    "ConversationsReceived: {} conversations",
                    conversations.len()
                );

                // Capture selected new_* phone BEFORE we mutate merged
                let pending_new_phone: Option<String> = self
                    .selected_thread
                    .as_ref()
                    .filter(|t| t.starts_with("new_"))
                    .and_then(|sel| self.conversations.iter().find(|c| c.thread_id == *sel))
                    .map(|c| c.phone_number.clone());

                let mut merged = self.conversations.clone();
                for incoming in &conversations {
                    if incoming.thread_id.starts_with("new_") {
                        continue;
                    }

                    if let Some(pos) = merged.iter().position(|c| {
                        c.thread_id.starts_with("new_")
                            && utils::phone_numbers_match(&c.phone_number, &incoming.phone_number)
                    }) {
                        merged[pos] = incoming.clone();
                    } else if let Some(existing) = merged
                        .iter_mut()
                        .find(|c| c.thread_id == incoming.thread_id)
                    {
                        *existing = incoming.clone();
                    } else {
                        merged.push(incoming.clone());
                    }
                }

                merged.retain(|c| {
                    if !c.thread_id.starts_with("new_") {
                        return true;
                    }
                    !conversations
                        .iter()
                        .any(|r| utils::phone_numbers_match(&r.phone_number, &c.phone_number))
                });

                self.conversations = merged;
                self.conversations
                    .retain(|c| !self.hidden_conversations.contains(&c.thread_id));
                self.update_conversation_names();

                // If we had a new_* selected, find its real thread by phone number now
                if let Some(phone) = pending_new_phone {
                    if let Some(real) = self.conversations.iter().find(|c| {
                        !c.thread_id.starts_with("new_")
                            && utils::phone_numbers_match(&c.phone_number, &phone)
                    }) {
                        self.selected_thread = Some(real.thread_id.clone());
                    }
                }
            }
            ProtocolEvent::MessageReceived(message) => {
                debug!("MessageReceived thread={}", message.thread_id);

                if self.hidden_conversations.contains(&message.thread_id) {
                    return;
                }

                let is_selected = self.selected_thread.as_deref() == Some(&message.thread_id);

                if is_selected {
                    let seen = self
                        .last_seen_timestamp
                        .entry(message.thread_id.clone())
                        .or_insert(0);
                    *seen = (*seen).max(message.date);

                    let device_id = self.device_id.clone();
                    let last_seen = self.last_seen_timestamp.clone();
                    tokio::spawn(async move {
                        kdeconnect_core::sms_read_state::save_last_seen(&device_id, &last_seen)
                            .await;
                    });

                    let already_exists = self.messages.iter().any(|m| {
                        m.id == message.id
                            || (m.id.starts_with("sending_")
                                && m.type_ == 2
                                && message.type_ == 2
                                && m.body == message.body
                                && (m.date - message.date).abs() < 300_000)
                    });

                    if !already_exists {
                        self.messages.push(message.clone());
                    } else {
                        if let Some(existing) = self.messages.iter_mut().find(|m| {
                            m.id.starts_with("sending_") && m.type_ == 2 && m.body == message.body
                        }) {
                            existing.id = message.id.clone();
                            existing.thread_id = message.thread_id.clone();
                            existing.date = message.date;
                        }
                    }
                    self.messages.sort_by_key(|m| m.date);
                }

                if let Some(conv) = self
                    .conversations
                    .iter_mut()
                    .find(|c| c.thread_id == message.thread_id)
                {
                    conv.last_message = message.body;
                    conv.timestamp = message.date;
                    if !is_selected && message.type_ != 2 {
                        conv.unread = true;
                    }
                }
                self.conversations
                    .sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            }
            ProtocolEvent::Error(e) => error!("SMS protocol error: {}", e),
        }
    }

    fn update_conversation_names(&mut self) {
        for conv in &mut self.conversations {
            if let Some(name) = self
                .contacts
                .iter()
                .find(|(phone, _)| utils::phone_numbers_match(phone, &conv.phone_number))
                .map(|(_, name)| name.clone())
            {
                conv.contact_name = name;
            }
        }
    }
}
