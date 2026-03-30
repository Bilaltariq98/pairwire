//! Viewer UI — renders the remote screen and cursor overlay using egui/wgpu.
//!
//! Only compiled when the `gui` feature is enabled. Requires a display and GPU.
//! On headless targets, use `--no-gui` mode which skips this module entirely.

#[cfg(feature = "gui")]
mod inner {
    use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
    use pairwire_capture::DecodedFrame;
    use pairwire_protocol::CursorUpdate;
    use tokio::sync::watch;
    use tracing::debug;

    /// egui application state for the screen viewer.
    pub struct ViewerApp {
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        texture: Option<TextureHandle>,
        frame_count: u64,
        last_fps_time: std::time::Instant,
        fps: f32,
    }

    impl ViewerApp {
        pub fn new(
            frame_rx: watch::Receiver<Option<DecodedFrame>>,
            cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        ) -> Self {
            Self {
                frame_rx,
                cursor_rx,
                texture: None,
                frame_count: 0,
                last_fps_time: std::time::Instant::now(),
                fps: 0.0,
            }
        }

        fn upload_frame(&mut self, ctx: &Context, frame: &DecodedFrame) {
            let w = frame.width as usize;
            let h = frame.height as usize;

            // DecodedFrame is BGRA; egui expects RGBA — swap B↔R
            let mut rgba = Vec::with_capacity(w * h * 4);
            for chunk in frame.data.chunks_exact(4) {
                rgba.push(chunk[2]); // R
                rgba.push(chunk[1]); // G
                rgba.push(chunk[0]); // B
                rgba.push(chunk[3]); // A
            }

            let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            match &mut self.texture {
                Some(tex) => tex.set(image, TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ctx.load_texture("screen", image, TextureOptions::LINEAR));
                }
            }

            self.frame_count += 1;
            let elapsed = self.last_fps_time.elapsed().as_secs_f32();
            if elapsed >= 1.0 {
                self.fps = self.frame_count as f32 / elapsed;
                self.frame_count = 0;
                self.last_fps_time = std::time::Instant::now();
                debug!(fps = self.fps, "viewer fps");
            }
        }
    }

    impl eframe::App for ViewerApp {
        fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
            let new_frame = self.frame_rx.borrow().clone();
            if let Some(frame) = new_frame {
                self.upload_frame(ctx, &frame);
            }
            ctx.request_repaint();

            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    if let Some(tex) = &self.texture {
                        let available = ui.available_size();
                        let tex_size = tex.size_vec2();
                        let scale = (available.x / tex_size.x).min(available.y / tex_size.y);
                        let display_size = tex_size * scale;
                        let offset = (available - display_size) * 0.5;
                        let rect =
                            egui::Rect::from_min_size(ui.min_rect().min + offset, display_size);

                        ui.painter().image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );

                        // Remote cursor overlay
                        if let Some(cursor) = self.cursor_rx.borrow().as_ref() {
                            if cursor.visible {
                                let cx =
                                    rect.min.x + (cursor.x as f32 / tex_size.x) * display_size.x;
                                let cy =
                                    rect.min.y + (cursor.y as f32 / tex_size.y) * display_size.y;
                                let pos = egui::pos2(cx, cy);
                                let color = egui::Color32::from_rgba_unmultiplied(255, 80, 80, 220);
                                let stroke = egui::Stroke::new(2.0, color);
                                let p = ui.painter();
                                p.circle_stroke(pos, 8.0, stroke);
                                p.line_segment(
                                    [pos - Vec2::new(12.0, 0.0), pos + Vec2::new(12.0, 0.0)],
                                    stroke,
                                );
                                p.line_segment(
                                    [pos - Vec2::new(0.0, 12.0), pos + Vec2::new(0.0, 12.0)],
                                    stroke,
                                );
                                if cursor.click_animation > 0 {
                                    let r = 8.0 + cursor.click_animation as f32 * 3.0;
                                    let a = 220u8.saturating_sub(cursor.click_animation * 20);
                                    p.circle_stroke(
                                        pos,
                                        r,
                                        egui::Stroke::new(
                                            1.5,
                                            egui::Color32::from_rgba_unmultiplied(255, 80, 80, a),
                                        ),
                                    );
                                }
                            }
                        }
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new("Connecting…")
                                    .size(24.0)
                                    .color(egui::Color32::GRAY),
                            );
                        });
                    }

                    if self.fps > 0.0 {
                        ui.painter().text(
                            ui.max_rect().right_top() + Vec2::new(-8.0, 8.0),
                            egui::Align2::RIGHT_TOP,
                            format!("{:.0} fps", self.fps),
                            egui::FontId::monospace(12.0),
                            egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                        );
                    }
                });
        }
    }

    /// Launch the viewer window. Blocks until the window is closed.
    /// Must be called from the main thread.
    pub fn run_viewer(
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        title: &str,
    ) -> anyhow::Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([1280.0, 720.0])
                .with_min_inner_size([320.0, 240.0]),
            ..Default::default()
        };

        eframe::run_native(
            title,
            options,
            Box::new(|_cc| Ok(Box::new(ViewerApp::new(frame_rx, cursor_rx)))),
        )
        .map_err(|e| anyhow::anyhow!("viewer error: {e}"))
    }
}

#[cfg(feature = "gui")]
pub use inner::run_viewer;
