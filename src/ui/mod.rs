pub mod auth_dialog;
pub mod floating_island;
pub mod library_view;
pub mod preferences_dialog;
pub mod search_view;
pub mod wave_view;
pub mod window;

pub use auth_dialog::show_auth_dialog;
pub use window::VolnaWindow;

/// Загружает встроенные CSS стили приложения
pub fn load_css() {
    let provider = gtk4::CssProvider::new();
    let css_data = include_str!("style.css");
    provider.load_from_string(css_data);

    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
