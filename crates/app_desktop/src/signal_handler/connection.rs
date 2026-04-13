use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rand::Rng;

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
        let mut ticks = speaking_ticks.borrow_mut();
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
        // Prune stale entries (>5s old) to prevent unbounded growth
        if tick.is_multiple_of(200) {
            ticks.retain(|_, &mut t| tick.saturating_sub(t) < 200);
        }
    }
}

pub fn drain_screen_share_frame(
    network: &Arc<TokioMutex<net_control::NetworkClient>>,
    w: &MainWindow,
) {
    let mut latest = None;
    if let Ok(mut net) = network.try_lock() {
        while let Some(raw) = net.try_recv_screen_frame() {
            latest = Some(raw);
        }
    }

    let Some(raw) = latest else {
        return;
    };
    let Some((sender_id, frame_data)) = net_control::parse_screen_frame(&raw) else {
        return;
    };
    if sender_id != w.get_screen_share_owner_id().as_str() {
        return;
    }

    match xcap::image::load_from_memory(frame_data) {
        Ok(image) => {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                rgba.as_raw(),
                width,
                height,
            );
            w.set_screen_share_image(slint::Image::from_rgba8(buffer));
        }
        Err(e) => {
            log::warn!("Failed to decode screen share frame: {e}");
            w.set_screen_share_owner_name("Decode error - stream may be corrupted".into());
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
    screen_share: &Arc<crate::screen_share::ScreenShareController>,
    rt_handle: &tokio::runtime::Handle,
    perf: &Rc<RefCell<perf_metrics::PerfCollector>>,
    audio_started: &Rc<RefCell<bool>>,
    audio: &Arc<TokioMutex<audio_core::AudioEngine>>,
    audio_active_flag: &Arc<AtomicBool>,
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

    // Connection just lost — stop audio, reset state, start reconnect cooldown
    if !connected && prev_connected {
        log::warn!("Connection lost, will attempt reconnect");
        screen_share.stop_capture();

        // Stop audio streams and reset audio_started so reconnect can restart them.
        // Without this, start_audio_if_needed bails on reconnect because audio_started
        // is still true, and the media session callback never gets re-wired.
        *audio_started.borrow_mut() = false;
        audio_active_flag.store(false, Ordering::Relaxed);
        let audio = audio.clone();
        rt_handle.spawn(async move {
            // Timeout prevents deadlock if audio lock is stuck (e.g. device driver hang)
            match tokio::time::timeout(std::time::Duration::from_secs(2), audio.lock()).await {
                Ok(mut aud) => {
                    aud.stop_capture();
                    aud.stop_playback();
                    log::info!("Audio stopped after disconnect — will restart on reconnect");
                }
                Err(_) => {
                    log::error!("Timed out acquiring audio lock during disconnect cleanup");
                }
            }
        });

        w.set_status_text("Reconnecting...".into());
        w.set_room_status("Connection lost, reconnecting...".into());
        crate::helpers::show_toast(w, "Connection lost \u{2014} reconnecting...", 2);
        w.set_has_screen_share(false);
        w.set_is_sharing_screen(false);
        w.set_screen_share_owner_name(slint::SharedString::default());
        w.set_screen_share_owner_id(slint::SharedString::default());
        w.set_screen_share_image(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
            slint::Rgba8Pixel,
        >::new(1, 1)));
        *reconnect_interval.borrow_mut() = 3;
        *reconnect_cooldown.borrow_mut() = 3; // first attempt after 3 ticks (~3s)
    }

    // Connection just restored — re-authenticate and auto-rejoin if in room view
    if connected && !prev_connected {
        w.set_status_text("Connected".into());
        w.set_room_status(slint::SharedString::default());
        crate::helpers::show_toast(w, "Reconnected", 1);

        // Snapshot all UI state atomically before spawning async task.
        // This prevents the race where user navigates away between snapshot
        // and the async rejoin, which would rejoin a stale room/space.
        let current_view = w.get_current_view();
        let room_code = w.get_room_code().to_string();
        let user_name = w.get_user_name().to_string();
        let space_invite = w.get_current_space_invite().to_string();
        let direct_message_user_id = if current_view == ui_shell::view_to_index(AppView::TextChat)
            && w.get_chat_is_direct_message()
        {
            Some(w.get_chat_channel_id().to_string())
        } else {
            None
        };
        let active_channel_id = w.get_active_channel_id().to_string();
        let in_space_channel = w.get_in_space_channel();
        let chat_channel_id = w.get_chat_channel_id().to_string();
        let is_in_room = !room_code.is_empty();
        let is_in_space = !space_invite.is_empty();
        let is_muted = w.get_is_muted();
        let is_deafened = w.get_is_deafened();
        let snapshot_view = current_view;
        let network = network.clone();
        let window_weak = w.as_weak();
        rt_handle.spawn(async move {
            // Verify UI hasn't navigated away since snapshot (user may have left room)
            let view_still_valid = window_weak
                .upgrade()
                .map(|w| w.get_current_view() == snapshot_view)
                .unwrap_or(false);
            if !view_still_valid && (is_in_room || is_in_space) {
                log::info!("Skipping auto-rejoin: user navigated away during reconnect");
                // Still re-authenticate even if we skip rejoin
                let net = network.lock().await;
                let _ = net
                    .send_signal(&SignalMessage::Authenticate {
                        token: config_store::load_auth_token(),
                        user_name,
                    })
                    .await;
                return;
            }
            let net = network.lock().await;

            // Re-authenticate after reconnect
            let _ = net
                .send_signal(&SignalMessage::Authenticate {
                    token: config_store::load_auth_token(),
                    user_name: user_name.clone(),
                })
                .await;

            // Rejoin space if we were in one
            if is_in_space {
                log::info!("Auto-rejoining space");
                let _ = net
                    .send_signal(&SignalMessage::JoinSpace {
                        invite_code: space_invite,
                        user_name: user_name.clone(),
                    })
                    .await;
                // Brief delay for server to process JoinSpace before channel operations
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;

                // Auto-rejoin voice channel within the space
                if in_space_channel && !active_channel_id.is_empty() {
                    log::info!("Auto-rejoining voice channel {active_channel_id}");
                    let _ = net
                        .send_signal(&SignalMessage::JoinChannel {
                            channel_id: active_channel_id,
                        })
                        .await;
                }

                // Re-select text channel if we had one open
                if !chat_channel_id.is_empty() && direct_message_user_id.is_none() {
                    let _ = net
                        .send_signal(&SignalMessage::SelectTextChannel {
                            channel_id: chat_channel_id,
                        })
                        .await;
                }
            }

            // Rejoin standalone room (not a space channel — those are handled above)
            if is_in_room && !in_space_channel {
                log::info!("Auto-rejoining room {room_code}");
                let _ = net
                    .send_signal(&SignalMessage::JoinRoom {
                        room_code,
                        user_name,
                        password: None,
                    })
                    .await;
            }

            // Restore mute/deafen state after any voice rejoin
            if is_in_room || in_space_channel {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if is_muted {
                    let _ = net
                        .send_signal(&SignalMessage::MuteChanged { is_muted })
                        .await;
                }
                if is_deafened {
                    let _ = net
                        .send_signal(&SignalMessage::DeafenChanged { is_deafened })
                        .await;
                }
            }

            if let Some(user_id) = direct_message_user_id.filter(|user_id| !user_id.is_empty()) {
                let _ = net
                    .send_signal(&SignalMessage::SelectDirectMessage { user_id })
                    .await;
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
                w.set_reconnect_attempts(w.get_reconnect_attempts() + 1);

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
                // Exponential backoff with jitter: cap at 1200 ticks (~30s)
                let mut interval = reconnect_interval.borrow_mut();
                *interval = (*interval * 2).min(1200);
                // Add random jitter (±25% of interval) to prevent thundering herd
                let jitter_range = (*interval / 4).max(1);
                let jitter =
                    rand::thread_rng().gen_range(0..jitter_range * 2) as i64 - jitter_range as i64;
                *cooldown = ((*interval as i64 + jitter).max(1)) as u64;
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
