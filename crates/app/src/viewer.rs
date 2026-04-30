//! Viewer UI — renders the remote screen and cursor overlay using egui/wgpu.
//!
//! Only compiled when the `gui` feature is enabled. Requires a display and GPU.
//! On headless targets, use `--no-gui` mode which skips this module entirely.

#[cfg(feature = "gui")]
mod inner {
    use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
    use pairwire_capture::DecodedFrame;
    use pairwire_protocol::CursorUpdate;
    use pairwire_protocol::drawing::{DrawMode, DrawPoint, DrawStroke};
    use pairwire_protocol::input::{InputMessage, KeyEvent, MouseClick, MouseScroll};
    use pairwire_transport::DrawingEvent;
    use tokio::sync::{broadcast, watch};
    use tracing::debug;

    use crate::reconnect::ConnectionState;

    /// egui application state for the screen viewer.
    pub struct ViewerApp {
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        conn_rx: watch::Receiver<ConnectionState>,
        input_tx: broadcast::Sender<InputMessage>,
        drawing_rx: watch::Receiver<Vec<DrawStroke>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
        texture: Option<TextureHandle>,
        frame_count: u64,
        last_fps_time: std::time::Instant,
        fps: f32,
        /// Cached texture dimensions for coordinate mapping.
        tex_size: Option<(f32, f32)>,
        /// Cached display rect and size from last render (for input coordinate mapping).
        display_rect: Option<(egui::Rect, Vec2)>,
        /// Whether drawing mode is active (toggled with Cmd/Ctrl+Shift+D).
        drawing_mode: bool,
        /// Current draw mode (laser vs persistent).
        draw_mode: DrawMode,
        /// Points accumulated in the current stroke (host-display coordinates).
        current_stroke: Vec<(u16, u16)>,
        /// Whether the pointer is currently pressed in drawing mode.
        drawing_active: bool,
        /// Peer ID for this guest's strokes (always 1 for the guest in 1:1 sessions).
        peer_id: u8,
    }

    impl ViewerApp {
        pub fn new(
            frame_rx: watch::Receiver<Option<DecodedFrame>>,
            cursor_rx: watch::Receiver<Option<CursorUpdate>>,
            conn_rx: watch::Receiver<ConnectionState>,
            input_tx: broadcast::Sender<InputMessage>,
            drawing_rx: watch::Receiver<Vec<DrawStroke>>,
            drawing_tx: broadcast::Sender<DrawingEvent>,
        ) -> Self {
            Self {
                frame_rx,
                cursor_rx,
                conn_rx,
                input_tx,
                drawing_rx,
                drawing_tx,
                texture: None,
                frame_count: 0,
                last_fps_time: std::time::Instant::now(),
                fps: 0.0,
                tex_size: None,
                display_rect: None,
                drawing_mode: false,
                draw_mode: DrawMode::Laser,
                current_stroke: Vec::new(),
                drawing_active: false,
                peer_id: 1,
            }
        }

        fn send_drawing(&self, event: DrawingEvent) {
            let _ = self.drawing_tx.send(event);
        }

        fn send_input(&self, msg: InputMessage) {
            // Best-effort send — if no receivers or channel full, silently drop.
            let _ = self.input_tx.send(msg);
        }

