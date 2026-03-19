use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// Menu item IDs for the tray context menu.
struct MenuIds {
    show: MenuItem,
    mute: MenuItem,
    deafen: MenuItem,
    disconnect: MenuItem,
    quit: MenuItem,
}

/// Holds the tray icon and its menu items. Must stay alive for the tray to persist.
pub struct Tray {
    icon: tray_icon::TrayIcon,
    ids: MenuIds,
    /// Set to true when the user picks "Quit" from the tray menu.
    pub quit_requested: Arc<AtomicBool>,
    /// Cached state to avoid redundant tooltip updates.
    last_tooltip: std::cell::RefCell<String>,
}

impl Tray {
    /// Create the system tray icon with a context menu.
    /// `icon_rgba` / `width` / `height` describe the icon image.
    pub fn new(icon_rgba: Vec<u8>, width: u32, height: u32) -> Option<Self> {
        let icon = Icon::from_rgba(icon_rgba, width, height).ok()?;

        let show = MenuItem::new("Show Voxlink", true, None);
        let mute = MenuItem::new("Toggle Mute", true, None);
        let deafen = MenuItem::new("Toggle Deafen", true, None);
        let disconnect = MenuItem::new("Disconnect", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        let _ = menu.append(&show);
        let _ = menu.append(&mute);
        let _ = menu.append(&deafen);
        let _ = menu.append(&disconnect);
        let _ = menu.append(&tray_icon::menu::PredefinedMenuItem::separator());
        let _ = menu.append(&quit);

        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("Voxlink")
            .build()
            .ok()?;

        #[cfg(target_os = "macos")]
        tray.set_icon_as_template(true);

        let quit_requested = Arc::new(AtomicBool::new(false));

        Some(Self {
            icon: tray,
            ids: MenuIds {
                show,
                mute,
                deafen,
                disconnect,
                quit,
            },
            quit_requested,
            last_tooltip: std::cell::RefCell::new("Voxlink".to_string()),
        })
    }

    /// Poll for tray menu events and invoke the appropriate window callbacks.
    /// Called from the tray timer. Returns true if Quit was selected.
    pub fn poll_events(&self, w: &ui_shell::MainWindow) -> bool {
        use slint::ComponentHandle;

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == self.ids.show.id() {
                w.window().show().ok();
                w.window().request_redraw();
            } else if id == self.ids.mute.id() {
                w.invoke_toggle_mute();
            } else if id == self.ids.deafen.id() {
                w.invoke_toggle_deafen();
            } else if id == self.ids.disconnect.id() {
                // Only disconnect if in a room/channel
                if !w.get_room_code().is_empty() {
                    if w.get_in_space_channel() {
                        w.invoke_leave_channel();
                    } else {
                        w.invoke_leave_room();
                    }
                }
            } else if id == self.ids.quit.id() {
                self.quit_requested.store(true, Ordering::Relaxed);
                return true;
            }
        }

        // Update tooltip and menu item states based on current app state
        self.update_state(w);

        false
    }

    /// Update tray tooltip and menu item enabled state based on current app state.
    fn update_state(&self, w: &ui_shell::MainWindow) {
        let in_call = !w.get_room_code().is_empty();
        let is_muted = w.get_is_muted();
        let is_deafened = w.get_is_deafened();

        // Build tooltip string
        let tooltip = if in_call {
            let room = w.get_room_code();
            let mut status = format!("Voxlink \u{2014} {room}");
            if is_deafened {
                status.push_str(" (Deafened)");
            } else if is_muted {
                status.push_str(" (Muted)");
            }
            status
        } else {
            "Voxlink".to_string()
        };

        // Only update if changed (avoids syscall overhead)
        let mut last = self.last_tooltip.borrow_mut();
        if *last != tooltip {
            let _ = self.icon.set_tooltip(Some(&tooltip));
            *last = tooltip;
        }

        // Enable/disable call-specific menu items
        self.ids.mute.set_enabled(in_call);
        self.ids.deafen.set_enabled(in_call);
        self.ids.disconnect.set_enabled(in_call);

        // Update mute/deafen labels to show current state
        if in_call {
            let mute_text = if is_muted { "Unmute" } else { "Mute" };
            let deafen_text = if is_deafened { "Undeafen" } else { "Deafen" };
            self.ids.mute.set_text(mute_text);
            self.ids.deafen.set_text(deafen_text);
        } else {
            self.ids.mute.set_text("Toggle Mute");
            self.ids.deafen.set_text("Toggle Deafen");
        }
    }
}

/// Load the app icon PNG from the assets directory and return (rgba, width, height).
pub fn load_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let icon_bytes = include_bytes!("../../../assets/icon.png");
    let img = xcap::image::load_from_memory(icon_bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Some((rgba.into_raw(), w, h))
}
