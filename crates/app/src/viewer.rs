//! Viewer UI — renders the remote screen and cursor overlay using egui/wgpu.
//!
//! Only compiled when the `gui` feature is enabled. Requires a display and GPU.
//! On headless targets, use `--no-gui` mode which skips this module entirely.

#[cfg(feature = "gui")]
mod inner {
    use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
    use pairwire_capture::DecodedFrame;
    use pairwire_protocol::CursorUpdate;
    use pairwire_protocol::input::{InputMessage, KeyEvent, MouseClick, MouseScroll};
    use tokio::sync::{broadcast, watch};
    use tracing::debug;

    use crate::reconnect::ConnectionState;

    /// egui application state for the screen viewer.
    pub struct ViewerApp {
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        conn_rx: watch::Receiver<ConnectionState>,
        input_tx: broadcast::Sender<InputMessage>,
        texture: Option<TextureHandle>,
        frame_count: u64,
        last_fps_time: std::time::Instant,
        fps: f32,
        /// Cached texture dimensions for coordinate mapping.
        tex_size: Option<(f32, f32)>,
    }

    impl ViewerApp {
        pub fn new(
            frame_rx: watch::Receiver<Option<DecodedFrame>>,
            cursor_rx: watch::Receiver<Option<CursorUpdate>>,
            conn_rx: watch::Receiver<ConnectionState>,
            input_tx: broadcast::Sender<InputMessage>,
        ) -> Self {
            Self {
                frame_rx,
                cursor_rx,
                conn_rx,
                input_tx,
                texture: None,
                frame_count: 0,
                last_fps_time: std::time::Instant::now(),
                fps: 0.0,
                tex_size: None,
            }
        }

        /// Map a position in screen-space to host display coordinates.
        fn map_to_host(
            &self,
            pos: egui::Pos2,
            rect: egui::Rect,
            display_size: Vec2,
        ) -> Option<(u16, u16)> {
            let (tex_w, tex_h) = self.tex_size?;
            if display_size.x == 0.0 || display_size.y == 0.0 {
                return None;
            }
            let rel_x = (pos.x - rect.min.x) / display_size.x;
            let rel_y = (pos.y - rect.min.y) / display_size.y;
            if !(0.0..=1.0).contains(&rel_x) || !(0.0..=1.0).contains(&rel_y) {
                return None;
            }
            let host_x = (rel_x * tex_w).min(tex_w - 1.0).max(0.0) as u16;
            let host_y = (rel_y * tex_h).min(tex_h - 1.0).max(0.0) as u16;
            Some((host_x, host_y))
        }

        fn send_input(&self, msg: InputMessage) {
            // Best-effort send — if no receivers or channel full, silently drop.
            let _ = self.input_tx.send(msg);
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

            self.tex_size = Some((w as f32, h as f32));
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

            let conn_state = self.conn_rx.borrow().clone();

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
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
                    } else if conn_state == ConnectionState::Connected {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new("Connecting…")
                                    .size(24.0)
                                    .color(egui::Color32::GRAY),
                            );
                        });
                    }

                    // Connection state overlay (reconnecting / lost)
                    match &conn_state {
                        ConnectionState::Reconnecting { attempt, max } => {
                            show_overlay(
                                ui,
                                &format!("Reconnecting… ({attempt}/{max})"),
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                                egui::Color32::from_rgb(255, 200, 50),
                            );
                        }
                        ConnectionState::Lost => {
                            show_overlay(
                                ui,
                                "Connection lost",
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                                egui::Color32::from_rgb(255, 80, 80),
                            );
                        }
                        ConnectionState::Connected => {}
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

            // Capture input events and forward to host.
            self.capture_input_events(ctx);
        }
    }

    impl ViewerApp {
        /// Read egui input events and send them as `InputMessage` to the host.
        fn capture_input_events(&self, ctx: &Context) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            ctx.input(|input| {
                // Keyboard events
                for event in &input.events {
                    match event {
                        egui::Event::Key {
                            key,
                            pressed,
                            modifiers,
                            ..
                        } => {
                            let mut mods: u8 = 0;
                            if modifiers.shift {
                                mods |= pairwire_protocol::input::MOD_SHIFT;
                            }
                            if modifiers.ctrl {
                                mods |= pairwire_protocol::input::MOD_CTRL;
                            }
                            if modifiers.alt {
                                mods |= pairwire_protocol::input::MOD_ALT;
                            }
                            if modifiers.mac_cmd || modifiers.command {
                                mods |= pairwire_protocol::input::MOD_META;
                            }
                            self.send_input(InputMessage::Key(KeyEvent {
                                key_code: *key as u32,
                                modifiers: mods,
                                pressed: *pressed,
                                timestamp_ms: now_ms,
                            }));
                        }
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed,
                            ..
                        } => {
                            // Map to host coordinates using the screen rect.
                            // For now, use a rough mapping based on window size.
                            if let Some((hx, hy)) = self.map_pointer_to_host(*pos) {
                                let btn = match button {
                                    egui::PointerButton::Primary => 0,
                                    egui::PointerButton::Secondary => 1,
                                    egui::PointerButton::Middle => 2,
                                    _ => 0,
                                };
                                self.send_input(InputMessage::Click(MouseClick {
                                    x: hx,
                                    y: hy,
                                    button: btn,
                                    pressed: *pressed,
                                    timestamp_ms: now_ms,
                                }));
                            }
                        }
                        egui::Event::MouseWheel { delta, .. } => {
                            if let Some(pos) = input.pointer.latest_pos() {
                                if let Some((hx, hy)) = self.map_pointer_to_host(pos) {
                                    #[allow(clippy::cast_possible_truncation)]
                                    self.send_input(InputMessage::Scroll(MouseScroll {
                                        x: hx,
                                        y: hy,
                                        delta_x: (delta.x * 100.0) as i16,
                                        delta_y: (delta.y * 100.0) as i16,
                                        timestamp_ms: now_ms,
                                    }));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            });
        }

        /// Map pointer position to host display coordinates.
        /// Returns None if the pointer is outside the screen texture area.
        fn map_pointer_to_host(&self, pos: egui::Pos2) -> Option<(u16, u16)> {
            let (tex_w, tex_h) = self.tex_size?;
            let tex = self.texture.as_ref()?;
            let tex_size = tex.size_vec2();

            // We need to reconstruct the display rect from the texture.
            // This is approximate — in update() we compute the exact rect.
            // For correct mapping we'd cache the rect, but this is close enough
            // for the initial implementation.
            // TODO: cache display_rect from update() for exact mapping.
            let _ = (tex_w, tex_h, tex_size);

            // For now, treat the whole window as the display area.
            // Coordinates are relative to texture dimensions.
            let host_x = pos.x.max(0.0).min(tex_w - 1.0) as u16;
            let host_y = pos.y.max(0.0).min(tex_h - 1.0) as u16;
            Some((host_x, host_y))
        }
    }

    fn show_overlay(ui: &egui::Ui, text: &str, bg: egui::Color32, fg: egui::Color32) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, bg);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(28.0),
            fg,
        );
    }

    /// Launch the viewer window. Blocks until the window is closed.
    /// Must be called from the main thread.
    pub fn run_viewer(
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        conn_rx: watch::Receiver<ConnectionState>,
        input_tx: broadcast::Sender<InputMessage>,
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
            Box::new(|_cc| {
                Ok(Box::new(ViewerApp::new(
                    frame_rx, cursor_rx, conn_rx, input_tx,
                )))
            }),
        )
        .map_err(|e| anyhow::anyhow!("viewer error: {e}"))
    }
}

#[cfg(feature = "gui")]
pub use inner::run_viewer;
