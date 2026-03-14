use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shared_types::{AppView, SignalMessage};
use slint::ComponentHandle;
use tokio::sync::Mutex as TokioMutex;
use ui_shell::MainWindow;

/// Drain incoming audio, decode, queue, and update speaking indicators.
pub fn drain_audio_and_update_speaking(
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    state: &Rc<RefCell<shared_types::AppState>>,
    speaking_ticks: &Rc<RefCell<HashMap<String, u64>>>,
    tick: u64,
    w: &MainWindow,
) {
    let mut got_audio = false;
    if let Ok(mut net) = network.try_lock() {
        if let Ok(aud) = audio.try_lock() {
            // Zero-copy audio drain: raw bytes parsed here as &str + &[u8] slices.
            // Eliminates per-frame String + Vec allocations on the hot path.
            while let Some(raw) = net.try_recv_audio() {
                if let Some((sender_id, audio_data)) = net_control::parse_audio_frame(&raw) {
                    let is_speaking = aud.decode_and_queue(sender_id, audio_data);
                    if is_speaking {
                        speaking_ticks
                            .borrow_mut()
                            .insert(sender_id.to_string(), tick);
                    }
                    got_audio = true;
                }
            }
        }
    }

    // Update speaking state with 250ms timeout (10 ticks at 40Hz)
    if got_audio || tick.is_multiple_of(10) {
        let ticks = speaking_ticks.borrow();
        let mut s = state.borrow_mut();
        let mut changed = false;
        for p in s.room.participants.iter_mut() {
            let should_speak = ticks
                .get(&p.id)
                .map(|&t| tick.saturating_sub(t) < 10)
                .unwrap_or(false);
            if p.is_speaking != should_speak {
                p.is_speaking = should_speak;
                changed = true;
            }
        }
        if changed {
            ui_shell::set_participants(w, &s.room.participants);
        }
    }
}

/// Handle connection monitoring, auto-reconnect, and auto-rejoin.
#[allow(clippy::too_many_arguments)]
pub fn check_connection(
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    w: &MainWindow,
    was_connected: &Rc<RefCell<bool>>,
    reconnect_cooldown: &Rc<RefCell<u64>>,
    reconnect_interval: &Rc<RefCell<u64>>,
    network_flag: &Arc<AtomicBool>,
    rt_handle: &tokio::runtime::Handle,
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
) {
    // If the lock is held (e.g. during leave_room async), skip this check
    // to avoid falsely detecting a disconnect.
    let Some(connected) = network.try_lock().ok().map(|n| n.is_connected()) else {
        // Still update perf display even when skipping connection check
        if w.get_current_view() == ui_shell::view_to_index(AppView::Performance) {
            let snap = perf.borrow_mut().snapshot();
            ui_shell::update_perf_display(w, &snap);
        }
        return;
    };
    let prev_connected = *was_connected.borrow();
    *was_connected.borrow_mut() = connected;

    w.set_is_connected(connected);
    network_flag.store(connected, Ordering::Relaxed);

    // Connection just lost — start with short cooldown
    if !connected && prev_connected {
        log::warn!("Connection lost, will attempt reconnect");
        w.set_status_text("Reconnecting...".into());
        w.set_room_status("Connection lost, reconnecting...".into());
        *reconnect_interval.borrow_mut() = 3;
        *reconnect_cooldown.borrow_mut() = 3; // first attempt after 3 ticks (~3s)
    }

    // Connection just restored — re-authenticate and auto-rejoin if in room view
    if connected && !prev_connected {
        w.set_status_text("Connected".into());
        w.set_room_status(slint::SharedString::default());
        let room_code = w.get_room_code().to_string();
        let user_name = w.get_user_name().to_string();
        let is_in_room = w.get_current_view() == 1 && !room_code.is_empty();
        let is_muted = w.get_is_muted();
        let is_deafened = w.get_is_deafened();
        let network = network.clone();
        rt_handle.spawn(async move {
            let net = network.lock().await;

            // Re-authenticate after reconnect
            let cfg = config_store::load_config();
            let _ = net
                .send_signal(&SignalMessage::Authenticate {
                    token: cfg.auth_token,
                    user_name: user_name.clone(),
                })
                .await;

            if is_in_room {
                log::info!("Auto-rejoining room {room_code}");
                let _ = net
                    .send_signal(&SignalMessage::JoinRoom {
                        room_code,
                        user_name,
                        password: None,
                    })
                    .await;
                if is_muted {
                    let _ = net.send_signal(&SignalMessage::MuteChanged { is_muted }).await;
                }
                if is_deafened {
                    let _ = net.send_signal(&SignalMessage::DeafenChanged { is_deafened }).await;
                }
            }
        });
    }

    // Reconnect attempts with exponential backoff
    if !connected && !prev_connected {
        let mut cooldown = reconnect_cooldown.borrow_mut();
        if *cooldown > 0 {
            *cooldown -= 1;
            if *cooldown == 0 {
                w.set_status_text("Reconnecting...".into());

                let network = network.clone();
                let window_weak = w.as_weak();
                rt_handle.spawn(async move {
                    let mut net = network.lock().await;
                    match net.try_reconnect().await {
                        Ok(true) => {
                            log::info!("Reconnected successfully");
                            if let Some(w) = window_weak.upgrade() {
                                w.set_is_connected(true);
                                w.set_status_text("Reconnected".into());
                            }
                        }
                        Ok(false) => {}
                        Err(e) => {
                            log::warn!("Reconnect failed: {e}");
                            if let Some(w) = window_weak.upgrade() {
                                w.set_status_text("Reconnect failed, retrying...".into());
                            }
                        }
                    }
                });
                // Exponential backoff: double the interval, cap at 1200 ticks (~30s)
                let mut interval = reconnect_interval.borrow_mut();
                *interval = (*interval * 2).min(1200);
                *cooldown = *interval;
            }
        }
    }

    if connected {
        *reconnect_cooldown.borrow_mut() = 0;
        *reconnect_interval.borrow_mut() = 3;
    }

    // Update perf display if on that view
    if w.get_current_view() == ui_shell::view_to_index(AppView::Performance) {
        let snap = perf.borrow_mut().snapshot();
        ui_shell::update_perf_display(w, &snap);
    }
}