        /// Render all completed strokes from `drawing_rx` as overlays on the screen texture.
        ///
        /// Laser strokes are drawn with 50% opacity; persistent strokes are fully opaque.
        /// Points are in host-display coordinates and are mapped back to viewer pixels.
        fn render_strokes(
            &self,
            ui: &egui::Ui,
            rect: egui::Rect,
            display_size: Vec2,
            tex_size: egui::Vec2,
        ) {
            let strokes = self.drawing_rx.borrow();
            if strokes.is_empty() && self.current_stroke.is_empty() {
                return;
            }

            let p = ui.painter();

            // Map a host-coordinate point to a viewer pixel position.
            let to_screen = |hx: u16, hy: u16| -> egui::Pos2 {
                let rx = hx as f32 / tex_size.x;
                let ry = hy as f32 / tex_size.y;
                egui::pos2(
                    rect.min.x + rx * display_size.x,
                    rect.min.y + ry * display_size.y,
                )
            };

            // Render completed strokes received from the remote peer.
            for stroke in strokes.iter() {
                if stroke.points.len() < 2 {
                    continue;
                }
                let alpha = match stroke.mode {
                    DrawMode::Laser => 160u8,
                    DrawMode::Persistent => 230u8,
                };
                let color = egui::Color32::from_rgba_unmultiplied(
                    stroke.color[0],
                    stroke.color[1],
                    stroke.color[2],
                    alpha,
                );
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in stroke.points.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
            }

            // Render the current in-progress stroke (local, not yet sent).
            if self.current_stroke.len() >= 2 {
                let color = egui::Color32::from_rgba_unmultiplied(255, 80, 80, 200);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in self.current_stroke.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
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
                        self.display_rect = Some((rect, display_size));

                        ui.painter().image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );

                        // Drawing strokes overlay (below cursor)
                        self.render_strokes(ui, rect, display_size, tex_size);

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
        /// Read egui input events and forward them to the host or drawing subsystem.
        ///
        /// When drawing mode is active (Cmd/Ctrl+Shift+D), pointer events build strokes
        /// instead of being forwarded as remote control input.
        fn capture_input_events(&mut self, ctx: &Context) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            // Collect events and pointer state before mutating self.
            let (events, latest_pos, pointer_down) = ctx.input(|input| {
                (
                    input.events.clone(),
                    input.pointer.latest_pos(),
                    input.pointer.primary_down(),
                )
            });

            for event in &events {
                match event {
                    egui::Event::Key {
                        key,
                        pressed,
                        modifiers,
                        ..
                    } => {
                        // Cmd/Ctrl+Shift+D — toggle drawing mode.
                        if *pressed
                            && *key == egui::Key::D
                            && modifiers.shift
                            && (modifiers.mac_cmd || modifiers.command || modifiers.ctrl)
                        {
                            self.drawing_mode = !self.drawing_mode;
                            self.current_stroke.clear();
                            self.drawing_active = false;
                            debug!(drawing_mode = self.drawing_mode, "drawing mode toggled");
                            continue;
                        }

                        // Escape clears strokes in drawing mode.
                        if *pressed && *key == egui::Key::Escape && self.drawing_mode {
                            self.send_drawing(DrawingEvent::Clear(
                                pairwire_protocol::drawing::DrawClear { peer_id: Some(self.peer_id) },
                            ));
                            self.current_stroke.clear();
                            self.drawing_active = false;
                            continue;
                        }

                        // In drawing mode, don't forward keyboard events to host.
                        if self.drawing_mode {
                            continue;
                        }

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

                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        if self.drawing_mode {
                            if *button == egui::PointerButton::Primary {
                                if *pressed {
                                    // Start a new stroke.
                                    self.current_stroke.clear();
                                    self.drawing_active = true;
                                    if let Some((hx, hy)) = self.map_pointer_to_host(*pos) {
                                        self.current_stroke.push((hx, hy));
                                    }
                                } else if self.drawing_active {
                                    // Stroke complete — send it.
                                    self.drawing_active = false;
                                    if self.current_stroke.len() >= 2 {
                                        let stroke = DrawStroke {
                                            peer_id: self.peer_id,
                                            color: [255, 80, 80],
                                            points: std::mem::take(&mut self.current_stroke),
                                            mode: self.draw_mode,
                                        };
                                        self.send_drawing(DrawingEvent::Stroke(stroke));
                                    } else {
                                        self.current_stroke.clear();
                                    }
                                }
                            }
                            continue;
                        }

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
                        if self.drawing_mode {
                            continue;
                        }
                        if let Some(pos) = latest_pos {
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

            // While drawing is active, sample pointer position each frame to build
            // the stroke and send live DrawPoint datagrams.
            if self.drawing_mode && self.drawing_active && pointer_down {
                if let Some(pos) = latest_pos {
                    if let Some((hx, hy)) = self.map_pointer_to_host(pos) {
                        let last = self.current_stroke.last().copied();
                        // Only append if the pointer moved at least 2 host pixels.
                        let moved = last.map_or(true, |(lx, ly)| {
                            let dx = (hx as i32 - lx as i32).abs();
                            let dy = (hy as i32 - ly as i32).abs();
                            dx >= 2 || dy >= 2
                        });
                        if moved {
                            self.current_stroke.push((hx, hy));
                            self.send_drawing(DrawingEvent::Point(DrawPoint {
                                peer_id: self.peer_id,
                                x: hx,
                                y: hy,
                            }));
                        }
                    }
                }
            }
        }

        /// Map pointer position to host display coordinates using the cached display rect.
        /// Returns None if the pointer is outside the screen texture area.
        fn map_pointer_to_host(&self, pos: egui::Pos2) -> Option<(u16, u16)> {
            let (tex_w, tex_h) = self.tex_size?;
            let (rect, display_size) = self.display_rect?;
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
        drawing_rx: watch::Receiver<Vec<DrawStroke>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
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
                    frame_rx,
                    cursor_rx,
                    conn_rx,
                    input_tx,
                    drawing_rx,
                    drawing_tx,
                )))
            }),
        )
        .map_err(|e| anyhow::anyhow!("viewer error: {e}"))
    }
}

#[cfg(feature = "gui")]
pub use inner::run_viewer;
