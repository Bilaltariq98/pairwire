//! Host-side transparent annotation overlay — spike/proof-of-concept.
//!
//! # Design Goal
//!
//! Replace the "mirror window" from `host_overlay.rs` with a fullscreen
//! **transparent** borderless window that sits on top of the host's desktop.
//! The desktop shows through the window. Strokes are drawn directly over
//! whatever the host is looking at, with no mirror needed.
//!
//! # Spike Status
//!
//! This module proves (or disproves) three claims:
//!
//! 1. `ViewportBuilder::with_transparent(true)` + `with_decorations(false)` +
//!    `Frame::NONE.fill(Color32::TRANSPARENT)` produces a visually transparent
//!    window on macOS where the desktop shows through.
//!
//! 2. `NSWindow.setIgnoresMouseEvents(YES/NO)` can be toggled at runtime via
//!    `objc2-app-kit` (already a dependency) to achieve click-through when the
//!    host is not drawing.
//!
//! 3. The existing drawing logic (laser/persistent, fade, stroke send) works
//!    unchanged with a transparent canvas.
//!
//! # macOS Transparency Mechanism
//!
//! winit 0.30.13 (used by eframe 0.31) already calls:
//!   - `NSWindow.setOpaque(NO)`
//!   - `NSWindow.setBackgroundColor(NSColor.clearColor)`
//!
//! when `WindowAttributes::transparent == true`, which is set by
//! `ViewportBuilder::with_transparent(true)`. This was confirmed in
//! `winit-0.30.13/src/platform_impl/macos/window_delegate.rs` (lines 659–662).
//! The prior agent's claim that "eframe/winit does not expose NSWindow flags
//! needed for per-pixel transparency" was **incorrect**.
//!
//! # Click-Through Mechanism
//!
//! `NSWindow.setIgnoresMouseEvents(YES)` makes the entire window pass all
//! mouse events to the windows below it. We toggle this based on `drawing_mode`:
//! - `drawing_mode = false` → ignores mouse events (host can use their desktop)
//! - `drawing_mode = true` → intercepts mouse events (host draws strokes)
//!
//! We obtain the `NSWindow*` pointer via `eframe::Frame::window_handle()` →
//! `RawWindowHandle::AppKit(handle)` → `NSView` → `NSView.window()`.
//!
//! # Coordinate System
//!
//! In the transparent overlay, the window covers the full primary display at
//! logical resolution. The `drawing canvas == the visible display area`, so
//! pointer coordinates map 1:1 to host display pixel coordinates (after scaling
//! from logical to physical pixels).
//!
//! There is NO texture to fit-scale the way the mirror window does. The overlay
//! uses `ctx.screen_rect()` as the canvas bounds and maps pointer positions to
//! normalised [0, 1] × [0, 1] coordinates, then multiplies by the capture
//! resolution reported via `tex_size` (which comes from the decoded frame
//! dimensions — the host's actual screen resolution).
//!
//! # Multi-Monitor Note
//!
//! `with_fullscreen(true)` covers the primary display only.
//! Covering secondary displays requires spawning additional viewports — out of
//! scope for this spike. Document and flag in the PR.
//!
//! # Focus / Keyboard Interaction
//!
//! A window in `ignoresMouseEvents=YES` mode also loses keyboard focus
//! on macOS (clicking through it focuses whatever is beneath it). When the host
//! presses Cmd+Shift+D to enter drawing mode we call
//! `NSWindow.makeKeyAndOrderFront()` to reclaim focus so keyboard events (Escape,
//! etc.) reach us. On exit from drawing mode we set `ignoresMouseEvents=YES` and
//! do NOT explicitly release focus — the user's next click will naturally focus
//! whatever app is beneath.

#[cfg(feature = "gui")]
mod inner {
    use std::time::Instant;

    use egui::{Color32, Context, Vec2};

