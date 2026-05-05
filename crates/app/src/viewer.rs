//! Viewer UI — renders the remote screen and cursor overlay using egui/wgpu.
//!
//! Only compiled when the `gui` feature is enabled. Requires a display and GPU.
//! On headless targets, use `--no-gui` mode which skips this module entirely.

#[cfg(feature = "gui")]
mod inner {
    use egui::{ColorImage, Context, TextureHandle, TextureOptions, Vec2};
    use pairwire_capture::DecodedFrame;
    use pairwire_protocol::drawing::{DrawMode, DrawPoint, DrawStroke};
    use pairwire_protocol::input::{InputMessage, KeyEvent, MouseClick, MouseMove, MouseScroll};
    use pairwire_protocol::{ControlMessage, ControlRequest, CursorUpdate};
    use pairwire_transport::{DrawingEvent, QualityTier, TimedStroke};
    use std::collections::HashMap;
    use std::time::Instant;
    use tokio::sync::{broadcast, watch};
    use tracing::{debug, info};

    use crate::control_state::RuntimeControlState;
    use crate::reconnect::ConnectionState;

    /// Per-peer fading trail: host-coordinate point + arrival time.
    type LiveTrails = HashMap<u8, Vec<((u16, u16), Instant)>>;

    /// Derive a consistent egui color from a peer ID.
    /// Peer 0 (host) → blue, peer 1 (guest) → red, others → hashed hue.
    fn peer_color(peer_id: u8) -> egui::Color32 {
        match peer_id {
            0 => egui::Color32::from_rgb(80, 140, 255), // host: blue
            1 => egui::Color32::from_rgb(255, 80, 80),  // guest: red
            _ => {
                // Hash peer_id to a hue in [0, 360).
                let hue = (peer_id as f32 * 137.508) % 360.0; // golden angle
                hsv_to_rgb(hue, 0.8, 1.0)
            }
        }
    }

    /// Short display name for a peer shown in the cursor name tag.
    fn peer_label(peer_id: u16) -> String {
        match peer_id {
            0 => "Host".to_string(),
            1 => "Guest".to_string(),
            n => format!("Peer {n}"),
        }
    }

