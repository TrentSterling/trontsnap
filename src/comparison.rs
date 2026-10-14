//! New, labeled comparison images; source screenshots are never modified.
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub struct Pair {
    pub before: PathBuf,
    pub after: PathBuf,
}

/// Returns true to create the comparison, false to cancel, or None while open.
pub fn dialog(ctx: &eframe::egui::Context, pair: &mut Pair) -> Option<bool> {
    use eframe::egui;
    let mut decision = None;
    egui::Window::new("Before / after comparison")
        .collapsible(false).resizable(false).default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("Create a labeled side-by-side image, then annotate it with arrows, boxes or text.");
            ui.add_space(8.0);
            for (label, path) in [("Before", &pair.before), ("After", &pair.after)] {
                ui.strong(label);
                ui.add(egui::Label::new(path.file_name().unwrap_or_default().to_string_lossy()).wrap()).on_hover_text(path.display().to_string());
            }
            ui.add_space(8.0);
            if ui.button("Swap before / after").clicked() { std::mem::swap(&mut pair.before, &mut pair.after); }
            ui.small("Saved as a new PNG in TrontSnap. Originals stay unchanged. Images keep their proportions and fit within 1280 × 1440 each.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Create and annotate").clicked() { decision = Some(true); }
                if ui.button("Cancel").clicked() { decision = Some(false); }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        decision = Some(false);
    }
    decision
}

fn load(path: &Path) -> Result<RgbaImage> {
    let mut reader = image::io::Reader::open(path)?.with_guessed_format()?;
    let mut limits = image::io::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .with_context(|| format!("Could not read {}", path.display()))?;
    Ok(if image.width() > 1280 || image.height() > 1440 {
        image.thumbnail(1280, 1440).to_rgba8()
    } else {
        image.to_rgba8()
    })
}

pub fn compose(before: &RgbaImage, after: &RgbaImage) -> RgbaImage {
    let cell_w = before.width().max(after.width()).max(160);
    let cell_h = before.height().max(after.height()).max(1);
    let mut result = RgbaImage::from_pixel(cell_w * 2 + 44, cell_h + 80, Rgba([20, 23, 31, 255]));
    for (image, left, label) in [(before, 16, "BEFORE"), (after, cell_w + 28, "AFTER")] {
        crate::editor::draw_text_px(
            &mut result,
            eframe::egui::pos2(left as f32, 16.0),
            label,
            26.0,
            eframe::egui::Color32::WHITE,
        );
        image::imageops::overlay(
            &mut result,
            image,
            (left + (cell_w - image.width()) / 2) as i64,
            (64 + (cell_h - image.height()) / 2) as i64,
        );
    }
    result
}

pub fn create(pair: &Pair, directory: &Path) -> Result<PathBuf> {
    let before = load(&pair.before)?;
    let after = load(&pair.after)?;
    let png = crate::capture::encode_png(&compose(&before, &after))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%3f");
    for suffix in 0..1000 {
        let path = directory.join(format!("TrontSnap_Compare_{stamp}_{suffix}.png"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(&png).and_then(|_| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error.into());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("Could not choose a unique comparison filename")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_keeps_aspect_order_and_original_files() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("compare-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pair = Pair {
            before: dir.join("before.png"),
            after: dir.join("after.png"),
        };
        RgbaImage::from_pixel(400, 200, Rgba([255, 0, 0, 255]))
            .save(&pair.before)
            .unwrap();
        RgbaImage::from_pixel(200, 400, Rgba([0, 0, 255, 255]))
            .save(&pair.after)
            .unwrap();
        let originals = [
            std::fs::read(&pair.before).unwrap(),
            std::fs::read(&pair.after).unwrap(),
        ];
        let first = create(&pair, &dir).unwrap();
        let second = create(&pair, &dir).unwrap();
        assert_ne!(first, second);
        let result = image::open(&first).unwrap().to_rgba8();
        assert_eq!(result.dimensions(), (844, 480));
        assert_eq!(*result.get_pixel(16, 164), Rgba([255, 0, 0, 255]));
        assert_eq!(*result.get_pixel(528, 64), Rgba([0, 0, 255, 255]));
        assert_eq!(*result.get_pixel(16, 64), Rgba([20, 23, 31, 255]));
        assert_eq!(std::fs::read(&pair.before).unwrap(), originals[0]);
        assert_eq!(std::fs::read(&pair.after).unwrap(), originals[1]);
        assert!(create(
            &Pair {
                before: dir.join("missing.png"),
                after: pair.after.clone()
            },
            &dir
        )
        .is_err());
        for path in [&pair.before, &pair.after, &first, &second] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(dir).unwrap();
    }
}
