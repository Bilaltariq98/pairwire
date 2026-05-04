//! Host-side annotation overlay window.
//!
//! When the `gui` feature is enabled and `--no-overlay` is NOT set, the host
//! gets a floating egui window showing a live mirror of their own screen share
//! (decoded from the capture pipeline) with a drawing canvas on top.
//!
//! **Design choice — mirror window (not transparent overlay):**
//! macOS transparent egui windows are non-trivial: eframe/winit does not expose
//! the `NSWindow` flags needed for per-pixel transparency without forking the
//! crate. Instead we take the simplest path: the host subscribes to a decoded
//! preview of their own capture (via a one-slot watch channel fed from the
//! capture tee in `session.rs`), renders that frame inside a normal window, and
//! draws annotations on top. This is exactly what the guest viewer does — so
//! all the drawing logic mirrors `viewer.rs`.
//!
//! The overlay window:
//! - Shows a live preview of the host's screen share (960×600 by default)
//! - Supports the same laser/persistent drawing modes as the guest viewer
//! - Strokes drawn here are sent to the guest via the existing `DrawingSender`
//! - Strokes received from the guest appear here as well (shared `strokes_rx`)
//! - Cmd+Shift+D toggles drawing mode on/off
//! - Escape clears the host's strokes while in drawing mode
//! - The OS close button exits the overlay without ending the session

#[cfg(feature = "gui")]
mod inner {
    use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
    use pairwire_capture::DecodedFrame;
    use pairwire_protocol::drawing::{DrawClear, DrawMode, DrawPoint, DrawStroke};
    use pairwire_transport::{DrawingEvent, TimedStroke};
    use std::collections::HashMap;
    use std::time::Instant;
    use tokio::sync::{broadcast, watch};
    use tracing::{debug, info};

    /// Per-peer fading trail: host-coordinate point + arrival time.
    type LiveTrails = HashMap<u8, Vec<((u16, u16), Instant)>>;

    use crate::session::HostOverlayChannels;

    /// Derive the host-blue color (peer_id=0).
    fn host_color() -> egui::Color32 {
        egui::Color32::from_rgb(80, 140, 255)
    }

    /// Derive a consistent egui color from a peer ID (same logic as `viewer.rs`).
    fn peer_color(peer_id: u8) -> egui::Color32 {
        match peer_id {
            0 => egui::Color32::from_rgb(80, 140, 255),
            1 => egui::Color32::from_rgb(255, 80, 80),
            _ => {
                let hue = (peer_id as f32 * 137.508) % 360.0;
                hsv_to_rgb(hue, 0.8, 1.0)
            }
        }
    }

    fn hsv_to_rgb(h: f32, s: f32, v: f32) -> egui::Color32 {
        let h = h / 60.0;
        let i = h.floor() as u32 % 6;
        let f = h - h.floor();
        let p = v * (1.0 - s);
        let q = v * (1.0 - f * s);
        let t = v * (1.0 - (1.0 - f) * s);
        let (r, g, b) = match i {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        egui::Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }

    /// egui application state for the host overlay.
    pub struct HostOverlayApp {
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        strokes_rx: watch::Receiver<Vec<TimedStroke>>,
        live_point_rx: watch::Receiver<Option<DrawPoint>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
        texture: Option<TextureHandle>,
        /// Cached texture dimensions for coordinate mapping.
        tex_size: Option<(f32, f32)>,
        /// Cached display rect and size from last render.
        display_rect: Option<(egui::Rect, Vec2)>,
        /// Whether drawing mode is active (toggled with Cmd/Ctrl+Shift+D).
        drawing_mode: bool,
        /// Current draw mode.
        draw_mode: DrawMode,
        /// Points accumulated in the current stroke (host-display coordinates).
        current_stroke: Vec<(u16, u16)>,
        /// Whether the pointer is currently pressed in drawing mode.
        drawing_active: bool,
        /// Host is always peer_id 0.
        peer_id: u8,
        /// FPS tracking.
        frame_count: u64,
        last_fps_time: Instant,
        fps: f32,
        /// Per-peer trail of recently received live `DrawPoint`s.
        live_trails: LiveTrails,
        /// Last `DrawPoint` routed into `live_trails`.
        last_live_point: Option<DrawPoint>,
        /// Last time we emitted a live `DrawPoint` datagram (epoch ms).
        last_live_send_ms: u64,
    }

    impl HostOverlayApp {
        pub fn new(channels: HostOverlayChannels) -> Self {
            Self {
                frame_rx: channels.frame_rx,
                strokes_rx: channels.strokes_rx,
                live_point_rx: channels.live_point_rx,
                drawing_tx: channels.drawing_event_tx,
                texture: None,
                tex_size: None,
                display_rect: None,
                drawing_mode: false,
                draw_mode: DrawMode::Laser,
                current_stroke: Vec::new(),
                drawing_active: false,
                peer_id: 0, // host is always peer 0
                frame_count: 0,
                last_fps_time: Instant::now(),
                fps: 0.0,
                live_trails: HashMap::new(),
                last_live_point: None,
                last_live_send_ms: 0,
            }
        }

        fn send_drawing(&self, event: DrawingEvent) {
            let _ = self.drawing_tx.send(event);
        }

        fn upload_frame(&mut self, ctx: &Context, frame: &DecodedFrame) {
            let w = frame.width as usize;
            let h = frame.height as usize;

            // DecodedFrame is BGRA; egui expects RGBA — swap B<->R.
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
                    self.texture =
                        Some(ctx.load_texture("host-preview", image, TextureOptions::LINEAR));
                }
            }

            self.tex_size = Some((w as f32, h as f32));
            self.frame_count += 1;
            let elapsed = self.last_fps_time.elapsed().as_secs_f32();
            if elapsed >= 1.0 {
                self.fps = self.frame_count as f32 / elapsed;
                self.frame_count = 0;
                self.last_fps_time = Instant::now();
                debug!(fps = self.fps, "host overlay fps");
            }
        }

