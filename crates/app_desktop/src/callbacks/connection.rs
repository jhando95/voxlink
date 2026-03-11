use std::sync::Arc;

use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

pub fn setup_connect(
    window: &MainWindow,
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    rt_handle: &tokio::runtime::Handle,
) {
    let window_weak = window.as_weak();
    let network = network.clone();
    let rt_handle = rt_handle.clone();
    window.on_connect_server(move || {
        let Some(w) = window_weak.upgrade() else { return; };
        let addr = w.get_server_address().to_string().trim().to_string();
        let user_name = w.get_user_name().to_string().trim().to_string();
        if addr.is_empty() {
            w.set_status_text("Enter a server address".into());
            return;
        }
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
        let Some(w) = window_weak.upgrade() else { return; };
        if w.get_current_view() == 1 {
            w.invoke_leave_room();
        }
        let network = network.clone();
        rt_handle.spawn(async move {
            network.lock().await.disconnect().await;
        });
        w.set_is_connected(false);
        w.set_status_text("Disconnected".into());
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
