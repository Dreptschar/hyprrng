/// Wayland layer-shell backend for overlay rendering and input handling.
/// This implements a complete Wayland client with layer-shell surface and keyboard input.
use anyhow::{anyhow, Result};
use memmap2::MmapMut;
use std::os::unix::io::AsFd;
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::sync::{Arc, Mutex};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_compositor::{self, WlCompositor},
        wl_keyboard::{self, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_region::{self, WlRegion},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
        wl_surface::{self, WlSurface},
    },
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};
use xkbcommon::xkb;
use xkbcommon::xkb::keysyms::{KEY_BackSpace, KEY_Escape, KEY_NoSymbol, KEY_Return};

use crate::geometry::CellIndex;

/// Events from the overlay input layer.
#[derive(Clone, Debug)]
pub enum OverlayEvent {
    /// User pressed a grid cell key (u/i/j/k).
    SelectCell(CellIndex),
    /// User pressed backspace to go up a level.
    Ascend,
    /// User pressed escape to cancel.
    Cancel,
    /// User pressed enter to confirm.
    Confirm,
}

/// Shared state for event collection.
#[derive(Default)]
pub struct SharedEvents {
    events: Vec<OverlayEvent>,
}

impl SharedEvents {
    pub fn push(&mut self, event: OverlayEvent) {
        self.events.push(event);
    }

    pub fn drain(&mut self) -> Vec<OverlayEvent> {
        self.events.drain(..).collect()
    }
}

/// Application state for Wayland.
pub struct AppState {
    pub events: Arc<Mutex<SharedEvents>>,
    pub layer_surface: Option<ZwlrLayerSurfaceV1>,
    pub surface: Option<WlSurface>,
    pub keyboard: Option<WlKeyboard>,
    pub shm: Option<WlShm>,
    pub seat: Option<WlSeat>,
    pub configured: bool,
    pub width: u32,
    pub height: u32,
    pub buffers: Vec<ShmBuffer>,
    pub next_buffer_id: u32,
    pub outputs: Vec<OutputInfo>,
    pub next_output_id: u32,
    pub keymap: Keymap,
    pub action_keys: ActionKeys,
    pub cell_keysyms: Vec<(xkb::Keysym, CellIndex)>,
    pub cell_keycodes: Vec<(u32, CellIndex)>,
    pub xkb_context: xkb::Context,
    pub xkb_keymap: Option<xkb::Keymap>,
    pub xkb_state: Option<xkb::State>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(SharedEvents::default())),
            layer_surface: None,
            surface: None,
            keyboard: None,
            shm: None,
            seat: None,
            configured: false,
            width: 0,
            height: 0,
            buffers: Vec::new(),
            next_buffer_id: 1,
            outputs: Vec::new(),
            next_output_id: 1,
            keymap: Keymap::default(),
            action_keys: ActionKeys::default(),
            cell_keysyms: Vec::new(),
            cell_keycodes: Vec::new(),
            xkb_context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_keymap: None,
            xkb_state: None,
        }
    }

    pub fn queue_event(&self, event: OverlayEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    pub fn poll_events(&self) -> Vec<OverlayEvent> {
        if let Ok(mut events) = self.events.lock() {
            events.drain()
        } else {
            Vec::new()
        }
    }

    pub fn is_configured(&self) -> bool {
        self.configured
    }

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.outputs
            .iter()
            .find(|o| o.has_mode)
            .map(|o| (o.width, o.height))
    }

    pub fn get_buffer(
        &mut self,
        width: u32,
        height: u32,
        qh: &QueueHandle<Self>,
    ) -> Result<&mut ShmBuffer> {
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| !b.busy && b.width == width && b.height == height)
        {
            return Ok(&mut self.buffers[idx]);
        }

        let shm = self
            .shm
            .as_ref()
            .ok_or_else(|| anyhow!("wl_shm not available"))?;

        let id = self.next_buffer_id;
        self.next_buffer_id = self.next_buffer_id.wrapping_add(1);

        let buffer = ShmBuffer::create(shm, width, height, id, qh)?;
        self.buffers.push(buffer);
        let last = self.buffers.len() - 1;
        Ok(&mut self.buffers[last])
    }
}

#[derive(Clone, Debug)]
pub struct ActionKeys {
    pub backspace: xkb::Keysym,
    pub esc: xkb::Keysym,
    pub enter: xkb::Keysym,
}

