# HyprRGN – Hyprland Recursive Grid Navigator

A lightweight, compileable Rust overlay for recursive grid navigation on Wayland/Hyprland.

## What It Does

- Opens a fullscreen layer-shell overlay on the focused output
- Draws a recursive grid (configurable up to 10x10)
- Navigates using the keyboard only
- Prints the final target center coordinates to stdout on confirm

## Build

```bash
cargo build --release
```

## Run

```bash
RUST_LOG=info ./target/release/hyprrgn
```

## Install (optional)

```bash
install -Dm755 ./target/release/hyprrgn ~/.local/bin/hyprrgn
```

Make sure `~/.local/bin` is in your `PATH`.

## Hyprland Bind Example

```hyprlandinit
bind = SUPER, G, exec, /path/to/hyprrgn
```

## Configuration

Config file path:
- Default: `~/.config/hyprrgn/config.toml`
- Override: `HYPRRGN_CONFIG=/path/to/config.toml`

Example config (2x2 default):

```toml
grid_size = { rows = 2, cols = 2 }

grid_color = { r = 1.0, g = 1.0, b = 1.0 }

[keybindings]
# row-major: top-left, top-right, bottom-left, bottom-right
cells = [
  ["u", "i"],
  ["j", "k"],
]

backspace = "backspace"
esc = "escape"
enter = "return"
```

Example config (3x3):

```toml
grid_size = { rows = 3, cols = 3 }

[keybindings]
cells = [
  ["q", "w", "e"],
  ["a", "s", "d"],
  ["z", "x", "c"],
]
```

Environment overrides:
- `HYPRRGN_GRID=3x3`
- `HYPRRGN_KEYS=asdfqwer...` (row-major, length = rows*cols)

## Notes

- Layout-aware key handling via xkbcommon
- Grid size capped to 10x10
- If `keybindings.cells` is omitted, a vim-friendly default key layout is generated automatically
- Pointer movement/clicking not implemented
