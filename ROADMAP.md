 Position & Roadmap Insights

  Where You Are

  Phase 0-3 are ~85% complete. The core infrastructure is solid:
  - Transport layer (iroh QUIC, moq-lite, stream routing) — done
  - Screen capture → encode → transmit → decode → render pipeline — working end-to-end
  - Audio capture + Opus encode → moq-lite transport → decode — working
  - Reconnection with exponential backoff — done
  - Viewer with cursor overlay rendering — done
  - Protocol types for input, cursor, clipboard, control — defined and tested

  The remaining 15% is the "last mile" wiring — connecting existing code complete modules (cursor polling, input injection,
  clipboard sync, quality adaptation) into the session loop.

  The Critical Gap

  You have a screen viewer, not a pair programming tool yet. The gap is Phase 3.5 — the interaction layer. Without it, pairwire
   is a read-only screen share. With it, you have Tuple's core UX.

  The good news: the protocol types and transport primitives for P3.5 already exist. The work is mostly session wiring + viewer
   rendering, not new infrastructure.

  Recommended Priority Order

  Tier 1 — Make it usable (1-2 weeks)
  1. P3.5.1 (Bidirectional Cursors) — Fix P1.8's backwards wiring, add host cursor polling via CGEvent. This is the single
  highest-impact change: you go from "viewer" to "I can see where they're pointing."
  2. P3.5.2 (Control Takeover) — Wire the existing InputSender/InputReceiver + ControlHandoff into session. Guest can drive.
  This is pair programming.
  3. P4.7 (Config System) — Foundation for all permission/settings work. Every subsequent feature needs it.

  Tier 2 — Make it complete (2-3 weeks)
  4. P4.4-P4.5 (Tunneling) — ~500 LoC, spec is tight, enormous UX win. "Share my localhost:3000" with zero extra tools.
  5. P3.5.5 (Clipboard Sync) — Code complete, just session wiring.
  6. P3.5.6 (Quality Adaptation) — Code complete, just session wiring. Makes it work on bad networks.

  Tier 3 — Make it delightful (2-3 weeks)
  7. P3.5.3 (Drawing) — Greenfield but the datagram/viewer patterns are established.
  8. P3.5.4 (Reactions) — Small, fun, follows same patterns.
  9. P4.1-P4.3 (Terminal Sharing) — The differentiator vs Tuple. Bigger lift but high-value.

  Tier 4 — Scale it (4+ weeks)
  10. P5.1 (Multi-Party Star) — Architecture is ready (moq-lite fan-out is natural), but this is a meaningful chunk of session
  orchestration.
  11. P5.2-P5.3 (Mesh Audio, SFU) — Only needed once multi-party works.

  Strategic Observations

  1. Your competitive moat is tunneling + terminal sharing + P2P privacy. Tuple has cursors, control, drawing — but no terminal
   sharing, no port tunneling, and routes traffic through their servers. Hit Tier 1-2 and you already have a differentiated
  product.
  2. The "code complete but not wired" pattern is your biggest risk. P2.3 (input), P2.4 (clipboard), P2.5 (quality adaptation)
  all say "code complete" but aren't functional. This creates a false sense of progress. I'd recommend a focused sprint to wire
   all three into the session before adding new features — it turns 3 "code complete" items into 3 actually working features
  with minimal effort.
  3. Config system (P4.7) is a hidden blocker. Permissions, tunnel defaults, terminal defaults, audio settings — everything in
  Tier 2+ needs config. Build it early. It's also what makes the CLI feel like a real tool vs a demo.
  4. macOS-first is the right call. CGEvent for cursor, ScreenCaptureKit for capture, CoreAudio for audio — all working. Don't
  touch cross-platform until the macOS experience is polished through Tier 2.
  5. Multi-party (Phase 5) is further away than it looks. The transport handles it, but session orchestration (peer list
  management, control handoff among N peers, cursor aggregation/relay, disconnect handling) is where the complexity lives. Get
  1:1 perfect first.
  6. Recording (P5.6) was right to deprioritize. It's a nice-to-have that doesn't affect the core pairing UX. The muxing work
  (H.264 + Opus → MP4) is also non-trivial.

  Bottom Line

  You're past the hardest part — the capture/encode/transport/decode pipeline is working. What remains is largely wiring and
  UX. The path from "working demo" to "usable tool" is Tier 1 (P3.5.1 → P3.5.2 → P4.7), which is maybe a week of focused work.
  The path to "competitive product" adds Tier 2, which is another 2-3 weeks. That's a strong position.
