# Research 7: UX & Interaction Design

## Design Philosophy

pairwire serves two audiences: **terminal power users** (Neovim, tmux, CLI-first) and **GUI-comfortable developers** (VS Code, IntelliJ, prefer visual tools). The UX must work for both without compromising either.

Principles:
1. **Terminal-first**: Everything works without a GUI. The CLI is the primary interface.
2. **Stays out of the way**: Minimal chrome during sessions. The tool disappears when you're in flow.
3. **Safe defaults**: Security-first permissions. Nothing shared until explicitly enabled.
4. **Equal participants**: Both peers can point, draw, react, and take control. Not a one-way broadcast.

## 1. Three Interaction Surfaces

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  1. CLI          Primary interface. Everything accessible.  │
│                  Power users may never open a window.       │
│                                                             │
│  2. Session Bar  Thin floating toolbar during sessions.     │
│                  Status, quick controls, mode switching.    │
│                  Both host and guest get one.               │
│                                                             │
│  3. Viewer       egui/wgpu window for screen share.         │
│                  Guest only. Shows remote screen + overlays.│
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## 2. CLI Interface

The CLI is the **primary** interface. Everything pairwire does is accessible from the terminal.

### Command Tree

```
pairwire host [OPTIONS]
    --fps <N>              Frame rate (default: 30)
    --width <W>            Encode width (default: 1920)
    --height <H>           Encode height (default: 1080)
    --tunnel <PORTS>       Pre-approve tunnel ports (comma-separated)
    --terminal             Share a terminal session
    --no-gui               Headless mode (no session bar)
    --no-audio             Disable audio

pairwire join <INVITE_URL> [OPTIONS]
    --no-gui               Headless mode (no viewer, no session bar)
    --no-audio             Disable audio
    --tunnel <PORTS>       Request tunnels for these host ports

pairwire terminal ls                   List available terminal sessions
pairwire terminal attach <ID>          Attach to a session in your real terminal
pairwire terminal attach <ID> --rw     Request read-write access
pairwire terminal detach <ID>          Detach from a session

pairwire tunnel ls                     List active tunnels
pairwire tunnel add <PORT>             Request a new tunnel
pairwire tunnel stop <PORT>            Stop a tunnel

pairwire config                        Open config in $EDITOR
pairwire config get <KEY>              Show a config value
pairwire config set <KEY> <VALUE>      Set a config value
pairwire config edit                   Open config in $EDITOR

pairwire status                        Show current session info
```

### Example Output

```
$ pairwire host --tunnel 3000,5432 --terminal

pairwire v0.2.0

  Invite link:
    pairwire://abc123?relay=https://euc1-1.relay.iroh.link/&token=deadbeef

  Sharing: screen (1080p30), audio, terminal (zsh)
  Tunnels: :3000 (pre-approved), :5432 (pre-approved)

  Waiting for guest to connect…

  ✓ alice connected
  ✓ Audio active
  ✓ Tunnel :3000 → alice:3000 active
  ✓ Tunnel :5432 → alice:5432 active
```

```
$ pairwire join 'pairwire://abc123...' --tunnel 3000

  Connected to bilal (1080p30, audio)

  ✓ Screen sharing active
  ✓ Audio active
  ✓ Tunnel localhost:3000 → host:3000 active
  ✓ 1 terminal session available (run: pairwire terminal ls)

  Opening viewer...
```

## 3. Session Bar

A thin floating toolbar (~30px), always-on-top, docked to the top edge of the screen. Both host and guest get one. This replaces the macOS-only system tray as the primary in-session UI.

### Host Session Bar

```
┌──────────────────────────────────────────────────────────────────┐
│ 🟢 Sharing with alice  │ 🖥 Screen │ >_ Term │ 🔗 Tunnels │ ⚙ │ ✕ │
└──────────────────────────────────────────────────────────────────┘
```

- **Status**: Connection quality dot (🟢 green / 🟡 yellow / 🔴 red) + peer name
- **Screen tab**: Shows resolution, FPS, quality tier
- **Term tab**: Terminal sessions list, create new, set permissions
- **Tunnels tab**: Active tunnels, add/remove
- **⚙ Settings**: Permission toggles, audio controls
- **✕ Disconnect**: End session

### Guest Session Bar

```
┌──────────────────────────────────────────────────────────────────────────┐
│ 🟢 Connected to bilal │ 🎤 │ ✏️ │ 😀 │ 🖥 Screen │ >_ Term │ 🔗 Tunnels │ ⚙ │
└──────────────────────────────────────────────────────────────────────────┘
```

- **🎤 Mute**: Toggle microphone
- **✏️ Draw**: Toggle drawing mode (laser pointer / persistent)
- **😀 React**: Open reaction picker
- **Tabs**: Same as host but with guest-specific options (request control, attach terminal)

## 4. Viewer Window (Guest)

The egui/wgpu window displays the remote screen with overlays.

