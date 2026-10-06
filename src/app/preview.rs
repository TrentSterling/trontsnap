use super::*;

impl App {
    pub(crate) fn preview(ctx: &egui::Context, paths: &[std::path::PathBuf]) -> Self {
        crate::theme::apply(ctx);
        ctx.style_mut(|s| s.interaction.selectable_labels = false);
        let icon = app_icon();
        let icon_tex = ctx.load_texture(
            "trontsnap-icon",
            egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            ),
            egui::TextureOptions::LINEAR,
        );
        // The tray + its menu items are Windows-only fields of App (cfg(windows)); the
        // Linux build has none of them, so the fixture must not name them there either.
        #[cfg(windows)]
        let auto_item = CheckMenuItem::new("Start at login", true, false, None);
        #[cfg(windows)]
        let cursor_item = CheckMenuItem::new("Capture cursor", true, true, None);
        #[cfg(windows)]
        let audio_item = CheckMenuItem::new("Record audio", true, true, None);
        #[cfg(windows)]
        let (_, menu_rx) = crossbeam_channel::unbounded();
        Self {
            gallery: Gallery::preview(ctx, paths),
            tab: Tab::Gallery,
            icon_tex,
            _hotkeys: None,
            #[cfg(windows)]
            _tray: None,
            chrome_drag: Default::default(),
            #[cfg(windows)]
            auto_id: auto_item.id().clone(),
            #[cfg(windows)]
            auto_item,
            #[cfg(windows)]
            cursor_id: cursor_item.id().clone(),
            #[cfg(windows)]
            cursor_item,
            #[cfg(windows)]
            audio_id: audio_item.id().clone(),
            #[cfg(windows)]
            audio_item,
            show_flag: Arc::new(AtomicBool::new(false)),
            good_size: egui::vec2(1400.0, 820.0),
            small_frames: 0,
            #[cfg(windows)]
            menu_rx,
            hwnd_cell: Arc::new(AtomicIsize::new(0)),
            show_theme_window: false,
            #[cfg(not(windows))]
            start_hidden: false,
            #[cfg(not(windows))]
            x11_win: 0,
            #[cfg(not(windows))]
            hidden: false,
        }
    }

    pub(crate) fn preview_draw(&mut self, ctx: &egui::Context, view: &str) {
        self.tab = if view == "settings" {
            Tab::Settings
        } else {
            Tab::Gallery
        };
        self.show_theme_window = view == "theme";
        self.draw(ctx, None);
    }
}