    /// Per-peer fading trail: host-coordinate point + arrival time.
    type LiveTrails = HashMap<u8, Vec<((u16, u16), Instant)>>;
    use pairwire_protocol::drawing::{DrawClear, DrawMode, DrawPoint, DrawStroke};
    use pairwire_transport::{DrawingEvent, TimedStroke};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::collections::HashMap;
    use tokio::sync::{broadcast, watch};
    use tracing::{debug, info, warn};

    use crate::session::{HostOverlayChannels, HostSessionState};

    // ── Color helpers (shared with host_overlay.rs) ────────────────────────────

    fn host_color() -> Color32 {
        Color32::from_rgb(80, 140, 255)
    }

    fn peer_color(peer_id: u8) -> Color32 {
        match peer_id {
            0 => Color32::from_rgb(80, 140, 255),
            1 => Color32::from_rgb(255, 80, 80),
            _ => {
                let hue = (peer_id as f32 * 137.508) % 360.0;
                hsv_to_rgb(hue, 0.8, 1.0)
            }
        }
    }

    fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color32 {
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
        Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }

    // ── NSApplication / NSScreen helpers ───────────────────────────────────────

    /// Set the app's activation policy to `.accessory` so the overlay process
    /// does NOT appear in the Dock and does NOT install a menu bar with the app
    /// name. This is what makes Tuple-style overlays feel like part of the OS
    /// rather than a foreground app.
    ///
    /// Must be called on the main thread before `eframe::run_native`.
    #[cfg(target_os = "macos")]
    fn set_accessory_activation_policy() {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        use objc2_foundation::MainThreadMarker;

        let Some(mtm) = MainThreadMarker::new() else {
            warn!("set_accessory_activation_policy called off main thread — skipping");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let ok = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        debug!(
            ok,
            "transparent overlay: set NSApplication activation policy = Accessory"
        );
    }

    #[cfg(not(target_os = "macos"))]
    fn set_accessory_activation_policy() {}

    /// Return the primary display's logical size in points, or `None` if it
    /// cannot be determined.
    #[cfg(target_os = "macos")]
    fn primary_screen_size() -> Option<(f32, f32)> {
        use objc2_app_kit::NSScreen;
        use objc2_foundation::MainThreadMarker;

        let mtm = MainThreadMarker::new()?;
        let screen = NSScreen::mainScreen(mtm)?;
        let frame = screen.frame();
        Some((frame.size.width as f32, frame.size.height as f32))
    }

    #[cfg(not(target_os = "macos"))]
    fn primary_screen_size() -> Option<(f32, f32)> {
        None
    }

    /// Once the NSWindow exists, raise its level above normal windows and let it
    /// follow the user across Spaces / fullscreen apps. This is what makes the
    /// overlay feel "stuck to the screen" rather than a regular app window.
    #[cfg(target_os = "macos")]
    fn configure_overlay_window_chrome(frame: &eframe::Frame) {
        use objc2_app_kit::{NSView, NSWindowCollectionBehavior};

        let handle = match frame.window_handle() {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "could not obtain window handle for chrome setup");
                return;
            }
        };
        let RawWindowHandle::AppKit(appkit_handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: eframe guarantees the pointer is a valid NSView for the lifetime
        // of App::update.
        let ns_view: &NSView = unsafe {
            appkit_handle
                .ns_view
                .as_ptr()
                .cast::<NSView>()
                .as_ref()
                .expect("ns_view pointer was null")
        };
        let Some(ns_window) = ns_view.window() else {
            return;
        };

        // NSStatusWindowLevel = 25 — sits above normal windows and most floating
        // panels but below screen-saver/menu-bar. Matches the "always on top of
        // everything except OS chrome" feel that Tuple uses.
        const NS_STATUS_WINDOW_LEVEL: isize = 25;
        ns_window.setLevel(NS_STATUS_WINDOW_LEVEL);

        // Show on every Space, ride along when the user enters another app's
        // fullscreen, and don't participate in window cycling (Cmd+`).
        let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle;
        ns_window.setCollectionBehavior(behavior);

        debug!("transparent overlay: configured window level + collection behavior");
    }

    #[cfg(not(target_os = "macos"))]
    fn configure_overlay_window_chrome(_frame: &eframe::Frame) {}

