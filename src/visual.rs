//! Repeatable screenshots of production UI, using public fixture images and a
//! GPU texture. No native window, tray, hotkeys, registry writes or OS input.
use eframe::egui;
use egui_wgpu::wgpu;
use std::path::Path;

struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: egui_wgpu::Renderer,
}

impl Renderer {
    fn new() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("offscreen adapter");
        println!("Offscreen adapter: {}", adapter.get_info().name);
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .expect("offscreen device");
        let renderer =
            egui_wgpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, None, 1, false);
        Self {
            device,
            queue,
            renderer,
        }
    }

    fn save(&mut self, ctx: &egui::Context, output: egui::FullOutput, size: [u32; 2], path: &Path) {
        let [width, height] = size;
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("TrontSnap offscreen capture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        for (id, delta) in &output.textures_delta.set {
            self.renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let mut commands =
            self.renderer
                .update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Production UI"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        let stride = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Capture readback"),
            size: (stride * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            extent,
        );
        commands.push(encoder.finish());
        self.queue.submit(commands);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv_timeout(std::time::Duration::from_secs(20))
            .unwrap()
            .unwrap();
        let pixels = buffer
            .slice(..)
            .get_mapped_range()
            .chunks(stride as usize)
            .flat_map(|row| row[..width as usize * 4].iter().copied())
            .collect();
        buffer.unmap();
        image::RgbaImage::from_raw(width, height, pixels)
            .unwrap()
            .save(path)
            .unwrap();
        for id in output.textures_delta.free {
            self.renderer.free_texture(&id);
        }
    }
}

fn capture(
    renderer: &mut Renderer,
    ctx: &egui::Context,
    mut draw: impl FnMut(&egui::Context),
    size: [u32; 2],
    path: &Path,
) {
    // Accumulate warm-up texture deltas: discarding them loses the font atlas
    // and thumbnail uploads before the final frame reaches the GPU renderer.
    let mut combined: Option<egui::FullOutput> = None;
    for frame in 0..5 {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            },
            |ctx| draw(ctx),
        );
        if let Some(combined) = &mut combined {
            combined.append(output);
        } else {
            combined = Some(output);
        }
    }
    renderer.save(ctx, combined.unwrap(), size, path);
}

#[test]
#[ignore = "Explicit offscreen production capture; prepare target/showcase-input with public images first"]
fn capture_current_app() {
    let inputs = std::env::var_os("TRONTSNAP_PREVIEW_IMAGES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/showcase-input".into());
    let mut paths: Vec<_> = std::fs::read_dir(inputs)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "png" || e == "jpg"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 18,
        "supply at least 18 public screenshot fixtures"
    );
    let out = Path::new("target/showcase");
    std::fs::create_dir_all(out).unwrap();
    let mut renderer = Renderer::new();
    for (name, view, dark, seed, size) in [
        ("gallery-dark", "gallery", true, [90, 209, 255], [1400, 820]),
        (
            "gallery-light",
            "gallery",
            false,
            [117, 90, 204],
            [1400, 820],
        ),
        ("theme", "theme", true, [90, 209, 255], [1400, 820]),
        ("settings", "settings", true, [90, 209, 255], [1000, 850]),
        ("compact", "gallery", true, [90, 209, 255], [680, 480]),
    ] {
        let ctx = egui::Context::default();
        let mut app = crate::app::App::preview(&ctx, &paths);
        crate::theme::preview_theme(&ctx, seed, dark);
        capture(
            &mut renderer,
            &ctx,
            |ctx| app.preview_draw(ctx, view),
            size,
            &out.join(format!("{name}.png")),
        );
    }
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    crate::theme::preview_theme(&ctx, [90, 209, 255], true);
    let draw = crate::editor::preview_drawer(paths[0].clone());
    capture(
        &mut renderer,
        &ctx,
        draw,
        [1200, 760],
        &out.join("editor.png"),
    );
    println!(
        "PASS: six current production UI captures in {} (public fixture gallery)",
        out.display()
    );
}