        fn render_strokes(
            &self,
            ui: &egui::Ui,
            rect: egui::Rect,
            display_size: Vec2,
            tex_size: egui::Vec2,
        ) {
            let strokes = self.strokes_rx.borrow();
            let p = ui.painter();

            let to_screen = |hx: u16, hy: u16| -> egui::Pos2 {
                let rx = hx as f32 / tex_size.x;
                let ry = hy as f32 / tex_size.y;
                egui::pos2(
                    rect.min.x + rx * display_size.x,
                    rect.min.y + ry * display_size.y,
                )
            };

            let resolve_color = |stroke: &DrawStroke| -> egui::Color32 {
                if stroke.color != [0, 0, 0] {
                    egui::Color32::from_rgb(stroke.color[0], stroke.color[1], stroke.color[2])
                } else {
                    peer_color(stroke.peer_id)
                }
            };

            let now = Instant::now();
            const LASER_FADE_MS: u128 = 2000;
            const LASER_BASE_ALPHA: f32 = 220.0;
            const PERSISTENT_ALPHA: u8 = 230;

            for timed in strokes.iter() {
                let stroke = &timed.stroke;
                if stroke.points.len() < 2 {
                    continue;
                }
                let alpha = match stroke.mode {
                    DrawMode::Laser => {
                        let age_ms = now.saturating_duration_since(timed.received_at).as_millis();
                        if age_ms >= LASER_FADE_MS {
                            continue;
                        }
                        let frac = 1.0 - (age_ms as f32 / LASER_FADE_MS as f32);
                        (LASER_BASE_ALPHA * frac).clamp(0.0, 255.0) as u8
                    }
                    DrawMode::Persistent => PERSISTENT_ALPHA,
                };
                let base = resolve_color(stroke);
                let color =
                    egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in stroke.points.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
            }

            // Render current in-progress local stroke.
            if self.current_stroke.len() >= 2 {
                let base = host_color();
                let color =
                    egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 200);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in self.current_stroke.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
            }