    // ── NSWindow click-through helper ──────────────────────────────────────────

    /// Toggle `NSWindow.setIgnoresMouseEvents` on the macOS window behind `frame`.
    ///
    /// When `ignores = true` all pointer events pass through to apps beneath —
    /// the host can use their desktop normally. When `ignores = false` the overlay
    /// captures pointer events so the host can draw strokes.
    ///
    /// # Safety
    ///
    /// The `frame` raw window handle must contain a valid `NSView*` pointer.
    /// eframe guarantees this is the case for the lifetime of the `App::update`
    /// callback.
    #[cfg(target_os = "macos")]
    fn set_window_ignores_mouse_events(frame: &eframe::Frame, ignores: bool) {
        use objc2_app_kit::NSView;

        let handle = match frame.window_handle() {
            Ok(h) => h,
            Err(e) => {
                warn!(error = %e, "could not obtain window handle for click-through toggle");
                return;
            }
        };

        let RawWindowHandle::AppKit(appkit_handle) = handle.as_raw() else {
            warn!("expected AppKit window handle on macOS");
            return;
        };

        // SAFETY: eframe guarantees the pointer is a valid NSView for the lifetime
        // of App::update. We do not retain the reference beyond this function.
        let ns_view: &NSView = unsafe {
            appkit_handle
                .ns_view
                .as_ptr()
                .cast::<NSView>()
                .as_ref()
                .expect("ns_view pointer was null — should not happen in eframe")
        };

        let Some(ns_window) = ns_view.window() else {
            warn!("NSView has no NSWindow — overlay not yet installed in a window");
            return;
        };

        ns_window.setIgnoresMouseEvents(ignores);

        debug!(
            ignores_mouse_events = ignores,
            "transparent overlay: click-through toggled"
        );
    }

    #[cfg(not(target_os = "macos"))]
    fn set_window_ignores_mouse_events(_frame: &eframe::Frame, _ignores: bool) {
        // No-op on non-macOS platforms.
    }

    // ── App state ──────────────────────────────────────────────────────────────

    /// egui application state for the transparent host overlay.
    ///
    /// Unlike `HostOverlayApp` in `host_overlay.rs`, this overlay renders NO
    /// preview frame. The canvas is transparent — the actual desktop shows through.
    /// Strokes are drawn directly on top of the real desktop content.
    pub struct TransparentHostOverlayApp {
        strokes_rx: watch::Receiver<Vec<TimedStroke>>,
        live_point_rx: watch::Receiver<Option<DrawPoint>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
        /// Session state watch — drives the status banner at the top of the overlay.
        /// When the sender is dropped (host process shutting down), the overlay closes.
        state_rx: watch::Receiver<HostSessionState>,

        /// Whether drawing mode is active (toggled with Cmd+Shift+D).
        drawing_mode: bool,
        /// Current draw mode (laser vs persistent).
        draw_mode: DrawMode,
        /// Points accumulated in the current stroke.
        ///
        /// Coordinates are normalised [0, 1] × [0, 1] relative to the overlay canvas
        /// bounds (`ctx.screen_rect()`). They are converted to host pixel coordinates
        /// (u16) when building a `DrawStroke` for transmission.
        current_stroke: Vec<(u16, u16)>,
        /// Whether the pointer is currently pressed in drawing mode.
        drawing_active: bool,
        /// Host is always peer_id 0.
        peer_id: u8,
        /// Per-peer trail of recently received live `DrawPoint`s.
        live_trails: LiveTrails,
        /// Last `DrawPoint` routed into `live_trails`.
        last_live_point: Option<DrawPoint>,
        /// Per-peer time of the most recent `DrawPoint` arrival. Used to keep the
        /// in-progress preview solid (no fade) while the peer is still drawing.
        last_live_arrival: HashMap<u8, Instant>,
        /// Per-peer most-recent stroke `received_at` we have observed in the
        /// stroke cache. When a newer stroke arrives, the live preview for that
        /// peer is cleared — the completed stroke now owns the fade timeline.
        last_stroke_seen: HashMap<u8, Instant>,
        /// Last time we emitted a live `DrawPoint` datagram (epoch ms).
        last_live_send_ms: u64,
        /// The pixel dimensions of the host screen capture (u16 coordinates used
        /// in `DrawStroke`). Populated from the first received stroke or set to a
        /// reasonable default (3024×1964 for a Retina MacBook).
        capture_size: (f32, f32),
        /// Tracks whether click-through was applied correctly after the first frame.
        click_through_init: bool,
        /// Tracks whether NSWindow level / collection behavior were applied.
        chrome_init: bool,
    }

