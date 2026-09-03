//! The `SmsMessage` action/event enum for the SMS window's Elm-style
//! update loop. Named "actions" rather than "messages" since this is a
//! UI-framework concept (iced/cosmic calls it a "Message") distinct from
//! an actual SMS message — having both in the same file under that name
//! was the source of a previous mix-up.

use std::collections::HashMap;

use super::avatar::Avatar;
use super::emoji::EmojiCategory;
use super::models::{Conversation, ProtocolEvent};

/// All possible messages that the SMS window can receive and process.
#[derive(Clone, Debug)]
pub enum SmsMessage {
    LoadConversations,
    #[allow(dead_code)]
    ConversationsLoaded(Vec<Conversation>),
    #[allow(dead_code)]
    ContactsLoaded(HashMap<String, String>),
    /// Phone -> decoded photo bytes, merged into existing entries rather
    /// than replacing them (vcards arrive in batches, and a later batch
    /// shouldn't blank out photos already shown from an earlier one).
    ContactPhotosLoaded(HashMap<String, Vec<u8>>),
    /// Result of baking `ContactPhotosLoaded`'s raw bytes into circular
    /// avatars off the UI thread — see the `ContactPhotosLoaded` handler
    /// in `app.rs`. Kept as a separate message rather than baking inline
    /// so a large contact list with many photos can't stall the UI.
    AvatarsBaked(HashMap<String, Avatar>),
    SelectThread(String),
    UpdateInput(String),
    UpdateSearch(String),
    SendMessage,
    RefreshThread,
    #[allow(dead_code)]
    CloseWindow,
    ProtocolEventReceived(ProtocolEvent),
    OpenNewChatDialog,
    CloseNewChatDialog,
    UpdateNewChatPhone(String),
    SelectContactForNewChat(usize),
    CreateNewChat,

    // Emoji picker
    ToggleEmojiPicker,
    SelectEmojiCategory(EmojiCategory),
    InsertEmoji(String),

    /// Opens the confirmation dialog for deleting (hiding) a conversation.
    RequestDeleteConversation(String),
    /// Closes the confirmation dialog without deleting anything.
    CancelDeleteConversation,
    /// Hides the pending conversation from this device's view going
    /// forward. Local-only — the SMS protocol has no delete packet, so
    /// this never touches the phone's actual messages or conversation.
    ConfirmDeleteConversation,

    /// User tapped a thumbnail that hasn't been fully downloaded yet.
    RequestFullAttachment {
        part_id: i64,
        unique_identifier: String,
    },
    /// A full-resolution attachment finished downloading. Payload is
    /// (filename/unique_identifier, saved path) — see
    /// `kdeconnect_dbus_client::ServiceEvent::SmsAttachmentReceived`.
    AttachmentReceived(String, std::path::PathBuf),
    /// User wants to open a downloaded attachment in its default external
    /// app (used for video, which iced can't render inline).
    OpenAttachment(std::path::PathBuf),
    /// User wants to save a downloaded attachment to a location of their
    /// choosing via the native "Save As" dialog.
    SaveAttachment(std::path::PathBuf),

    /// Opens the native file picker for staging an outgoing attachment.
    PickAttachment,
    /// Files chosen from the picker, appended to `pending_attachments`.
    AttachmentsPicked(Vec<String>),
    /// Removes one staged attachment by index before sending.
    RemovePendingAttachment(usize),
}
