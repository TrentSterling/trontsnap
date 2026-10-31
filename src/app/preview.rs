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
        let auto_item = CheckMenuItem::new("Start at login", true, false, None);
        let cursor_item = CheckMenuItem::new("Capture cursor", true, true, None);
        let audio_item = CheckMenuItem::new("Record audio", true, true, None);
        let (_, menu_rx) = crossbeam_channel::unbounded();
        Self {
            gallery: Gallery::preview(ctx, paths),
            tab: Tab::Gallery,
            icon_tex,
            _hotkeys: None,
            _tray: None,
            chrome_drag: Default::default(),
            auto_id: auto_item.id().clone(),
            auto_item,
            cursor_id: cursor_item.id().clone(),
            cursor_item,
            audio_id: audio_item.id().clone(),
            audio_item,
            show_flag: Arc::new(AtomicBool::new(false)),
            good_size: egui::vec2(1400.0, 820.0),
            small_frames: 0,
            menu_rx,
            hwnd_cell: Arc::new(AtomicIsize::new(0)),
            show_theme_window: false,
        }
    }

    pub(crate) fn preview_draw(&mut self, ctx: &egui::Context, view: &str) {
        self.tab = if view == "settings" {
            Tab::Settings
        } else if view == "about" {
            Tab::About
        } else {
            Tab::Gallery
        };
        self.show_theme_window = view == "theme";
        self.draw(ctx, None);
    }
}
