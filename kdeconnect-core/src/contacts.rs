//! Small shared helpers for contact data extracted from vcards (see
//! `parse_vcard` in `plugin_interface.rs`). Kept as base64 the whole way
//! from the phone to the UI — same convention as SMS thumbnails — so this
//! is just the one decode step, exposed here so callers (the SMS window)
//! don't need their own `base64` dependency for it.

/// Decodes a base64-encoded contact photo. Returns `None` on bad input
/// rather than failing — a broken photo shouldn't take down the rest of
/// the contact list.
pub fn decode_photo(base64_encoded: &str) -> Option<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose};
    general_purpose::STANDARD.decode(base64_encoded).ok()
}
