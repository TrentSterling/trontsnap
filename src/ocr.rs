// Region OCR: turn a capture into TEXT via the OCR engine Windows ships
// (Windows.Media.Ocr) — the PowerToys Text Extractor feature, no cloud, no
// model download, no new dependency (the `windows` crate is already here).
//
// The engine reads the user's installed language packs. If none of them has
// OCR data, TryCreateFromUserProfileLanguages yields nothing; that is a
// message, never a panic — the picker must not die because OCR is unavailable.

use anyhow::Context as _;
use image::RgbaImage;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Recognize the text in `img`, joined per OCR line with CRLF. Blocks (the
/// WinRT async is waited synchronously); run on a worker thread, never the UI
/// thread. Returns Ok("") when the region simply contains no words.
pub fn recognize(img: &RgbaImage) -> anyhow::Result<String> {
    // WinRT needs an initialized apartment on the calling thread. MTA matches
    // every other worker in this process; RPC_E_CHANGED_MODE (already STA)
    // is fine too, so the result is deliberately ignored.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let engine = OcrEngine::TryCreateFromUserProfileLanguages().context(
        "Windows OCR has no language pack installed \
         (Settings > Time & Language > Language, add language features)",
    )?;

    // The engine rejects images past MaxImageDimension (2600px on current
    // Windows). Text that big is comfortably readable downscaled, so shrink
    // to fit instead of failing a large-monitor region grab.
    let max = OcrEngine::MaxImageDimension().unwrap_or(2600);
    let (w, h) = (img.width(), img.height());
    let scaled;
    let src: &RgbaImage = if w > max || h > max {
        let s = max as f32 / w.max(h) as f32;
        let nw = ((w as f32 * s) as u32).max(1);
        let nh = ((h as f32 * s) as u32).max(1);
        scaled = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
        &scaled
    } else {
        img
    };

    // RgbaImage -> BGRA8 bytes -> IBuffer -> SoftwareBitmap. DataWriter is the
    // idiomatic windows-rs route to an IBuffer without unsafe.
    let (sw, sh) = (src.width(), src.height());
    let mut bgra = Vec::with_capacity((sw * sh * 4) as usize);
    for px in src.pixels() {
        bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        sw as i32,
        sh as i32,
    )?;

    let result = engine.RecognizeAsync(&bitmap)?.get()?;

    // NOT OcrResult::Text(): that flattens every word into one space-joined
    // string. Joining per line keeps the layout, which is what makes pasted
    // code, logs and tables usable.
    let mut lines = Vec::new();
    for line in result.Lines()? {
        lines.push(line.Text()?.to_string());
    }
    Ok(lines.join("\r\n"))
}

#[cfg(test)]
mod tests {
    // Exercises the full WinRT chain headlessly: apartment init, engine from
    // the user's language packs, IBuffer -> SoftwareBitmap, RecognizeAsync.
    // Ignored by default like the other machine-dependent smokes: it needs an
    // installed OCR language pack, which a bare CI image may not have.
    #[test]
    #[ignore]
    fn ocr_blank_smoke() {
        let img = image::RgbaImage::from_pixel(320, 120, image::Rgba([255, 255, 255, 255]));
        let text = super::recognize(&img).expect("OCR engine + interop should succeed");
        assert!(text.trim().is_empty(), "blank image produced text: {text:?}");
    }
}