            // Render fading live-trail polylines for remote peers.
            const LIVE_TRAIL_MS: u128 = 700;
            for (peer, trail) in self.live_trails.iter() {
                if trail.len() < 2 {
                    continue;
                }
                let base = peer_color(*peer);
                for window in trail.windows(2) {
                    let age_ms = now.saturating_duration_since(window[1].1).as_millis();
                    if age_ms >= LIVE_TRAIL_MS {
                        continue;
                    }
                    let frac = 1.0 - (age_ms as f32 / LIVE_TRAIL_MS as f32);
                    let alpha = (200.0 * frac).clamp(0.0, 255.0) as u8;
                    let color =
                        egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
                    let a = to_screen(window[0].0.0, window[0].0.1);
                    let b = to_screen(window[1].0.0, window[1].0.1);
                    p.line_segment([a, b], egui::Stroke::new(3.0, color));
                }
            }
        }

        fn pump_live_points(&mut self) {
            const LIVE_TRAIL_MS: u128 = 700;
            let current = *self.live_point_rx.borrow();
            if current != self.last_live_point {
                self.last_live_point = current;
                if let Some(pt) = current {
                    let trail = self.live_trails.entry(pt.peer_id).or_default();
                    trail.push(((pt.x, pt.y), Instant::now()));
                }
            }
            let now = Instant::now();
            for trail in self.live_trails.values_mut() {
                trail
                    .retain(|(_, t)| now.saturating_duration_since(*t).as_millis() < LIVE_TRAIL_MS);
            }
            self.live_trails.retain(|_, trail| !trail.is_empty());
        }

        fn render_hud(&self, ui: &egui::Ui) {
            let p = ui.painter();
            let base = ui.max_rect().right_bottom() + Vec2::new(-10.0, -10.0);
            let line_h = 16.0;
            let mut y = base.y;

            if self.fps > 0.0 {
                let fps_text = format!("{:.0} fps (host preview)", self.fps);
                p.text(
                    egui::pos2(base.x, y),
                    egui::Align2::RIGHT_BOTTOM,
                    &fps_text,
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                );
                y -= line_h;
            }

            if self.drawing_mode {
                let mode_label = match self.draw_mode {
                    DrawMode::Laser => "✏ laser (host)",
                    DrawMode::Persistent => "✏ draw (host)",
                };
                p.text(
                    egui::pos2(base.x, y),
                    egui::Align2::RIGHT_BOTTOM,
                    mode_label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_rgba_unmultiplied(80, 140, 255, 220),
                );
                // suppress unused variable warning when drawing_mode is false
                let _ = y;
            }
        }

        fn render_toolbar(&mut self, ui: &mut egui::Ui) {
            let rect = ui.max_rect();
            let toolbar_h = 32.0;
            let toolbar_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x, rect.max.y - toolbar_h),
                egui::vec2(rect.width(), toolbar_h),
            );

            ui.painter().rect_filled(
                toolbar_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(20, 20, 20, 160),
            );

            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(toolbar_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.add_space(8.0);

            let draw_label = if self.drawing_mode {
                "✏ Drawing ON"
            } else {
                "✏ Draw"
            };
            let draw_color = if self.drawing_mode {
                egui::Color32::from_rgb(80, 140, 255)
            } else {
                egui::Color32::from_rgba_unmultiplied(200, 200, 200, 200)
            };
            if child
                .add(
                    egui::Button::new(egui::RichText::new(draw_label).color(draw_color).size(12.0))
                        .frame(false),
                )
                .clicked()
            {
                self.drawing_mode = !self.drawing_mode;
                if !self.drawing_mode {
                    self.current_stroke.clear();
                    self.drawing_active = false;
                }
            }

            if self.drawing_mode {
                child.add_space(4.0);
                let laser_active = self.draw_mode == DrawMode::Laser;
                let laser_color = if laser_active {
                    egui::Color32::from_rgb(80, 140, 255)
                } else {
                    egui::Color32::from_rgba_unmultiplied(160, 160, 160, 180)
                };
                if child
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Laser").color(laser_color).size(11.0),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.draw_mode = DrawMode::Laser;
                }
                let persist_color = if !laser_active {
                    egui::Color32::from_rgb(80, 140, 255)
                } else {
                    egui::Color32::from_rgba_unmultiplied(160, 160, 160, 180)
                };
                if child
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Persistent")
                                .color(persist_color)
                                .size(11.0),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.draw_mode = DrawMode::Persistent;
                }

                child.add_space(4.0);
                if child
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Clear mine")
                                .color(egui::Color32::from_rgba_unmultiplied(200, 100, 100, 200))
                                .size(11.0),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.send_drawing(DrawingEvent::Clear(DrawClear {
                        peer_id: Some(self.peer_id),
                    }));
                    self.current_stroke.clear();
                }

                child.add_space(4.0);
                if child
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Clear all")
                                .color(egui::Color32::from_rgba_unmultiplied(220, 80, 80, 220))
                                .size(11.0),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.send_drawing(DrawingEvent::Clear(DrawClear { peer_id: None }));
                    self.current_stroke.clear();
                    self.live_trails.clear();
                }
            }
        }

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

        fn capture_input_events(&mut self, ctx: &Context) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

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
                            debug!(
                                drawing_mode = self.drawing_mode,
                                "host overlay drawing mode toggled"
                            );
                            continue;
                        }

                        // Escape clears strokes in drawing mode.
                        if *pressed && *key == egui::Key::Escape && self.drawing_mode {
                            self.send_drawing(DrawingEvent::Clear(DrawClear {
                                peer_id: Some(self.peer_id),
                            }));
                            self.current_stroke.clear();
                            self.drawing_active = false;
                            continue;
                        }
                    }

                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        if self.drawing_mode && *button == egui::PointerButton::Primary {
                            if *pressed {
                                self.current_stroke.clear();
                                self.drawing_active = true;
                                if let Some((hx, hy)) = self.map_pointer_to_host(*pos) {
                                    self.current_stroke.push((hx, hy));
                                }
                            } else if self.drawing_active {
                                self.drawing_active = false;
                                let c = host_color();
                                // Tap-to-dot: synthesise a second point for single-click taps.
                                if self.current_stroke.len() == 1 {
                                    let (x, y) = self.current_stroke[0];
                                    self.current_stroke
                                        .push((x.saturating_add(1), y.saturating_add(1)));
                                }
                                if self.current_stroke.len() >= 2 {
                                    let stroke = DrawStroke {
                                        peer_id: self.peer_id,
                                        color: [c.r(), c.g(), c.b()],
                                        points: std::mem::take(&mut self.current_stroke),
                                        mode: self.draw_mode.clone(),
                                    };
                                    self.send_drawing(DrawingEvent::Stroke(stroke));
                                } else {
                                    self.current_stroke.clear();
                                }
                            }
                        }
                    }

                    _ => {}
                }
            }

            // Sample pointer each frame while drawing to build strokes and send live points.
            const LIVE_POINT_MIN_GAP_MS: u64 = 16;
            if self.drawing_mode
                && self.drawing_active
                && pointer_down
                && let Some(pos) = latest_pos
                && let Some((hx, hy)) = self.map_pointer_to_host(pos)
            {
                let last = self.current_stroke.last().copied();
                let moved = last.is_none_or(|(lx, ly)| {
                    let dx = (hx as i32 - lx as i32).abs();
                    let dy = (hy as i32 - ly as i32).abs();
                    dx >= 2 || dy >= 2
                });
                if moved {
                    self.current_stroke.push((hx, hy));
                    if now_ms.saturating_sub(self.last_live_send_ms) >= LIVE_POINT_MIN_GAP_MS {
                        self.last_live_send_ms = now_ms;
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

    impl eframe::App for HostOverlayApp {
        fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
            let new_frame = self.frame_rx.borrow().clone();
            if let Some(frame) = new_frame {
                self.upload_frame(ctx, &frame);
            }
            ctx.request_repaint();

            self.pump_live_points();

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(10, 10, 10)))
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

                        self.render_strokes(ui, rect, display_size, tex_size);
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Waiting for capture to start…\n\
                                    (session must connect a guest first)",
                                )
                                .size(18.0)
                                .color(egui::Color32::GRAY),
                            );
                        });
                    }

                    self.render_hud(ui);
                    self.render_toolbar(ui);
                });

            self.capture_input_events(ctx);
        }
    }

    /// Launch the host overlay window. Blocks until the window is closed.
    ///
    /// On macOS with the `tray` feature, this is called from a background thread
    /// (not the main thread) since the tray event loop owns the main thread.
    /// winit on macOS can handle secondary windows on non-main threads when
    /// NSApplication is already initialised (which the tray does).
    pub fn run_host_overlay(channels: HostOverlayChannels) -> anyhow::Result<()> {
        info!("host overlay starting");
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("pairwire — Host Annotation Overlay")
                .with_inner_size([960.0, 600.0])
                .with_min_inner_size([320.0, 240.0])
                .with_always_on_top(),
            ..Default::default()
        };

        eframe::run_native(
            "pairwire — Host Annotation Overlay",
            options,
            Box::new(|_cc| Ok(Box::new(HostOverlayApp::new(channels)))),
        )
        .map_err(|e| anyhow::anyhow!("host overlay error: {e}"))
    }
}

#[cfg(feature = "gui")]
pub use inner::run_host_overlay;
