use crate::types::State;
use crate::connection::send_error;

pub(crate) async fn handle_whisper_to(state: &State, peer_id: &str, target_peer_ids: Vec<String>) {
    // Cap whisper targets to 20 to prevent abuse
    if target_peer_ids.len() > 20 {
        send_error(state, peer_id, "Too many whisper targets (max 20)").await;
        return;
    }
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id) {
        if let Ok(mut wt) = peer.whisper_targets.write() {
            *wt = target_peer_ids;
        }
    }
}

pub(crate) async fn handle_whisper_stopped(state: &State, peer_id: &str) {
    let s = state.read().await;
    if let Some(peer) = s.peers.get(peer_id) {
        if let Ok(mut wt) = peer.whisper_targets.write() {
            wt.clear();
        }
    }
}