impl Default for ActionKeys {
    fn default() -> Self {
        Self {
            backspace: KEY_BackSpace.into(),
            esc: KEY_Escape.into(),
            enter: KEY_Return.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Keymap {
    pub backspace: u32,
    pub esc: u32,
    pub enter: u32,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            backspace: 14,
            esc: 1,
            enter: 28,
        }
    }
}

pub fn keymap_from_env(base: Keymap) -> Keymap {
    base
}

pub fn action_keys_from_config(cfg: &crate::config::Config) -> ActionKeys {
    let mut ak = ActionKeys::default();
    if let Some(kb) = &cfg.keybindings {
        if let Some(k) = kb.backspace.as_deref().and_then(keysym_from_name) {
            ak.backspace = k;
        }
        if let Some(k) = kb.esc.as_deref().and_then(keysym_from_name) {
            ak.esc = k;
        }
        if let Some(k) = kb.enter.as_deref().and_then(keysym_from_name) {
            ak.enter = k;
        }
    }
    ak
}

pub fn apply_keybindings(base: Keymap, cfg: &crate::config::Config) -> Keymap {
    let mut km = base;
    if let Some(kb) = &cfg.keybindings {
        if let Some(k) = kb.backspace.as_deref().and_then(keycode_for_name) {
            km.backspace = k;
        }
        if let Some(k) = kb.esc.as_deref().and_then(keycode_for_name) {
            km.esc = k;
        }
        if let Some(k) = kb.enter.as_deref().and_then(keycode_for_name) {
            km.enter = k;
        }
    }
    km
}

fn keycode_for_char(c: char) -> Option<u32> {
    match c.to_ascii_lowercase() {
        'a' => Some(30),
        'b' => Some(48),
        'c' => Some(46),
        'd' => Some(32),
        'e' => Some(18),
        'f' => Some(33),
        'g' => Some(34),
        'h' => Some(35),
        'i' => Some(23),
        'j' => Some(36),
        'k' => Some(37),
        'l' => Some(38),
        'm' => Some(50),
        'n' => Some(49),
        'o' => Some(24),
        'p' => Some(25),
        'q' => Some(16),
        'r' => Some(19),
        's' => Some(31),
        't' => Some(20),
        'u' => Some(22),
        'v' => Some(47),
        'w' => Some(17),
        'x' => Some(45),
        'y' => Some(21),
        'z' => Some(44),
        _ => None,
    }
}

fn keycode_for_name(name: &str) -> Option<u32> {
    match name.trim().to_ascii_lowercase().as_str() {
        "u" => Some(30),
        "i" => Some(23),
        "j" => Some(36),
        "k" => Some(37),
        "h" => Some(35),
        "l" => Some(38),
        "y" => Some(21),
        "o" => Some(24),
        "backspace" => Some(14),
        "esc" | "escape" => Some(1),
        "enter" | "return" => Some(28),
        _ => None,
    }
}

fn keysym_from_name(name: &str) -> Option<xkb::Keysym> {
    let n = name.trim();
    match n.to_ascii_lowercase().as_str() {
        "backspace" => Some(KEY_BackSpace.into()),
        "esc" | "escape" => Some(KEY_Escape.into()),
        "enter" | "return" => Some(KEY_Return.into()),
        _ => {
            let s = if n.len() == 1 { n } else { n };
            let sym = xkb::keysym_from_name(s, xkb::KEYSYM_CASE_INSENSITIVE);
            if sym == KEY_NoSymbol.into() {
                None
            } else {
                Some(sym)
            }
        }
    }
}

pub fn build_cell_maps(
    rows: usize,
    cols: usize,
    cfg: &crate::config::Config,
) -> (
    Vec<(xkb::Keysym, CellIndex)>,
    Vec<(u32, CellIndex)>,
    Vec<String>,
) {
    let rows = rows.clamp(1, 10);
    let cols = cols.clamp(1, 10);
    let count = rows * cols;

    let mut labels = if rows == 2 && cols == 2 {
        vec!["U", "I", "J", "K"]
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    if let Some(kb) = &cfg.keybindings {
        if let Some(cells) = &kb.cells {
            if cells.len() == rows && cells.iter().all(|r| r.len() == cols) {
                labels.clear();
                for r in cells {
                    for c in r {
                        labels.push(c.to_ascii_uppercase());
                    }
                }
            }
        }
    }

    if let Ok(keys) = std::env::var("HYPRRGN_KEYS") {
        let chars: Vec<char> = keys.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.len() >= count {
            labels = chars
                .into_iter()
                .take(count)
                .map(|c| c.to_string().to_ascii_uppercase())
                .collect();
        }
    }

    if labels.is_empty() {
        labels = default_cell_labels(count);
        if labels.len() < count {
            tracing::warn!(
                "Not enough default keys for {} cells; {} cells will be unmapped",
                count,
                count - labels.len()
            );
            while labels.len() < count {
                labels.push(String::new());
            }
        }
    }

    let mut keysyms = Vec::new();
    let mut keycodes = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let idx = r * cols + c;
            let label = labels.get(idx).cloned().unwrap_or_default();
            if label.is_empty() {
                continue;
            }
            if let Some(sym) = keysym_from_name(&label) {
                keysyms.push((sym, CellIndex { row: r, col: c }));
            }
            if label.chars().count() == 1 {
                if let Some(code) = keycode_for_char(label.chars().next().unwrap()) {
                    keycodes.push((code, CellIndex { row: r, col: c }));
                }
            }
        }
    }

