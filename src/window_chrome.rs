//! Observe the title strip without allocating a widget over its controls.
//! Clicks belong to the controls; movement past egui's click tolerance belongs
//! to the native window, even when the press began on a button or search field.
use eframe::egui;

pub(crate) const RESIZE_EDGE: f32 = 6.0;
pub(crate) const RESIZE_CORNER: f32 = 10.0;

#[derive(Default)]
pub(crate) struct ChromeDrag {
    origin: Option<egui::Pos2>,
    moving: bool,
}

impl ChromeDrag {
    pub(crate) fn update(&mut self, ctx: &egui::Context, maximized: bool) {
        let screen = ctx.screen_rect();
        let (pressed, down, position) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.interact_pos(),
            )
        });
        if !down {
            self.origin = None;
            self.moving = false;
            return;
        }
        if pressed {
            self.moving = false;
            self.origin = position.filter(|&p| {
                let strip = egui::Rect::from_min_max(
                    screen.min,
                    egui::pos2(screen.right(), screen.top() + 40.0),
                );
                if !strip.contains(p) {
                    return false;
                }
                // Keep the existing resize edges and corners available.
                if !maximized
                    && (p.y < screen.top() + RESIZE_EDGE
                        || p.x < screen.left() + RESIZE_EDGE
                        || p.x > screen.right() - RESIZE_EDGE
                        || (p.y < screen.top() + RESIZE_CORNER
                            && (p.x < screen.left() + RESIZE_CORNER
                                || p.x > screen.right() - RESIZE_CORNER)))
                {
                    return false;
                }
                // Floating windows and menus can cover the strip. Their input
                // stays local; only the strip and its caption overlay move us.
                ctx.layer_id_at(p).is_none_or(|layer| {
                    layer.order == egui::Order::Background
                        || layer.id == egui::Id::new("trontsnap_caption_overlay")
                })
            });
        }
        let Some((origin, position)) = self.origin.zip(position) else {
            return;
        };
        let tolerance = ctx.options(|o| o.input_options.max_click_dist);
        self.moving |= origin.distance(position) > tolerance;
        if self.moving {
            // A restored window can still be gaining focus. Retry while held,
            // preserving the native Aero Snap and maximized-window move paths.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }
}