    impl TransparentHostOverlayApp {
        pub fn new(channels: HostOverlayChannels, capture_width: u32, capture_height: u32) -> Self {
            Self {
                strokes_rx: channels.strokes_rx,
                live_point_rx: channels.live_point_rx,
                drawing_tx: channels.drawing_event_tx,
                state_rx: channels.state_rx,
                drawing_mode: false,
                draw_mode: DrawMode::Laser,
                current_stroke: Vec::new(),
                drawing_active: false,
                peer_id: 0,
                live_trails: HashMap::new(),
                last_live_point: None,
                last_live_arrival: HashMap::new(),
                last_stroke_seen: HashMap::new(),
                last_live_send_ms: 0,
                capture_size: (capture_width as f32, capture_height as f32),
                click_through_init: false,
                chrome_init: false,
            }
        }

        fn send_drawing(&self, event: DrawingEvent) {
            let _ = self.drawing_tx.send(event);
        }

        /// Map a pointer position from egui logical coordinates to host pixel
        /// coordinates (u16), clamped to the capture resolution.
        fn map_pointer_to_host_coords(
            &self,
            pos: egui::Pos2,
            canvas: egui::Rect,
        ) -> Option<(u16, u16)> {
            if canvas.width() == 0.0 || canvas.height() == 0.0 {
                return None;
            }
            let rel_x = (pos.x - canvas.min.x) / canvas.width();
            let rel_y = (pos.y - canvas.min.y) / canvas.height();
            if !(0.0..=1.0).contains(&rel_x) || !(0.0..=1.0).contains(&rel_y) {
                return None;
            }
            let hx = (rel_x * self.capture_size.0).clamp(0.0, self.capture_size.0 - 1.0) as u16;
            let hy = (rel_y * self.capture_size.1).clamp(0.0, self.capture_size.1 - 1.0) as u16;
            Some((hx, hy))
        }