```
┌─────────────────────────────────────────────────────────┐
│ Session Bar (docked above or integrated)                │
├─────────────────────────────────────────────────────────┤
│                                                         │
│              Remote Screen (scaled to fit)               │
│                                                         │
│         🔴 Host cursor (in captured video)              │
│         🔵 Guest cursor (overlay)                       │
│                                                         │
│         ✏️ Drawing strokes (overlay)                     │
│         😀 Reaction animations (overlay)                │
│                                                         │
│                                                         │
│                                                         │
│                                      ┌────────────────┐ │
│                                      │ 119 fps  🟢    │ │
│                                      │ You have ctrl  │ │
│                                      └────────────────┘ │
├─────────────────────────────────────────────────────────┤
│ Terminal Panel (optional, toggled via Tab key)           │
│ ┌─ zsh ~/project ── [RW] ──┐┌─ npm run dev ── [RO] ──┐ │
│ │ $ cargo test              ││ ready on localhost:3000 │ │
│ │ running 42 tests...       ││ ✓ compiled in 200ms    │ │
│ │ test result: ok           ││                        │ │
│ └───────────────────────────┘└────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

### Overlay Layers (bottom to top)
1. Screen texture (remote desktop)
2. Drawing strokes (both peers)
3. Cursor overlays (guest = blue crosshair, host = already in video)
4. Reaction animations (floating emoji)
5. Status badges (FPS, control indicator, quality)

## 5. Terminal UX — Two Paths

### Path A: Native Terminal (Power Users) — Primary

```
$ pairwire terminal attach 1

Attaching to session 1 (zsh ~/project) [RW]...
Connected. Type normally. Ctrl+D to detach.

~/project $ cargo test
   Compiling pairwire v0.1.0
    Finished test [unoptimized] target(s)
     Running 42 tests
test result: ok. 42 passed
```

The guest's real terminal. Their fonts, keybinds, tmux setup, shell config. pairwire bridges PTY bytes over QUIC — exactly how SSH works. This is the recommended path for developers who live in the terminal.

**Implementation**: Spawn a local PTY, pipe QUIC stream bytes to/from it. The guest's terminal emulator does all the rendering.

### Path B: In-Viewer Terminal Panel (GUI Users) — Convenience

A split view in the egui viewer showing terminal output below the screen share. Like VS Code's integrated terminal — good enough for watching logs and light interaction, not meant to replace a real terminal.

```
┌─ Terminals ────────────────────────────────────────────┐
│ [1. zsh ~/project RW] [2. npm run dev RO] [+ New]     │
├────────────────────────────────────────────────────────┤
│ ~/project $ cargo test                                  │
│    Compiling pairwire v0.1.0                           │
│     Finished test [unoptimized] target(s)              │
│      Running 42 tests                                  │
│ test result: ok. 42 passed; 0 failed                   │
│                                                        │
│ ~/project $ █                                          │
└────────────────────────────────────────────────────────┘
```

**Implementation**: `alacritty_terminal` or `vte` crate for ANSI parsing. Render terminal grid in egui. Support 24-bit color, Unicode, cursor, 10k line scrollback.

**Both paths coexist.** One user on Path A, another on Path B, same session. Path A is recommended, Path B is convenient.

## 6. Tunnel UX

### Configuration

Tunnels are **explicit opt-in** (security-first).

Host decides which ports to offer:
```
$ pairwire host --tunnel 3000,8080
```

Or in config:
```toml
[tunnels]
default_ports = [3000, 8080]
```

### Guest Experience

```
$ pairwire join 'pairwire://...' --tunnel 3000

  ✓ Tunnel localhost:3000 → host:3000 active

  # Now open http://localhost:3000 in your browser
```

### Session Bar Panel

```
┌─ Tunnels ─────────────────────────┐
│                                    │
│  :3000 → host:3000  ● active      │
│  :5432 → host:5432  ● active      │
│  :8080 → host:8080  ○ stopped     │
│                                    │
│  [+ Add tunnel]                    │
└────────────────────────────────────┘
```

## 7. Settings & Config

Three layers with clear precedence:

```
1. CLI flags (highest)     --fps 60
2. Config file (middle)    ~/.config/pairwire/config.toml
3. Hardcoded defaults      CaptureConfig::default()
```

### Config File

```toml
# ~/.config/pairwire/config.toml
# Created on first run with commented defaults.

[identity]
name = "bilal"                      # Display name shown to peers

[capture]
fps = 30
max_resolution = "1080p"            # "720p" | "1080p" | "native"
preferred_encoder = "hardware"      # "hardware" | "software"
keyframe_interval_secs = 2

[audio]
enabled = true
input_device = "default"            # or specific device name
# noise_suppression = false         # future

[permissions]
default_remote_control = "denied"   # "denied" | "view_only" | "keyboard_only" | "full_control"
auto_accept_control = false         # auto-grant control requests
clipboard_sync = "text_only"        # "off" | "text_only" | "all"
max_clipboard_size_mb = 1