    /// Translate an `egui::Key` to a macOS virtual key code (`kVK_*`).
    ///
    /// egui's `Key` enum discriminants are arbitrary and do not match any platform's
    /// scan codes. The host injects via `CGEvent` which expects `kVK_*` codes, so we
    /// map here. Returns `None` for keys we don't translate (those events are dropped).
    #[allow(clippy::too_many_lines)]
    fn egui_key_to_mac_keycode(key: egui::Key) -> Option<u32> {
        use egui::Key;
        Some(match key {
            // Letters
            Key::A => 0,
            Key::S => 1,
            Key::D => 2,
            Key::F => 3,
            Key::H => 4,
            Key::G => 5,
            Key::Z => 6,
            Key::X => 7,
            Key::C => 8,
            Key::V => 9,
            Key::B => 11,
            Key::Q => 12,
            Key::W => 13,
            Key::E => 14,
            Key::R => 15,
            Key::Y => 16,
            Key::T => 17,
            Key::O => 31,
            Key::U => 32,
            Key::I => 34,
            Key::P => 35,
            Key::L => 37,
            Key::J => 38,
            Key::K => 40,
            Key::N => 45,
            Key::M => 46,
            // Digits
            Key::Num1 => 18,
            Key::Num2 => 19,
            Key::Num3 => 20,
            Key::Num4 => 21,
            Key::Num6 => 22,
            Key::Num5 => 23,
            Key::Num9 => 25,
            Key::Num7 => 26,
            Key::Num8 => 28,
            Key::Num0 => 29,
            // Punctuation
            Key::Equals => 24,
            Key::Minus => 27,
            Key::CloseBracket => 30,
            Key::OpenBracket => 33,
            Key::Quote => 39,
            Key::Semicolon => 41,
            Key::Backslash => 42,
            Key::Comma => 43,
            Key::Slash => 44,
            Key::Period => 47,
            Key::Backtick => 50,
            // Control
            Key::Enter => 36,
            Key::Tab => 48,
            Key::Space => 49,
            Key::Backspace => 51,
            Key::Escape => 53,
            Key::Delete => 117,
            // Navigation
            Key::Home => 115,
            Key::End => 119,
            Key::PageUp => 116,
            Key::PageDown => 121,
            Key::ArrowLeft => 123,
            Key::ArrowRight => 124,
            Key::ArrowDown => 125,
            Key::ArrowUp => 126,
            // Function keys
            Key::F1 => 122,
            Key::F2 => 120,
            Key::F3 => 99,
            Key::F4 => 118,
            Key::F5 => 96,
            Key::F6 => 97,
            Key::F7 => 98,
            Key::F8 => 100,
            Key::F9 => 101,
            Key::F10 => 109,
            Key::F11 => 103,
            Key::F12 => 111,
            Key::F13 => 105,
            Key::F14 => 107,
            Key::F15 => 113,
            Key::F16 => 106,
            Key::F17 => 64,
            Key::F18 => 79,
            Key::F19 => 80,
            Key::F20 => 90,
            _ => return None,
        })
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

    /// egui application state for the screen viewer.
    pub struct ViewerApp {
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        conn_rx: watch::Receiver<ConnectionState>,
        input_tx: broadcast::Sender<InputMessage>,
        drawing_rx: watch::Receiver<Vec<TimedStroke>>,
        live_point_rx: watch::Receiver<Option<DrawPoint>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
        /// Sends mute state to the session (true = muted).
        mute_tx: watch::Sender<bool>,
        /// Receives current connection quality tier from the session.
        quality_rx: watch::Receiver<QualityTier>,
        /// Receives the current control state published by the session.
        control_rx: watch::Receiver<RuntimeControlState>,
        /// Sends ControlMessage events to the session (e.g. ControlRequest).
        control_tx: broadcast::Sender<ControlMessage>,
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
        /// Local mic mute state — mirrored to session via mute_tx.
        muted: bool,
        /// Whether the guest has sent a ControlRequest and is awaiting host approval.
        /// Cleared when `control_rx` changes to a new state (Grant or no-op).
        has_pending_request: bool,
        /// Bitmask of mouse buttons currently held by the guest pointer
        /// (0x01=left, 0x02=right, 0x04=middle). Used to stamp `MouseMove`
        /// events with the correct button state for drag injection.
        held_buttons: u8,
        /// Per-peer trail of recently received live `DrawPoint`s, with arrival time.
        /// Used to render a fading laser trail for the in-flight remote stroke.
        live_trails: LiveTrails,
        /// Last `DrawPoint` we routed into `live_trails` — used to skip duplicate
        /// values when the watch channel hasn't actually changed.
        last_live_point: Option<DrawPoint>,
        /// Last time we emitted a live `DrawPoint` datagram (epoch ms). Used to
        /// throttle outgoing live points to roughly 60 Hz.
        last_live_send_ms: u64,
    }

    impl ViewerApp {
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            frame_rx: watch::Receiver<Option<DecodedFrame>>,
            cursor_rx: watch::Receiver<Option<CursorUpdate>>,
            conn_rx: watch::Receiver<ConnectionState>,
            input_tx: broadcast::Sender<InputMessage>,
            drawing_rx: watch::Receiver<Vec<TimedStroke>>,
            live_point_rx: watch::Receiver<Option<DrawPoint>>,
            drawing_tx: broadcast::Sender<DrawingEvent>,
            mute_tx: watch::Sender<bool>,
            quality_rx: watch::Receiver<QualityTier>,
            control_rx: watch::Receiver<RuntimeControlState>,
            control_tx: broadcast::Sender<ControlMessage>,
        ) -> Self {
            Self {
                frame_rx,
                cursor_rx,
                conn_rx,
                input_tx,
                drawing_rx,
                live_point_rx,
                drawing_tx,
                mute_tx,
                quality_rx,
                control_rx,
                control_tx,
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
                muted: false,
                has_pending_request: false,
                held_buttons: 0,
                live_trails: HashMap::new(),
                last_live_point: None,
                last_live_send_ms: 0,
            }
        }

        fn toggle_mute(&mut self) {
            self.muted = !self.muted;
            let _ = self.mute_tx.send(self.muted);
            debug!(muted = self.muted, "mic mute toggled");
        }

        /// Send a `ControlRequest` to the host and mark local state as pending.
        fn request_control(&mut self) {
            let _ = self
                .control_tx
                .send(ControlMessage::ControlRequest(ControlRequest {
                    peer_id: u16::from(self.peer_id),
                }));
            self.has_pending_request = true;
            info!(peer_id = self.peer_id, "guest sent ControlRequest");
        }

        fn send_drawing(&self, event: DrawingEvent) {
            let _ = self.drawing_tx.send(event);
        }

        fn send_input(&self, msg: InputMessage) {
            // Best-effort send — if no receivers or channel full, silently drop.
            let _ = self.input_tx.send(msg);
        }

        /// Render the status HUD in the bottom-right corner.
        ///
        /// Shows: FPS + quality dot, mute badge (when muted), drawing badge, control state.
        fn render_hud(&self, ui: &egui::Ui) {
            let p = ui.painter();
            let base = ui.max_rect().right_bottom() + Vec2::new(-10.0, -10.0);
            let line_h = 16.0;
            let mut y = base.y;

            // Quality dot + FPS
            let (dot_color, tier_label) = match *self.quality_rx.borrow() {
                QualityTier::Full => (egui::Color32::from_rgb(80, 200, 80), ""),
                QualityTier::Reduced => (egui::Color32::from_rgb(240, 180, 0), " ↓"),
                QualityTier::Degraded => (egui::Color32::from_rgb(220, 60, 60), " ↓↓"),
            };
            if self.fps > 0.0 {
                let fps_text = format!("{:.0} fps{tier_label}", self.fps);
                let text_pos = egui::pos2(base.x, y);
                p.text(
                    text_pos,
                    egui::Align2::RIGHT_BOTTOM,
                    &fps_text,
                    egui::FontId::monospace(11.0),
                    egui::Color32::from_rgba_unmultiplied(200, 200, 200, 180),
                );
                // Quality dot to the left of the text
                let galley = p.layout_no_wrap(
                    fps_text,
                    egui::FontId::monospace(11.0),
                    egui::Color32::WHITE,
                );
                let dot_x = base.x - galley.size().x - 8.0;
                p.circle_filled(egui::pos2(dot_x, y - 5.0), 4.0, dot_color);
                y -= line_h;
            }

            // Mute badge — only shown when muted
            if self.muted {
                p.text(
                    egui::pos2(base.x, y),
                    egui::Align2::RIGHT_BOTTOM,
                    "🎤 muted",
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_rgba_unmultiplied(220, 80, 80, 220),
                );
                y -= line_h;
            }

            // Drawing mode badge
            if self.drawing_mode {
                let mode_label = match self.draw_mode {
                    DrawMode::Laser => "✏ laser",
                    DrawMode::Persistent => "✏ draw",
                };
                p.text(
                    egui::pos2(base.x, y),
                    egui::Align2::RIGHT_BOTTOM,
                    mode_label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_rgba_unmultiplied(255, 200, 80, 220),
                );
                y -= line_h;
            }

            // Control state badge
            let control_state = *self.control_rx.borrow();
            let (ctrl_label, ctrl_color) =
                if self.has_pending_request && control_state == RuntimeControlState::HostActive {
                    (
                        "Awaiting host approval…",
                        egui::Color32::from_rgba_unmultiplied(255, 220, 0, 220),
                    )
                } else {
                    match control_state {
                        RuntimeControlState::HostActive => (
                            "View only",
                            egui::Color32::from_rgba_unmultiplied(180, 180, 180, 180),
                        ),
                        // note: currently unreachable from the guest side — the host does not yet
                        // broadcast a "received your request" ack back to the guest watch channel.
                        // This arm is kept for exhaustiveness and future-proofing: if the host
                        // ever sends a GuestRequested state transition the guest will render it
                        // correctly without a code change.
                        RuntimeControlState::GuestRequested => (
                            "Awaiting host approval…",
                            egui::Color32::from_rgba_unmultiplied(255, 220, 0, 220),
                        ),
                        RuntimeControlState::GuestActive => (
                            "You have control",
                            egui::Color32::from_rgba_unmultiplied(100, 220, 100, 220),
                        ),
                    }
                };
            p.text(
                egui::pos2(base.x, y),
                egui::Align2::RIGHT_BOTTOM,
                ctrl_label,
                egui::FontId::proportional(11.0),
                ctrl_color,
            );
        }

        /// Render the toolbar — a compact strip at the bottom of the viewer.
        ///
        /// Contains: mute toggle, draw mode toggle, draw mode switcher (laser/persistent).
        fn render_toolbar(&mut self, ui: &mut egui::Ui) {
            let rect = ui.max_rect();
            let toolbar_h = 32.0;
            let toolbar_rect = egui::Rect::from_min_size(
                egui::pos2(rect.min.x, rect.max.y - toolbar_h),
                egui::vec2(rect.width(), toolbar_h),
            );

            // Semi-transparent background strip
            ui.painter().rect_filled(
                toolbar_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(20, 20, 20, 160),
            );

            // Place buttons inside the toolbar rect using a child UI.
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(toolbar_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.add_space(8.0);

            // Mute button
            let mic_label = if self.muted {
                "🔇 Unmute"
            } else {
                "🎤 Mute"
            };
            let mic_color = if self.muted {
                egui::Color32::from_rgb(220, 80, 80)
            } else {
                egui::Color32::from_rgba_unmultiplied(200, 200, 200, 200)
            };
            if child
                .add(
                    egui::Button::new(egui::RichText::new(mic_label).color(mic_color).size(12.0))
                        .frame(false),
                )
                .clicked()
            {
                self.toggle_mute();
            }

            child.separator();

            // Request Control button — only visible when guest is in view-only mode
            let ctrl_state = *self.control_rx.borrow();
            if ctrl_state == RuntimeControlState::HostActive && !self.has_pending_request {
                if child
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Request Control")
                                .color(egui::Color32::from_rgba_unmultiplied(200, 200, 200, 200))
                                .size(12.0),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.request_control();
                }
                child.separator();
            }

            // Draw mode toggle
            let draw_label = if self.drawing_mode {
                "✏ Drawing ON"
            } else {
                "✏ Draw"
            };
            let draw_color = if self.drawing_mode {
                egui::Color32::from_rgb(255, 200, 80)
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

            // Draw mode switcher (only visible when drawing is active)
            if self.drawing_mode {
                child.add_space(4.0);
                let laser_active = self.draw_mode == DrawMode::Laser;
                let laser_color = if laser_active {
                    egui::Color32::from_rgb(255, 200, 80)
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
                    egui::Color32::from_rgb(255, 200, 80)
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
                // Clear button — drops only this peer's strokes.
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
                    self.send_drawing(DrawingEvent::Clear(pairwire_protocol::drawing::DrawClear {
                        peer_id: Some(self.peer_id),
                    }));
                    self.current_stroke.clear();
                }

                child.add_space(4.0);
                // Clear all — drops every peer's strokes (host + guest).
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
                    self.send_drawing(DrawingEvent::Clear(pairwire_protocol::drawing::DrawClear {
                        peer_id: None,
                    }));
                    self.current_stroke.clear();
                    self.live_trails.clear();
                }
            }
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

            // Resolve the stroke's display color: prefer the explicit `color`
            // field when the sender supplied a non-zero value, else fall back
            // to a per-peer palette so old senders still render legibly.
            let resolve_color = |stroke: &DrawStroke| -> egui::Color32 {
                if stroke.color != [0, 0, 0] {
                    egui::Color32::from_rgb(stroke.color[0], stroke.color[1], stroke.color[2])
                } else {
                    peer_color(stroke.peer_id)
                }
            };

            let now = Instant::now();
            // Laser strokes fade linearly over LASER_FADE_MS; persistent strokes never fade.
            const LASER_FADE_MS: u128 = 2000;
            const LASER_BASE_ALPHA: f32 = 220.0;
            const PERSISTENT_ALPHA: u8 = 230;

            // Render completed strokes received from the remote peer.
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

            // Render the current in-progress local stroke (local, not yet sent).
            if self.current_stroke.len() >= 2 {
                let base = peer_color(self.peer_id);
                let color =
                    egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 200);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in self.current_stroke.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
            }

            // Render fading live-trail polylines for each remote peer mid-stroke.
            // Each segment fades over LIVE_TRAIL_MS so the trail looks like a laser.
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

        /// Drain newly received `DrawPoint`s from the live preview channel into
        /// per-peer trails, and prune trail entries older than the fade window.
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

            self.pump_live_points();

            // Clear the pending-request flag whenever the control state changes
            // (host responded: either granted or silently reverted to HostActive).
            if self.control_rx.has_changed().unwrap_or(false) {
                self.control_rx.mark_unchanged();
                self.has_pending_request = false;
            }

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

                        // Remote cursor overlay — Tuple-style arrow + name tag
                        if let Some(cursor) = self.cursor_rx.borrow().as_ref()
                            && cursor.visible
                        {
                            {
                                let cx =
                                    rect.min.x + (cursor.x as f32 / tex_size.x) * display_size.x;
                                let cy =
                                    rect.min.y + (cursor.y as f32 / tex_size.y) * display_size.y;
                                let tip = egui::pos2(cx, cy);
                                #[allow(clippy::cast_possible_truncation)]
                                let color = peer_color(cursor.peer_id as u8);
                                let p = ui.painter();

                                // Arrow: filled triangle pointing top-left (standard cursor shape).
                                // tip is the hot-spot (top-left corner of the arrow).
                                let arrow = vec![
                                    tip,
                                    tip + Vec2::new(0.0, 14.0),
                                    tip + Vec2::new(4.0, 10.0),
                                    tip + Vec2::new(8.0, 16.0),
                                    tip + Vec2::new(10.0, 15.0),
                                    tip + Vec2::new(6.0, 9.0),
                                    tip + Vec2::new(11.0, 9.0),
                                ];
                                p.add(egui::Shape::convex_polygon(
                                    arrow,
                                    color,
                                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                                ));

                                // Name tag below the arrow tip.
                                let label = peer_label(cursor.peer_id);
                                let tag_pos = tip + Vec2::new(12.0, 14.0);
                                // Background pill
                                let font = egui::FontId::proportional(11.0);
                                let galley = p.layout_no_wrap(
                                    label.clone(),
                                    font.clone(),
                                    egui::Color32::WHITE,
                                );
                                let pad = Vec2::new(4.0, 2.0);
                                let bg_rect = egui::Rect::from_min_size(
                                    tag_pos - pad,
                                    galley.size() + pad * 2.0,
                                );
                                p.rect_filled(bg_rect, 3.0, color);
                                p.text(
                                    tag_pos,
                                    egui::Align2::LEFT_TOP,
                                    label,
                                    font,
                                    egui::Color32::WHITE,
                                );

                                // Click ripple animation
                                if cursor.click_animation > 0 {
                                    let r = 6.0 + cursor.click_animation as f32 * 2.5;
                                    let a = 200u8.saturating_sub(cursor.click_animation * 18);
                                    p.circle_stroke(
                                        tip,
                                        r,
                                        egui::Stroke::new(
                                            1.5,
                                            egui::Color32::from_rgba_unmultiplied(
                                                color.r(),
                                                color.g(),
                                                color.b(),
                                                a,
                                            ),
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

                    self.render_hud(ui);
                    self.render_toolbar(ui);
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

                        // Cmd/Ctrl+Shift+M — toggle mute from keyboard.
                        if *pressed
                            && *key == egui::Key::M
                            && modifiers.shift
                            && (modifiers.mac_cmd || modifiers.command || modifiers.ctrl)
                        {
                            self.toggle_mute();
                            continue;
                        }

                        // Cmd/Ctrl+Shift+R — request control from keyboard.
                        if *pressed
                            && *key == egui::Key::R
                            && modifiers.shift
                            && (modifiers.mac_cmd || modifiers.command || modifiers.ctrl)
                        {
                            let ctrl_state = *self.control_rx.borrow();
                            if ctrl_state == RuntimeControlState::HostActive
                                && !self.has_pending_request
                            {
                                self.request_control();
                            }
                            continue;
                        }

                        // Escape clears strokes in drawing mode.
                        if *pressed && *key == egui::Key::Escape && self.drawing_mode {
                            self.send_drawing(DrawingEvent::Clear(
                                pairwire_protocol::drawing::DrawClear {
                                    peer_id: Some(self.peer_id),
                                },
                            ));
                            self.current_stroke.clear();
                            self.drawing_active = false;
                            continue;
                        }

                        // Escape cancels a pending control request when not in drawing mode.
                        if *pressed && *key == egui::Key::Escape && self.has_pending_request {
                            self.has_pending_request = false;
                            info!("guest cancelled pending control request");
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
                        let Some(key_code) = egui_key_to_mac_keycode(*key) else {
                            continue;
                        };
                        self.send_input(InputMessage::Key(KeyEvent {
                            key_code,
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
                                    let c = peer_color(self.peer_id);
                                    // Tap-to-dot: a single-point stroke would be dropped by the
                                    // renderer (it needs >=2 points), so synthesise a tiny second
                                    // point so quick taps render as visible dots.
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
                            continue;
                        }

                        if let Some((hx, hy)) = self.map_pointer_to_host(*pos) {
                            let (btn, mask) = match button {
                                egui::PointerButton::Primary => (0u8, 0x01u8),
                                egui::PointerButton::Secondary => (1u8, 0x02u8),
                                egui::PointerButton::Middle => (2u8, 0x04u8),
                                _ => (0u8, 0x01u8),
                            };
                            if *pressed {
                                self.held_buttons |= mask;
                            } else {
                                self.held_buttons &= !mask;
                            }
                            self.send_input(InputMessage::Click(MouseClick {
                                x: hx,
                                y: hy,
                                button: btn,
                                pressed: *pressed,
                                timestamp_ms: now_ms,
                            }));
                        }
                    }

                    egui::Event::PointerMoved(pos) => {
                        if self.drawing_mode {
                            continue;
                        }
                        if let Some((hx, hy)) = self.map_pointer_to_host(*pos) {
                            self.send_input(InputMessage::Move(MouseMove {
                                x: hx,
                                y: hy,
                                buttons: self.held_buttons,
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
            // Live points are throttled: append to the local stroke whenever the
            // pointer moves >=2 host pixels (so the saved stroke stays smooth) but
            // only emit a datagram every ~16 ms so we don't flood the wire.
            const LIVE_POINT_MIN_GAP_MS: u64 = 16;
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
                            if now_ms.saturating_sub(self.last_live_send_ms)
                                >= LIVE_POINT_MIN_GAP_MS
                            {
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
        drawing_rx: watch::Receiver<Vec<TimedStroke>>,
        live_point_rx: watch::Receiver<Option<DrawPoint>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
        mute_tx: watch::Sender<bool>,
        quality_rx: watch::Receiver<QualityTier>,
        control_rx: watch::Receiver<RuntimeControlState>,
        control_tx: broadcast::Sender<ControlMessage>,
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
                    live_point_rx,
                    drawing_tx,
                    mute_tx,
                    quality_rx,
                    control_rx,
                    control_tx,
                )))
            }),
        )
        .map_err(|e| anyhow::anyhow!("viewer error: {e}"))
    }
}

#[cfg(feature = "gui")]
pub use inner::run_viewer;
