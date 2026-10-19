pub mod auth_dialog;
pub mod collection;
pub mod floating_island;
pub mod library_view;
pub mod preferences_dialog;
pub mod search_view;
pub mod wave_clip;
pub mod wave_gl;
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

/// Путь к иконке приложения (`data/icons/<имя>`): рядом с исходниками при разработке, затем в установленных
/// каталогах данных (`/usr/share/volna/icons/` для пакета). None — иконки нет, вызывающий берёт запасную из темы.
pub fn bundled_icon(name: &str) -> Option<std::path::PathBuf> {
    let mut candidates = vec![std::path::Path::new("data/icons").join(name)];
    // путь к исходникам вшивается только в отладочную сборку: в пакет попадать он не должен
    #[cfg(debug_assertions)]
    candidates.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons").join(name));
    candidates.extend(glib::system_data_dirs().into_iter().map(|d| d.join("volna/icons").join(name)));
    candidates.into_iter().find(|p| p.exists())
}