[terminal]
default_shell = "$SHELL"
default_permission = "read_only"    # "read_only" | "read_write"
scrollback_lines = 10000
share_env = false                   # sanitize env for security

[tunnels]
auto_detect = false                 # auto-detect listening ports
default_ports = []                  # always offer these when hosting
allow_database_ports = false        # block 3306, 5432, 6379 by default
auto_accept = false                 # require host approval

[ui]
session_bar_position = "top"        # "top" | "bottom"
cursor_color = "blue"               # your cursor color as seen by peer
theme = "system"                    # "light" | "dark" | "system"
```

### CLI Config Management

```
$ pairwire config                   # opens in $EDITOR
$ pairwire config get capture.fps   # → 30
$ pairwire config set capture.fps 60
$ pairwire config get permissions   # shows full section
```

## 8. Permission Model

**Security-first defaults.** Nothing is shared beyond screen + audio until explicitly enabled.

| Feature | Default | Options | Rationale |
|---------|---------|---------|-----------|
| Screen sharing | On (host) | on / off | Core feature |
| Audio | On | on / off | Core feature |
| Remote control | **Denied** | denied / view_only / keyboard_only / full_control | Must be explicitly granted |
| Control requests | **Require approval** | require_approval / auto_accept | Prevent surprise takeover |
| Clipboard sync | **Text only** | off / text_only / all | Prevent accidental file transfer |
| Clipboard size | 1 MB max | configurable | Prevent large data leaks |
| Terminal access | **Read-only** | read_only / read_write | Prevent accidental commands |
| Tunnel ports | **Explicit only** | offered ports only | No port scanning |
| Database ports | **Blocked** | blocked / allowed | Prevent accidental DB exposure |
| Tunnel approval | **Required** | require / auto_accept | Host controls what's exposed |
| Environment | **Sanitized** | sanitized ($SHELL only) / full | Prevent env var leaks |

## 9. Multi-Party UX (3-5 Peers)

When multiple guests connect:

```
┌────────────────────────────────────────────────────────────────────┐
│ 🟢 Sharing with alice, bob, carol │ 🖥 Screen │ >_ Term │ ⚙ │ ✕ │
└────────────────────────────────────────────────────────────────────┘
```

- **Distinct cursor colors**: Host = red, Guest 1 = blue, Guest 2 = green, Guest 3 = purple
- **One broadcaster**: Host shares screen to all. No guest-to-guest screen sharing.
- **One driver**: Only one person has keyboard/mouse control at a time. Badge shows who: "alice has control"
- **Everyone can draw**: All peers can draw simultaneously with distinct colors.
- **Everyone can react**: Reactions visible to all.
- **Peer list**: Session bar shows all connected peers with quality indicators.
- **Audio**: Everyone hears everyone (star relay through host initially, mesh later).
- **Terminals**: Each session can be shared to specific peers or all.

## 10. Privacy & Security Stance

> "We send only the contents of your screen and audio during calls. We never send complete files over the network."
> — Tuple (for comparison)

pairwire goes further:

- **P2P**: No server sees your data. Streams are encrypted end-to-end via QUIC TLS 1.3.
- **Even relayed traffic is encrypted**: iroh's relay forwards opaque encrypted packets. It cannot read the content.
- **No file transfer** unless explicit (clipboard sync with size limits, tunnel with port approval).
- **Tunnels are explicit opt-in**: Host chooses which ports. Guest can only request offered ports.
- **Terminal env is sanitized**: Only `$SHELL` is shared by default, not `$AWS_SECRET_ACCESS_KEY`.
- **Session-scoped**: Everything dies when the session ends. No persistence, no accounts, no tracking.
- **Open source**: Users can audit exactly what's shared.

## 11. Keyboard Shortcuts

| Key | Action | Context |
|-----|--------|---------|
| `Cmd/Ctrl + Shift + D` | Toggle drawing mode | Viewer |
| `Cmd/Ctrl + Shift + L` | Laser pointer (hold) | Viewer |
| `1-8` (with modifier) | Send reaction | Viewer |
| `Cmd/Ctrl + Shift + C` | Toggle remote control | Viewer |
| `Tab` | Toggle terminal panel | Viewer |
| `Cmd/Ctrl + T` | Open new terminal session | Session bar |
| `Escape` | Release control / cancel drawing | Viewer |

## 12. Recommendations — Build Order

1. **Config system** (P4.7) — foundation for everything else
2. **Bidirectional cursors** (P3.5.1) — instant visual improvement
3. **Control takeover** (P3.5.2) — enables real pair programming
4. **Terminal sharing** (P4.1-P4.3) — the differentiator
5. **Tunneling** (P4.4-P4.6) — completes the dev workflow
6. **Drawing + reactions** (P3.5.3-P3.5.4) — polish
7. **Clipboard sync** (P3.5.5) — quality of life
8. **Session bar** — replaces tray menu, unifies controls
9. **Quality adaptation** (P3.5.6) — resilience
10. **Multi-party** (P5.1-P5.3) — scale