        /// Render all strokes received from the broadcast channel as overlays on the
        /// transparent canvas. The canvas is the full `ctx.screen_rect()`.
        fn render_strokes(&self, ui: &egui::Ui, canvas: egui::Rect) {
            let strokes = self.strokes_rx.borrow();
            let p = ui.painter();

            let to_screen = |hx: u16, hy: u16| -> egui::Pos2 {
                let rx = hx as f32 / self.capture_size.0;
                let ry = hy as f32 / self.capture_size.1;
                egui::pos2(
                    canvas.min.x + rx * canvas.width(),
                    canvas.min.y + ry * canvas.height(),
                )
            };

            let resolve_color = |stroke: &DrawStroke| -> Color32 {
                if stroke.color != [0, 0, 0] {
                    Color32::from_rgb(stroke.color[0], stroke.color[1], stroke.color[2])
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
                let color = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in stroke.points.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
            }

            // Current in-progress local stroke.
            if self.current_stroke.len() >= 2 {
                let base = host_color();
                let color = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 200);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in self.current_stroke.windows(2) {
                    let a = to_screen(window[0].0, window[0].1);
                    let b = to_screen(window[1].0, window[1].1);
                    p.line_segment([a, b], egui_stroke);
                }
            }

            // Live-trail polylines for remote peers — the in-progress preview
            // of a stroke that hasn't yet arrived as a completed DrawStroke.
            // Always solid, never fades by age. Cleared in `pump_live_points`
            // when the completed stroke arrives (or after PREVIEW_STALE_MS as a
            // crash-safety fallback).
            const PREVIEW_ALPHA: u8 = 200;
            for (peer, trail) in self.live_trails.iter() {
                if trail.len() < 2 {
                    continue;
                }
                let base = peer_color(*peer);
                let color =
                    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), PREVIEW_ALPHA);
                let egui_stroke = egui::Stroke::new(3.0, color);
                for window in trail.windows(2) {
                    let a = to_screen(window[0].0.0, window[0].0.1);
                    let b = to_screen(window[1].0.0, window[1].0.1);
                    p.line_segment([a, b], egui_stroke);
                }
            }
        }

        fn pump_live_points(&mut self) {
            let now = Instant::now();

            // 1. Ingest the latest live point datagram (if changed).
            let current = *self.live_point_rx.borrow();
            if current != self.last_live_point {
                self.last_live_point = current;
                if let Some(pt) = current {
                    let trail = self.live_trails.entry(pt.peer_id).or_default();
                    trail.push(((pt.x, pt.y), now));
                    self.last_live_arrival.insert(pt.peer_id, now);
                }
            }

            // 2. Detect newly-arrived completed strokes and retire that peer's
            //    live preview — the stroke now owns the fade timeline.
            {
                let strokes = self.strokes_rx.borrow();
                for ts in strokes.iter() {
                    let peer = ts.stroke.peer_id;
                    let prev = self.last_stroke_seen.get(&peer).copied();
                    if prev.is_none_or(|p| ts.received_at > p) {
                        self.last_stroke_seen.insert(peer, ts.received_at);
                        // Only clear preview if the stroke is genuinely newer
                        // than the live points that built the preview. Without
                        // this guard, replaying cached strokes on first frame
                        // would wipe an in-progress preview from another peer.
                        let preview_arrival = self.last_live_arrival.get(&peer).copied();
                        if preview_arrival.is_some_and(|a| ts.received_at >= a) {
                            self.live_trails.remove(&peer);
                            self.last_live_arrival.remove(&peer);
                        }
                    }
                }
            }

            // 3. Drop the entire preview if nothing new has arrived for a long
            //    time. The completed stroke would normally clear the preview
            //    via step 2; this is just a safety net in case the peer
            //    disconnects or crashes mid-stroke.
            //
            //    Note: we deliberately do NOT fade individual points by age.
            //    The guest only sends DrawPoints when the cursor MOVES; if they
            //    hold the mouse still, no points arrive but the stroke is still
            //    very much in progress. Fading by age caused the host preview
            //    to vanish whenever the guest paused with the pen down.
            const PREVIEW_STALE_MS: u128 = 5_000;
            self.last_live_arrival.retain(|peer, t| {
                let stale = now.saturating_duration_since(*t).as_millis() >= PREVIEW_STALE_MS;
                if stale {
                    self.live_trails.remove(peer);
                }
                !stale
            });
        }

        /// Render the session-state status banner at the top-center of the overlay.
        ///
        /// Shows "Waiting for guest to connect…" or "Connected: <peer_id>".
        /// Uses a semi-transparent pill that does NOT block click-through (it is
        /// painted by the painter, not an egui widget, so hit-test is unaffected).
        fn render_session_banner(&self, ui: &egui::Ui, canvas: egui::Rect) {
            let state = self.state_rx.borrow();
            let label = match &*state {
                HostSessionState::Waiting => "Waiting for guest to connect…".to_owned(),
                HostSessionState::Connected { guests } => {
                    let ids = guests.join(", ");
                    format!("{} guest(s) connected: {ids}", guests.len())
                }
            };
            drop(state);

            let p = ui.painter();
            let font = egui::FontId::proportional(12.0);
            let text_color = Color32::from_rgb(200, 200, 200);
            let galley = p.layout_no_wrap(label.clone(), font.clone(), text_color);
            let pad = Vec2::new(10.0, 4.0);
            let label_size = galley.size() + pad * 2.0;
            // Position: top-center, a little below the menu bar.
            let label_x = canvas.center().x - label_size.x / 2.0;
            let label_y = canvas.min.y + 6.0;
            let label_pos = egui::pos2(label_x, label_y);
            let bg_rect = egui::Rect::from_min_size(label_pos, label_size);
            p.rect_filled(
                bg_rect,
                6.0,
                Color32::from_rgba_unmultiplied(10, 10, 10, 140),
            );
            p.text(
                label_pos + pad,
                egui::Align2::LEFT_TOP,
                label,
                font,
                text_color,
            );
        }

        /// Render the minimal HUD showing drawing mode status.
        ///
        /// In the transparent overlay the HUD uses a small semi-opaque pill so it
        /// is readable against any desktop background.
        fn render_hud(&self, ui: &egui::Ui, canvas: egui::Rect) {
            if !self.drawing_mode {
                return;
            }
            let p = ui.painter();
            let mode_label = match self.draw_mode {
                DrawMode::Laser => "✏ laser — host drawing (Cmd+Shift+D to stop)",
                DrawMode::Persistent => "✏ draw — host drawing (Cmd+Shift+D to stop)",
            };
            let font = egui::FontId::proportional(13.0);
            let galley = p.layout_no_wrap(mode_label.to_owned(), font.clone(), Color32::WHITE);
            let pad = Vec2::new(8.0, 4.0);
            let label_size = galley.size() + pad * 2.0;
            let label_pos = egui::pos2(canvas.center().x - label_size.x / 2.0, canvas.min.y + 16.0);
            let bg_rect = egui::Rect::from_min_size(label_pos, label_size);
            p.rect_filled(
                bg_rect,
                4.0,
                Color32::from_rgba_unmultiplied(20, 20, 20, 180),
            );
            p.text(
                label_pos + pad,
                egui::Align2::LEFT_TOP,
                mode_label,
                font,
                Color32::from_rgb(80, 200, 80),
            );
        }

        fn capture_input_events(
            &mut self,
            ctx: &Context,
            canvas: egui::Rect,
            frame: &mut eframe::Frame,
        ) {
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
                        // Cmd+Shift+D — toggle drawing mode.
                        if *pressed
                            && *key == egui::Key::D
                            && modifiers.shift
                            && (modifiers.mac_cmd || modifiers.command || modifiers.ctrl)
                        {
                            self.drawing_mode = !self.drawing_mode;
                            self.current_stroke.clear();
                            self.drawing_active = false;

                            // Toggle click-through based on drawing mode.
                            // drawing_mode = true  → capture mouse (draw)
                            // drawing_mode = false → pass through mouse (use desktop)
                            set_window_ignores_mouse_events(frame, !self.drawing_mode);

                            info!(
                                drawing_mode = self.drawing_mode,
                                "transparent overlay: drawing mode toggled"
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
                                if let Some((hx, hy)) =
                                    self.map_pointer_to_host_coords(*pos, canvas)
                                {
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
                && let Some((hx, hy)) = self.map_pointer_to_host_coords(pos, canvas)
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

    impl eframe::App for TransparentHostOverlayApp {
        /// Clear the wgpu surface with a fully-transparent color every frame.
        ///
        /// `eframe`'s default returns the egui visuals' window fill (opaque dark
        /// gray), which causes the entire desktop to look dimmed under the
        /// overlay even though `viewport.transparent = true` and the central
        /// panel frame is `Color32::TRANSPARENT`. Returning [0, 0, 0, 0] here
        /// preserves the alpha channel so the desktop shows through cleanly.
        fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
            [0.0, 0.0, 0.0, 0.0]
        }

        fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
            ctx.request_repaint();
            self.pump_live_points();

            // On the first frame, ensure the overlay starts in click-through mode.
            // We do this here rather than at construction because the NSWindow
            // pointer is not valid until the first update() call.
            if !self.chrome_init {
                configure_overlay_window_chrome(frame);
                self.chrome_init = true;
            }
            if !self.click_through_init {
                set_window_ignores_mouse_events(frame, true);
                self.click_through_init = true;
                debug!("transparent overlay: initial click-through enabled");
            }

            // When the session_tx is dropped (run_host returning / tray quit),
            // the watch channel is closed. Treat that as a signal to close the
            // overlay window so the process exits cleanly.
            if self.state_rx.has_changed().is_err() {
                info!("session state channel closed — closing overlay window");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }

            let canvas = ctx.screen_rect();

            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
                .show(ctx, |ui| {
                    self.render_strokes(ui, canvas);
                    self.render_session_banner(ui, canvas);
                    self.render_hud(ui, canvas);
                });

            self.capture_input_events(ctx, canvas, frame);
        }
    }

    /// Launch the transparent host overlay window. Blocks until the window is closed.
    ///
    /// # Arguments
    ///
    /// * `channels` — the drawing and live-point channels from the session.
    /// * `capture_width` / `capture_height` — the capture resolution (host screen
    ///   in physical pixels). Used to map overlay pointer positions to `DrawStroke`
    ///   coordinates. Should match what `scap` reports as the first frame dimensions
    ///   (see LEARNINGS.md — Retina displays capture at 2× logical resolution).
    ///
    /// # Transparency
    ///
    /// `ViewportBuilder::with_transparent(true)` instructs winit to call
    /// `NSWindow.setOpaque(NO)` + `NSWindow.setBackgroundColor(NSColor.clearColor)`,
    /// which on macOS allows the framebuffer alpha channel to composite against the
    /// desktop. Combined with `Frame::NONE.fill(Color32::TRANSPARENT)`, the egui
    /// canvas itself is transparent. Only the strokes (painted with non-zero alpha)
    /// are visible.
    ///
    /// # Click-Through
    ///
    /// The window starts in click-through mode (`ignoresMouseEvents = YES`) so the
    /// host can use their desktop normally. Press Cmd+Shift+D to enter drawing mode,
    /// which disables click-through so pointer events reach the overlay and build
    /// strokes. Press Cmd+Shift+D again (or Escape) to exit drawing mode and
    /// restore click-through.
    pub fn run_transparent_host_overlay(
        channels: HostOverlayChannels,
        capture_width: u32,
        capture_height: u32,
    ) -> anyhow::Result<()> {
        info!(
            capture_width,
            capture_height, "transparent host overlay starting"
        );

        // Hide from Dock + suppress menu-bar app name BEFORE the window opens.
        // Otherwise macOS briefly shows "pairwire" in the menu bar and a Dock
        // icon, breaking the "feels like part of the OS" illusion.
        set_accessory_activation_policy();

        // Size the window to the primary display in logical points. We avoid
        // `with_fullscreen(true)` because that puts the overlay into macOS true
        // fullscreen mode (own Space, dock auto-hide, app-name menu bar) — which
        // is exactly the "feels like an app" experience we want to avoid.
        let (w, h) = primary_screen_size().unwrap_or((1440.0, 900.0));

        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("pairwire-overlay")
                // Transparent: winit calls NSWindow.setOpaque(NO) + setBackgroundColor(clear).
                .with_transparent(true)
                // No title bar or chrome.
                .with_decorations(false)
                // Always on top so it sits above other windows.
                .with_always_on_top()
                // Borderless window matching the primary display, anchored at
                // (0, 0). Combined with the NSWindow level + collection-behavior
                // tweaks applied on first frame, this gives a Tuple-style overlay
                // that floats above every Space without taking one over.
                .with_inner_size([w, h])
                .with_position([0.0, 0.0])
                .with_resizable(false)
                .with_taskbar(false),
            // The wgpu renderer must also be set up for transparency.
            // This passes `transparent = true` to `Painter::new`.
            ..Default::default()
        };

        eframe::run_native(
            "pairwire — Transparent Overlay",
            options,
            Box::new(move |_cc| {
                Ok(Box::new(TransparentHostOverlayApp::new(
                    channels,
                    capture_width,
                    capture_height,
                )))
            }),
        )
        .map_err(|e| anyhow::anyhow!("transparent host overlay error: {e}"))
    }
}

#[cfg(feature = "gui")]
pub use inner::run_transparent_host_overlay;
