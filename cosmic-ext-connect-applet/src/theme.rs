//! Reads the COSMIC desktop theme (accent colour, dark/light mode) directly from
//! the host filesystem. `dirs::config_dir()` resolves to the Flatpak sandbox
//! when running as a Flatpak, so we always use `dirs::home_dir()` instead.

use cosmic::widget::button::Catalog;
use serde::Deserialize;

/// COSMIC's default teal, used as a fallback when the host theme cannot be read.
pub const FALLBACK_TEAL: cosmic::iced::Color = cosmic::iced::Color {
    r: 0.067,
    g: 0.533,
    b: 0.533,
    a: 1.0,
};

#[derive(Deserialize)]
struct SrgbaColor {
    red: f32,
    green: f32,
    blue: f32,
}

#[derive(Deserialize)]
struct AccentFile {
    base: SrgbaColor,
}

/// Reads the user's current COSMIC accent colour from the host config directory.
///
/// Returns `None` if any file is missing or cannot be parsed, in which case
/// the caller should fall back to [`FALLBACK_TEAL`].
pub fn try_load_cosmic_accent() -> Option<cosmic::iced::Color> {
    let home = dirs::home_dir()?;
    let cosmic_cfg = home.join(".config").join("cosmic");

    // Read dark/light preference; default to dark when the file is absent.
    let is_dark = std::fs::read_to_string(
        cosmic_cfg
            .join("com.system76.CosmicTheme.Mode")
            .join("v1")
            .join("is_dark"),
    )
    .map(|s| s.trim() == "true")
    .unwrap_or(true);

    let theme_dir = if is_dark {
        "CosmicTheme.Dark"
    } else {
        "CosmicTheme.Light"
    };

    let accent_path = cosmic_cfg
        .join(format!("com.system76.{theme_dir}"))
        .join("v1")
        .join("accent");

    let text = std::fs::read_to_string(accent_path).ok()?;
    let parsed: AccentFile = ron::from_str(&text).ok()?;

    Some(cosmic::iced::Color {
        r: parsed.base.red,
        g: parsed.base.green,
        b: parsed.base.blue,
        a: 1.0,
    })
}

/// A `Button::Custom` class that mirrors `Button::Text` but overrides the text
/// colour with the host's real accent. Needed because `Button::Text`'s default
/// styling pulls from the sandboxed (always-teal) theme under Flatpak.
pub fn accent_link_button(accent: cosmic::iced::Color) -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(move |focused, theme| {
            let mut style = theme.active(focused, false, &cosmic::theme::Button::Text);
            style.text_color = Some(accent);
            style
        }),
        hovered: Box::new(move |focused, theme| {
            let mut style = theme.hovered(focused, false, &cosmic::theme::Button::Text);
            style.text_color = Some(accent);
            style
        }),
        pressed: Box::new(move |focused, theme| {
            let mut style = theme.pressed(focused, false, &cosmic::theme::Button::Text);
            style.text_color = Some(accent);
            style
        }),
        disabled: Box::new(|theme| theme.disabled(&cosmic::theme::Button::Text)),
    }
}

/// A `Button::Custom` class that mirrors `Button::Suggested` but fills the
/// background with the host's real accent, for the same sandboxed-theme
pub fn accent_filled_button(accent: cosmic::iced::Color) -> cosmic::theme::Button {
    cosmic::theme::Button::Custom {
        active: Box::new(move |focused, theme| {
            let mut style = theme.active(focused, false, &cosmic::theme::Button::Suggested);
            style.background = Some(cosmic::iced::Background::Color(accent));
            style
        }),
        hovered: Box::new(move |focused, theme| {
            let mut style = theme.hovered(focused, false, &cosmic::theme::Button::Suggested);
            style.background = Some(cosmic::iced::Background::Color(accent));
            style
        }),
        pressed: Box::new(move |focused, theme| {
            let mut style = theme.pressed(focused, false, &cosmic::theme::Button::Suggested);
            style.background = Some(cosmic::iced::Background::Color(accent));
            style
        }),
        disabled: Box::new(|theme| theme.disabled(&cosmic::theme::Button::Suggested)),
    }
}

/// Loads a named symbolic icon and recolors its fill to the given accent.
pub fn accent_icon(name: &str, accent: cosmic::iced::Color) -> cosmic::widget::icon::Handle {
    let hex = format!(
        "#{:02x}{:02x}{:02x}",
        (accent.r * 255.0).round() as u8,
        (accent.g * 255.0).round() as u8,
        (accent.b * 255.0).round() as u8,
    );

    cosmic::widget::icon::from_name(name)
        .path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|svg| cosmic::widget::icon::from_svg_bytes(svg.replace("#232323", &hex).into_bytes()))
        .unwrap_or_else(|| cosmic::widget::icon::from_name(name).handle())
}
