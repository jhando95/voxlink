use std::sync::{Arc, Mutex};

use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

/// Guards config read-modify-write to prevent races between save and remove.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub fn setup_connect(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_connect_server(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let addr = w.get_server_address().to_string().trim().to_string();
        let user_name = w.get_user_name().to_string().trim().to_string();
        if addr.is_empty() {
            w.set_status_text("Enter a server address".into());
            return;
        }
        w.set_reconnect_attempts(0);
        w.set_ping_ms(-1);
        let network = network.clone();
        let window_weak = window_weak.clone();
        w.set_status_text("Connecting...".into());

        rt_handle.spawn(async move {
            {
                let mut cfg = config_store::load_config();
                cfg.user_name = user_name;
                cfg.server_address = addr.clone();
                let _ = config_store::save_config(&cfg);
            }

            let mut net = network.lock().await;
            match net.connect(&addr).await {
                Ok(()) => {
                    log::info!("Connected to server");
                    // Send Authenticate after connecting
                    let cfg = config_store::load_config();
                    let auth_msg = shared_types::SignalMessage::Authenticate {
                        token: cfg.auth_token,
                        user_name: cfg.user_name,
                    };
                    if let Err(e) = net.send_signal(&auth_msg).await {
                        log::warn!("Failed to send auth: {e}");
                    }
                    if let Some(w) = window_weak.upgrade() {
                        w.set_is_connected(true);
                        w.set_status_text("Connected".into());
                    }
                }
                Err(e) => {
                    log::error!("Connection failed: {e}");
                    if let Some(w) = window_weak.upgrade() {
                        let msg = format!("{e}");
                        let friendly = if msg.contains("timed out") {
                            "Failed: Server not reachable"
                        } else if msg.contains("refused") {
                            "Failed: Connection refused"
                        } else if msg.contains("dns") || msg.contains("resolve") {
                            "Failed: Server not found"
                        } else {
                            "Failed: Could not connect"
                        };
                        w.set_status_text(friendly.into());
                        crate::helpers::show_toast(&w, friendly, 3);
                    }
                }
            }
        });
    });
}

pub fn setup_disconnect(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_disconnect_server(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        // Leave room/channel regardless of current view (call persists across views)
        if !w.get_room_code().is_empty() {
            w.invoke_leave_room();
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            network.lock().await.disconnect().await;
        });
        w.set_is_connected(false);
        w.set_status_text("Disconnected".into());
        w.set_ping_ms(-1);
        w.set_reconnect_attempts(0);
        w.set_dropped_frames_baseline(w.get_dropped_frames_total());
        w.set_dropped_frames(0);
    });
}

pub fn setup_save_server(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_save_server(move |name| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let address = w.get_server_address().to_string().trim().to_string();
        if address.is_empty() {
            w.set_status_text("Enter a server address first".into());
            return;
        }
        let display_name = name.to_string().trim().to_string();
        let label = if display_name.is_empty() || display_name == address {
            // Generate a friendly name from the address
            address
                .replace("ws://", "")
                .replace("wss://", "")
                .split(':')
                .next()
                .unwrap_or("Server")
                .to_string()
        } else {
            display_name
        };

        let window_weak = window_weak.clone();
        std::thread::spawn(move || {
            let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut cfg = config_store::load_config();
            // Don't add duplicates
            if cfg.saved_servers.iter().any(|s| s.address == address) {
                return;
            }
            let is_first = cfg.saved_servers.is_empty();
            cfg.saved_servers.push(config_store::SavedServer {
                name: label,
                address,
                is_default: is_first,
            });
            let _ = config_store::save_config(&cfg);

            // Update UI
            if let Some(w) = window_weak.upgrade() {
                let model: Vec<ui_shell::SavedServerData> = cfg
                    .saved_servers
                    .iter()
                    .map(|s| ui_shell::SavedServerData {
                        name: s.name.clone().into(),
                        address: s.address.clone().into(),
                        is_default: s.is_default,
                    })
                    .collect();
                w.set_saved_servers(std::rc::Rc::new(slint::VecModel::from(model)).into());
                w.set_status_text("Server saved".into());
            }
        });
    });
}

pub fn setup_remove_server(window: &MainWindow) {
    let window_weak = window.as_weak();
    window.on_remove_server(move |idx| {
        let idx = idx as usize;
        let window_weak = window_weak.clone();
        std::thread::spawn(move || {
            let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut cfg = config_store::load_config();
            if idx < cfg.saved_servers.len() {
                cfg.saved_servers.remove(idx);
                // If we removed the default, make the first one default
                if !cfg.saved_servers.is_empty()
                    && !cfg.saved_servers.iter().any(|s| s.is_default)
                {
                    cfg.saved_servers[0].is_default = true;
                }
                let _ = config_store::save_config(&cfg);

                if let Some(w) = window_weak.upgrade() {
                    let model: Vec<ui_shell::SavedServerData> = cfg
                        .saved_servers
                        .iter()
                        .map(|s| ui_shell::SavedServerData {
                            name: s.name.clone().into(),
                            address: s.address.clone().into(),
                            is_default: s.is_default,
                        })
                        .collect();
                    w.set_saved_servers(std::rc::Rc::new(slint::VecModel::from(model)).into());
                }
            }
        });
    });
}

pub fn setup_find_server(window: &MainWindow, rt_handle: &tokio::runtime::Handle) {
    let window_weak = window.as_weak();
    let rt_handle = rt_handle.clone();
    window.on_find_server(move || {
        let window_weak = window_weak.clone();
        if let Some(w) = window_weak.upgrade() {
            w.set_status_text("Scanning LAN...".into());
        }
        rt_handle.spawn(async move {
            match net_control::discover_lan_server().await {
                Some(addr) => {
                    log::info!("Found server on LAN: {addr}");
                    if let Some(w) = window_weak.upgrade() {
                        w.set_server_address(addr.into());
                        w.set_status_text("Server found!".into());
                    }
                }
                None => {
                    log::info!("No server found on LAN");
                    if let Some(w) = window_weak.upgrade() {
                        w.set_status_text("No server found on LAN".into());
                    }
                }
            }
        });
    });
}