    (keysyms, keycodes, labels)
}

fn default_cell_labels(count: usize) -> Vec<String> {
    let mut chars: Vec<char> = "asdfghjkl;qwertyuiopzxcvbnm,./1234567890[]'`-="
        .chars()
        .collect();
    chars.push('\\');

    chars
        .into_iter()
        .filter(|c| !c.is_whitespace())
        .take(count)
        .map(|c| c.to_string().to_ascii_uppercase())
        .collect()
}

#[derive(Clone, Debug)]
pub struct OutputInfo {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub has_mode: bool,
}

/// Wayland SHM buffer.
pub struct ShmBuffer {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub mmap: MmapMut,
    pub buffer: WlBuffer,
    pub busy: bool,
}

impl ShmBuffer {
    pub fn create(
        shm: &WlShm,
        width: u32,
        height: u32,
        id: u32,
        qh: &QueueHandle<AppState>,
    ) -> Result<Self> {
        let stride = width * 4;
        let size = stride * height;

        let memfd = memfd::MemfdOptions::default()
            .allow_sealing(true)
            .create("hyprrgn")?;
        let file = memfd.as_file();
        file.set_len(size as u64)?;

        let mmap = unsafe { MmapMut::map_mut(file)? };

        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            qh,
            id,
        );
        pool.destroy();

        Ok(Self {
            id,
            width,
            height,
            stride,
            mmap,
            buffer,
            busy: false,
        })
    }
}

// ============================================================================
// Wayland protocol implementations
// ============================================================================

impl Dispatch<WlRegistry, GlobalListContents> for AppState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == "wl_output" {
                    let id = state.next_output_id;
                    state.next_output_id = state.next_output_id.wrapping_add(1);
                    let output = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, id);
                    let _ = output;
                    state.outputs.push(OutputInfo {
                        id,
                        width: 0,
                        height: 0,
                        has_mode: false,
                    });
                } else if interface == "wl_seat" && state.seat.is_none() {
                    let seat = registry.bind::<WlSeat, _, _>(name, version.min(7), qh, ());
                    state.keyboard = Some(seat.get_keyboard(qh, ()));
                    state.seat = Some(seat);
                }
            }
            wl_registry::Event::GlobalRemove { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for AppState {
    fn event(
        state: &mut Self,
        _output: &WlOutput,
        _event: wl_output::Event,
        output_id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match _event {
            wl_output::Event::Mode {
                width,
                height,
                flags,
                ..
            } => {
                if let wayland_client::WEnum::Value(mode_flags) = flags {
                    if mode_flags.contains(wl_output::Mode::Current) {
                        if let Some(info) = state.outputs.iter_mut().find(|o| o.id == *output_id) {
                            info.width = width as u32;
                            info.height = height as u32;
                            info.has_mode = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCompositor, ()> for AppState {
    fn event(
        _state: &mut Self,
        _compositor: &WlCompositor,
        _event: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Compositor events handled passively
    }
}

impl Dispatch<WlShm, ()> for AppState {
    fn event(
        _state: &mut Self,
        _shm: &WlShm,
        _event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // SHM format events handled passively
    }
}

impl Dispatch<WlShmPool, ()> for AppState {
    fn event(
        _state: &mut Self,
        _pool: &WlShmPool,
        _event: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // SHM pool events handled passively
    }
}

impl Dispatch<WlRegion, ()> for AppState {
    fn event(
        _state: &mut Self,
        _region: &WlRegion,
        _event: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Region events handled passively
    }
}

impl Dispatch<WlSeat, ()> for AppState {
    fn event(
        _state: &mut Self,
        _seat: &WlSeat,
        _event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Seat events handled passively
    }
}

impl Dispatch<WlKeyboard, ()> for AppState {
    fn event(
        state: &mut Self,
        _keyboard: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wl_keyboard::Event;

        match event {
            Event::Keymap { format, fd, size } => {
                if format == wayland_client::WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                    let mut file = unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) };
                    let mut data = vec![0u8; size as usize];
                    if std::io::Read::read_exact(&mut file, &mut data).is_ok() {
                        // Keymap is NUL-terminated text; tolerate non-UTF8 bytes.
                        let text = String::from_utf8_lossy(&data);
                        let text = text.trim_end_matches('\0').to_string();
                        if let Some(km) = xkb::Keymap::new_from_string(
                            &state.xkb_context,
                            text,
                            xkb::KEYMAP_FORMAT_TEXT_V1,
                            xkb::KEYMAP_COMPILE_NO_FLAGS,
                        ) {
                            state.xkb_state = Some(xkb::State::new(&km));
                            state.xkb_keymap = Some(km);
                        }
                    }
                }
            }
            Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(st) = state.xkb_state.as_mut() {
                    st.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            Event::Key {
                key,
                state: key_state,
                ..
            } => {
                // Check if key is pressed
                let is_pressed =
                    matches!(key_state.into_result(), Ok(wl_keyboard::KeyState::Pressed));

                if is_pressed {
                    let mut overlay_event = None;

                    if let Some(st) = &state.xkb_state {
                        let sym = st.key_get_one_sym((key + 8).into());
                        if sym == state.action_keys.backspace {
                            overlay_event = Some(OverlayEvent::Ascend);
                        } else if sym == state.action_keys.esc {
                            overlay_event = Some(OverlayEvent::Cancel);
                        } else if sym == state.action_keys.enter {
                            overlay_event = Some(OverlayEvent::Confirm);
                        } else {
                            for (ks, cell) in &state.cell_keysyms {
                                if *ks == sym {
                                    overlay_event = Some(OverlayEvent::SelectCell(*cell));
                                    break;
                                }
                            }
                        }
                    }

                    if overlay_event.is_none() {
                        overlay_event = match key {
                            k if k == state.keymap.backspace => Some(OverlayEvent::Ascend),
                            k if k == state.keymap.esc => Some(OverlayEvent::Cancel),
                            k if k == state.keymap.enter => Some(OverlayEvent::Confirm),
                            _ => None,
                        };
                        if overlay_event.is_none() {
                            for (kc, cell) in &state.cell_keycodes {
                                if *kc == key {
                                    overlay_event = Some(OverlayEvent::SelectCell(*cell));
                                    break;
                                }
                            }
                        }
                    }

                    let overlay_event = match overlay_event {
                        Some(ev) => ev,
                        None => return,
                    };

                    state.queue_event(overlay_event);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, u32> for AppState {
    fn event(
        state: &mut Self,
        _buffer: &WlBuffer,
        event: wl_buffer::Event,
        buffer_id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            if let Some(buf) = state.buffers.iter_mut().find(|b| b.id == *buffer_id) {
                buf.busy = false;
            }
        }
    }
}

impl Dispatch<WlSurface, ()> for AppState {
    fn event(
        _state: &mut Self,
        _surface: &WlSurface,
        _event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Surface events handled passively
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_layer_surface_v1::Event;

        match event {
            Event::Configure {
                serial,
                width,
                height,
            } => {
                tracing::debug!("Layer surface configured: {}x{}", width, height);

                if width == 0 || height == 0 {
                    if let Some((ow, oh)) = state.output_size() {
                        state.width = ow;
                        state.height = oh;
                    } else {
                        state.width = width;
                        state.height = height;
                    }
                } else {
                    state.width = width;
                    state.height = height;
                }
                state.configured = true;

                // Acknowledge the configure
                _layer_surface.ack_configure(serial);

                // Request a frame callback for rendering
                // Frame callbacks can be handled in the overlay module if needed.
            }
            Event::Closed => {
                tracing::info!("Layer surface closed");
                // Exit the application
                std::process::exit(0);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for AppState {
    fn event(
        _state: &mut Self,
        _layer_shell: &ZwlrLayerShellV1,
        _event: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Layer shell events handled passively
    }
}

impl Dispatch<XdgWmBase, ()> for AppState {
    fn event(
        _state: &mut Self,
        _xdg_wm_base: &XdgWmBase,
        _event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // XDG WM base events handled passively
    }
}

/// Initialize the Wayland connection and create the layer-shell surface.
pub fn init_wayland(
    _width: u32,
    _height: u32,
    keymap: Keymap,
    action_keys: ActionKeys,
    cell_keysyms: Vec<(xkb::Keysym, CellIndex)>,
    cell_keycodes: Vec<(u32, CellIndex)>,
) -> Result<(Connection, EventQueue<AppState>, AppState)> {
    tracing::info!("Initializing Wayland connection");

    // Connect to Wayland
    let conn = Connection::connect_to_env()
        .map_err(|e| anyhow!("Failed to connect to Wayland: {:?}", e))?;

    // Initialize registry and queue
    let (globals, mut event_queue) = registry_queue_init(&conn)
        .map_err(|e| anyhow!("Failed to initialize registry queue: {:?}", e))?;

    let qh = event_queue.handle();

    // Create application state
    let mut state = AppState::new();
    state.keymap = keymap;
    state.action_keys = action_keys;
    state.cell_keysyms = cell_keysyms;
    state.cell_keycodes = cell_keycodes;

    // Bind required globals
    let _compositor = globals
        .bind::<wayland_client::protocol::wl_compositor::WlCompositor, _, _>(&qh, 1..=6, ())
        .map_err(|e| anyhow!("Failed to bind compositor: {:?}", e))?;

    let layer_shell = globals
        .bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())
        .map_err(|e| anyhow!("Failed to bind layer shell: {:?}", e))?;

    let _xdg_wm_base = globals
        .bind::<XdgWmBase, _, _>(&qh, 1..=6, ())
        .map_err(|e| anyhow!("Failed to bind XDG WM base: {:?}", e))?;

    let shm = globals
        .bind::<WlShm, _, _>(&qh, 1..=1, ())
        .map_err(|e| anyhow!("Failed to bind wl_shm: {:?}", e))?;

    if state.seat.is_none() {
        if let Ok(seat) = globals.bind::<WlSeat, _, _>(&qh, 1..=7, ()) {
            state.keyboard = Some(seat.get_keyboard(&qh, ()));
            state.seat = Some(seat);
        }
    }

    // Create surface
    let surface = _compositor.create_surface(&qh, ());
    state.surface = Some(surface.clone());
    state.shm = Some(shm);

    // Create layer surface
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None, // Use the focused output
        zwlr_layer_shell_v1::Layer::Overlay,
        "hyprrgn".to_string(),
        &qh,
        (),
    );

    // Configure layer surface
    layer_surface.set_size(0, 0); // Let the compositor choose output size
    layer_surface.set_anchor(Anchor::all());
    layer_surface.set_exclusive_zone(-1); // Don't reserve space
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer_surface.set_margin(0, 0, 0, 0);

    state.layer_surface = Some(layer_surface);
    // Make the surface fully transparent to the compositor
    let empty_region = _compositor.create_region(&qh, ());
    surface.set_opaque_region(Some(&empty_region));
    empty_region.destroy();

    // Commit the surface to make it visible
    surface.commit();

    // Dispatch initial events to configure the surface
    event_queue.dispatch_pending(&mut state)?;

    Ok((conn, event_queue, state))
}

/// Run the Wayland event loop until an event occurs or timeout.
pub fn run_event_loop(
    event_queue: &mut wayland_client::EventQueue<AppState>,
    state: &mut AppState,
    timeout_ms: Option<u32>,
) -> Result<Vec<OverlayEvent>> {
    let mut events = Vec::new();

    let _ = timeout_ms;

    // Flush outbound requests
    event_queue.flush()?;

    // Dispatch and block until we receive new events
    event_queue.blocking_dispatch(state)?;

    // Collect any events that were queued
    events.extend(state.poll_events());

    Ok(events)
}
