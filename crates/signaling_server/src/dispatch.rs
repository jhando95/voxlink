// ─── Signal Dispatch ───
//
// Contains handle_signal, the top-level match that dispatches SignalMessage
// variants to their handler functions.  Per-variant handlers remain in
// main.rs (crate root) and will be moved in Tasks A10-A17.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use shared_types::SignalMessage;
use crate::types::{State, Db};
use crate::ServerMetrics;
use crate::handlers;

// Mirror the private type alias from main.rs so the function signature compiles.
type Metrics = Arc<ServerMetrics>;

use crate::handlers::channel_settings::ChannelSetting;

pub(crate) async fn handle_signal(
    state: &State,
    metrics: &Metrics,
    peer_id: &str,
    msg: SignalMessage,
    db: &Db,
) {
    match msg {
        SignalMessage::CreateRoom {
            user_name,
            password,
        } => {
            handlers::room::handle_create_room(state, peer_id, user_name, password).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::JoinRoom {
            room_code,
            user_name,
            password,
        } => {
            handlers::room::handle_join_room(state, peer_id, room_code, user_name, password).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::LeaveRoom => {
            crate::handle_disconnect(state, peer_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::MuteChanged { is_muted } => {
            handlers::room::handle_mute_changed(state, peer_id, is_muted).await;
        }
        SignalMessage::DeafenChanged { is_deafened } => {
            handlers::room::handle_deafen_changed(state, peer_id, is_deafened).await;
        }
        SignalMessage::StartScreenShare => {
            handlers::room::handle_start_screen_share(state, peer_id).await;
        }
        SignalMessage::StopScreenShare => {
            handlers::room::handle_stop_screen_share(state, peer_id).await;
        }
        SignalMessage::ScreenShareTransportFeedback {
            frames_completed,
            frames_dropped,
            frames_timed_out,
        } => {
            handlers::room::handle_screen_share_transport_feedback(
                state,
                peer_id,
                frames_completed,
                frames_dropped,
                frames_timed_out,
            )
            .await;
        }
        SignalMessage::CreateSpace { name, user_name } => {
            handlers::space::handle_create_space(state, peer_id, name, user_name, db).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::JoinSpace {
            invite_code,
            user_name,
        } => {
            handlers::space::handle_join_space(state, peer_id, invite_code, user_name, db).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::LeaveSpace => {
            handlers::space::handle_leave_space(state, peer_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::CreateChannel {
            channel_name,
            channel_type,
            voice_quality,
        } => {
            handlers::channel::handle_create_channel(
                state,
                peer_id,
                channel_name,
                channel_type,
                voice_quality,
                db,
            )
            .await;
        }
        SignalMessage::JoinChannel { channel_id } => {
            handlers::channel::handle_join_channel(state, peer_id, channel_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::LeaveChannel => {
            handlers::channel::handle_leave_channel(state, peer_id).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::DeleteChannel { channel_id } => {
            handlers::channel::handle_delete_channel(state, peer_id, channel_id, db).await;
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::DeleteSpace => {
            handlers::space::handle_delete_space(state, peer_id, db).await;
        }
        SignalMessage::RenameSpace { name } => {
            handlers::space::handle_rename_space(state, peer_id, name, db).await;
        }
        SignalMessage::SetSpaceDescription { description } => {
            handlers::space::handle_set_space_description(state, peer_id, description, db).await;
        }
        SignalMessage::SelectTextChannel { channel_id } => {
            handlers::chat::handle_select_text_channel(state, peer_id, channel_id).await;
        }
        SignalMessage::SelectDirectMessage { user_id } => {
            handlers::chat::handle_select_direct_message(state, peer_id, user_id, db).await;
        }
        SignalMessage::SetTyping {
            channel_id,
            is_typing,
        } => {
            handlers::chat::handle_set_typing(state, peer_id, channel_id, is_typing).await;
        }
        SignalMessage::SetDirectTyping { user_id, is_typing } => {
            handlers::chat::handle_set_direct_typing(state, peer_id, user_id, is_typing, db).await;
        }
        SignalMessage::SendTextMessage {
            channel_id,
            content,
            reply_to_message_id,
        } => {
            handlers::chat::handle_send_text_message(
                state,
                peer_id,
                channel_id,
                content,
                reply_to_message_id,
                db,
            )
            .await;
        }
        SignalMessage::SendDirectMessage {
            user_id,
            content,
            reply_to_message_id,
        } => {
            handlers::chat::handle_send_direct_message(
                state,
                peer_id,
                user_id,
                content,
                reply_to_message_id,
                db,
            )
            .await;
        }
        SignalMessage::PinMessage {
            channel_id,
            message_id,
            pinned,
        } => {
            handlers::chat::handle_pin_message(state, peer_id, channel_id, message_id, pinned, db)
                .await;
        }
        SignalMessage::Authenticate { token, user_name } => {
            if handlers::auth::handle_authenticate(state, peer_id, token, user_name, db).await {
                metrics.auth_success_total.fetch_add(1, Ordering::Relaxed);
            } else {
                metrics.auth_failure_total.fetch_add(1, Ordering::Relaxed);
            }
            handlers::presence::notify_watchers_for_peer(state, peer_id).await;
        }
        SignalMessage::WatchFriendPresence { user_ids } => {
            handlers::presence::handle_watch_friend_presence(state, peer_id, user_ids).await;
        }
        SignalMessage::SendFriendRequest { user_id } => {
            handlers::friends::handle_send_friend_request(state, peer_id, user_id, db).await;
        }
        SignalMessage::SendFriendRequestByName { name } => {
            handlers::friends::handle_send_friend_request_by_name(state, peer_id, name, db).await;
        }
        SignalMessage::RespondFriendRequest { user_id, accept } => {
            handlers::friends::handle_respond_friend_request(state, peer_id, user_id, accept, db)
                .await;
        }
        SignalMessage::CancelFriendRequest { user_id } => {
            handlers::friends::handle_cancel_friend_request(state, peer_id, user_id, db).await;
        }
        SignalMessage::RemoveFriend { user_id } => {
            handlers::friends::handle_remove_friend(state, peer_id, user_id, db).await;
        }
        SignalMessage::EditTextMessage {
            channel_id,
            message_id,
            new_content,
        } => {
            handlers::chat::handle_edit_text_message(
                state,
                peer_id,
                channel_id,
                message_id,
                new_content,
                db,
            )
            .await;
        }
        SignalMessage::EditDirectMessage {
            user_id,
            message_id,
            new_content,
        } => {
            handlers::chat::handle_edit_direct_message(
                state,
                peer_id,
                user_id,
                message_id,
                new_content,
                db,
            )
            .await;
        }
        SignalMessage::DeleteTextMessage {
            channel_id,
            message_id,
        } => {
            handlers::chat::handle_delete_text_message(state, peer_id, channel_id, message_id, db)
                .await;
        }
        SignalMessage::DeleteDirectMessage {
            user_id,
            message_id,
        } => {
            handlers::chat::handle_delete_direct_message(state, peer_id, user_id, message_id, db)
                .await;
        }
        SignalMessage::ReactToMessage {
            channel_id,
            message_id,
            emoji,
        } => {
            handlers::chat::handle_react_to_message(state, peer_id, channel_id, message_id, emoji)
                .await;
        }
        SignalMessage::ReactToDirectMessage {
            user_id,
            message_id,
            emoji,
        } => {
            handlers::chat::handle_react_to_direct_message(
                state, peer_id, user_id, message_id, emoji,
            )
            .await;
        }
        SignalMessage::SetUserStatus { status } => {
            handlers::account::handle_set_user_status(state, peer_id, status, db).await;
        }
        SignalMessage::SetChannelTopic { channel_id, topic } => {
            handlers::channel_settings::handle_set_channel_topic(state, peer_id, channel_id, topic, db).await;
        }
        SignalMessage::KickMember { member_id } => {
            handlers::moderation::handle_kick_member(state, peer_id, member_id, db).await;
        }
        SignalMessage::MuteMember { member_id, muted } => {
            handlers::moderation::handle_mute_member(state, peer_id, member_id, muted, db).await;
        }
        SignalMessage::ServerDeafenMember {
            member_id,
            deafened,
        } => {
            handlers::moderation::handle_server_deafen_member(
                state, peer_id, member_id, deafened, db,
            )
            .await;
        }
        SignalMessage::BanMember { member_id } => {
            handlers::moderation::handle_ban_member(state, peer_id, member_id, db).await;
        }
        SignalMessage::SetMemberRole { user_id, role } => {
            handlers::space::handle_set_member_role(state, peer_id, user_id, role, db).await;
        }
        SignalMessage::SearchMessages {
            channel_id,
            query,
            limit,
        } => {
            handlers::chat::handle_search_messages(state, peer_id, channel_id, query, limit, db)
                .await;
        }
        SignalMessage::SearchSpaceMessages { query, limit } => {
            handlers::chat::handle_search_space_messages(state, peer_id, query, limit, db).await;
        }
        SignalMessage::SetProfile { bio } => {
            handlers::account::handle_set_profile(state, peer_id, bio, db).await;
        }
        SignalMessage::RequestUdp => {
            crate::handle_request_udp(state, peer_id).await;
        }
        SignalMessage::SetChannelUserLimit {
            channel_id,
            user_limit,
        } => {
            handlers::channel_settings::handle_channel_setting(
                state,
                peer_id,
                channel_id,
                ChannelSetting::UserLimit(user_limit),
            )
            .await;
        }
        SignalMessage::SetChannelSlowMode {
            channel_id,
            slow_mode_secs,
        } => {
            handlers::channel_settings::handle_channel_setting(
                state,
                peer_id,
                channel_id,
                ChannelSetting::SlowMode(slow_mode_secs),
            )
            .await;
        }
        SignalMessage::SetChannelCategory {
            channel_id,
            category,
        } => {
            handlers::channel_settings::handle_channel_setting(
                state,
                peer_id,
                channel_id,
                ChannelSetting::Category(category),
            )
            .await;
        }
        SignalMessage::SetChannelStatus { channel_id, status } => {
            handlers::channel_settings::handle_channel_setting(state, peer_id, channel_id, ChannelSetting::Status(status))
                .await;
        }
        SignalMessage::SetChannelPermissions {
            channel_id,
            min_role,
        } => {
            let role = match min_role.to_lowercase().as_str() {
                "owner" => shared_types::SpaceRole::Owner,
                "admin" => shared_types::SpaceRole::Admin,
                "moderator" | "mod" => shared_types::SpaceRole::Moderator,
                _ => shared_types::SpaceRole::Member,
            };
            let role_str = min_role.to_lowercase();
            let cid = channel_id.clone();
            handlers::channel_settings::handle_channel_setting(state, peer_id, channel_id, ChannelSetting::MinRole(role)).await;
            // Persist min_role to DB
            if let Some(ref db) = db {
                let db = db.clone();
                tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = db.lock_conn() {
                        let _ = conn.execute(
                            "UPDATE channels SET min_role = ?1 WHERE id = ?2",
                            rusqlite::params![role_str, cid],
                        );
                    }
                });
            }
        }
        SignalMessage::SetChannelAutoDelete {
            channel_id,
            auto_delete_hours,
        } => {
            let cid = channel_id.clone();
            let hours = auto_delete_hours;
            handlers::channel_settings::handle_channel_setting(
                state,
                peer_id,
                channel_id,
                ChannelSetting::AutoDelete(auto_delete_hours),
            )
            .await;
            // Persist to DB
            if let Some(ref db) = db {
                let db = db.clone();
                tokio::task::spawn_blocking(move || {
                    db.set_channel_auto_delete(&cid, hours);
                });
            }
        }
        SignalMessage::ReorderChannels { channel_ids } => {
            handlers::channel::handle_reorder_channels(state, peer_id, channel_ids).await;
        }
        SignalMessage::SetPrioritySpeaker {
            peer_id: target_id,
            enabled,
        } => {
            handlers::channel_settings::handle_set_priority_speaker(state, peer_id, target_id, enabled).await;
        }
        SignalMessage::WhisperTo { target_peer_ids } => {
            handlers::whisper::handle_whisper_to(state, peer_id, target_peer_ids).await;
        }
        SignalMessage::WhisperStopped => {
            handlers::whisper::handle_whisper_stopped(state, peer_id).await;
        }
        SignalMessage::TimeoutMember {
            member_id,
            duration_secs,
        } => {
            handlers::timeouts::handle_timeout_member(state, peer_id, member_id, duration_secs, db).await;
        }
        // v0.8.0: Block/Unblock
        SignalMessage::BlockUser { user_id } => {
            handlers::moderation::handle_block_user(state, peer_id, user_id, db).await;
        }
        SignalMessage::UnblockUser { user_id } => {
            handlers::moderation::handle_unblock_user(state, peer_id, user_id, db).await;
        }
        // v0.8.0: Ban management
        SignalMessage::UnbanMember { user_id } => {
            handlers::moderation::handle_unban_member(state, peer_id, user_id, db).await;
        }
        SignalMessage::ListBans => {
            handlers::moderation::handle_list_bans(state, peer_id, db).await;
        }
        // v0.8.0: Group DMs
        SignalMessage::CreateGroupDM { user_ids, name } => {
            handlers::chat::handle_create_group_dm(state, peer_id, user_ids, name, db).await;
        }
        SignalMessage::SelectGroupDM { group_id } => {
            handlers::chat::handle_select_group_dm(state, peer_id, group_id, db).await;
        }
        SignalMessage::SendGroupMessage {
            group_id,
            content,
            reply_to_message_id,
        } => {
            handlers::chat::handle_send_group_message(
                state,
                peer_id,
                group_id,
                content,
                reply_to_message_id,
                db,
            )
            .await;
        }
        // v0.8.0: Invite settings
        SignalMessage::SetInviteSettings {
            expires_hours,
            max_uses,
        } => {
            handlers::space::handle_set_invite_settings(
                state,
                peer_id,
                expires_hours,
                max_uses,
                db,
            )
            .await;
        }
        // v0.8.0: Message threads
        SignalMessage::GetThread {
            channel_id,
            message_id,
        } => {
            handlers::chat::handle_get_thread(state, peer_id, channel_id, message_id).await;
        }
        // v0.8.0: Nicknames
        SignalMessage::SetNickname { nickname } => {
            handlers::space::handle_set_nickname(state, peer_id, nickname, db).await;
        }
        // v0.8.0: Message forwarding
        SignalMessage::ForwardMessage {
            source_channel_id,
            message_id,
            target_channel_id,
        } => {
            handlers::chat::handle_forward_message(
                state,
                peer_id,
                source_channel_id,
                message_id,
                target_channel_id,
                db,
            )
            .await;
        }
        // v0.8.0: Status presets
        SignalMessage::SetStatusPreset { preset } => {
            handlers::presence::handle_set_status_preset(state, peer_id, preset).await;
        }
        // v0.8.0: Account system
        SignalMessage::CreateAccount {
            email,
            password,
            display_name,
        } => {
            handlers::auth::handle_create_account(
                state,
                peer_id,
                email,
                password,
                display_name,
                db,
            )
            .await;
        }
        SignalMessage::Login { email, password } => {
            handlers::auth::handle_login(state, peer_id, email, password, db).await;
        }
        SignalMessage::Logout => {
            handlers::auth::handle_logout(state, peer_id, db).await;
        }
        SignalMessage::ChangePassword {
            current_password,
            new_password,
        } => {
            handlers::auth::handle_change_password(
                state,
                peer_id,
                current_password,
                new_password,
                db,
            )
            .await;
        }
        SignalMessage::RevokeAllSessions => {
            handlers::auth::handle_revoke_all_sessions(state, peer_id, db).await;
        }
        // v0.10.0: Auto-moderation
        SignalMessage::AddAutomodWord { word, action } => {
            handlers::moderation::handle_add_automod_word(state, peer_id, word, action, db).await;
        }
        SignalMessage::RemoveAutomodWord { word } => {
            handlers::moderation::handle_remove_automod_word(state, peer_id, word, db).await;
        }
        SignalMessage::ListAutomodWords => {
            handlers::moderation::handle_list_automod_words(state, peer_id, db).await;
        }
        // v0.10.0: Role colors
        SignalMessage::SetRoleColor { role, color } => {
            handlers::space::handle_set_role_color(state, peer_id, role.clone(), color.clone(), db)
                .await;
        }
        // v0.10.0: Activity status
        SignalMessage::SetActivity { activity } => {
            handlers::presence::handle_set_activity(state, peer_id, activity.clone()).await;
        }
        // DM Voice Calls
        SignalMessage::CallUser { target_user_id } => {
            handlers::calls::handle_call_user(state, peer_id, target_user_id).await;
        }
        SignalMessage::AcceptCall { room_key } => {
            handlers::calls::handle_accept_call(state, peer_id, room_key).await;
        }
        SignalMessage::DeclineCall { room_key } => {
            handlers::calls::handle_decline_call(state, peer_id, room_key).await;
        }
        // Scheduled Events
        SignalMessage::CreateScheduledEvent {
            title,
            description,
            start_time,
            end_time,
        } => {
            handlers::events::handle_create_event(state, peer_id, title, description, start_time, end_time, db).await;
        }
        SignalMessage::DeleteScheduledEvent { event_id } => {
            handlers::events::handle_delete_event(state, peer_id, event_id, db).await;
        }
        SignalMessage::ToggleEventInterest { event_id } => {
            handlers::events::handle_toggle_event_interest(state, peer_id, event_id, db).await;
        }
        SignalMessage::ListScheduledEvents => {
            handlers::events::handle_list_events(state, peer_id, db).await;
        }
        // Message Scheduling
        SignalMessage::ScheduleMessage {
            channel_id,
            content,
            send_at,
        } => {
            handlers::scheduling::handle_schedule_message(state, peer_id, channel_id, content, send_at, db).await;
        }
        SignalMessage::CancelScheduledMessage { schedule_id } => {
            handlers::scheduling::handle_cancel_scheduled_message(state, peer_id, schedule_id, db).await;
        }
        // Welcome Message
        SignalMessage::SetWelcomeMessage { message } => {
            handlers::scheduling::handle_set_welcome_message(state, peer_id, message, db).await;
        }
        // Voice Recording
        SignalMessage::StartRecording { channel_id } => {
            handlers::recording::handle_start_recording(state, peer_id, channel_id).await;
        }
        SignalMessage::StopRecording { channel_id } => {
            handlers::recording::handle_stop_recording(state, peer_id, channel_id).await;
        }
        // Account management
        SignalMessage::SetDisplayName { name } => {
            handlers::account::handle_set_display_name(state, peer_id, name, db).await;
        }
        SignalMessage::DeleteAccount => {
            handlers::account::handle_delete_account(state, peer_id, db).await;
        }
        // Server discovery
        SignalMessage::SetSpacePublic { is_public } => {
            handlers::channel_settings::handle_set_space_public(state, peer_id, is_public, db).await;
        }
        SignalMessage::BrowsePublicSpaces => {
            handlers::channel_settings::handle_browse_public_spaces(state, peer_id, db).await;
        }
        // Favorites are client-side only (stored in config_store)
        SignalMessage::ToggleFavoriteChannel { .. } => {}
        // Voice notes
        SignalMessage::SendVoiceNote {
            channel_id,
            duration_secs,
            data,
        } => {
            handlers::recording::handle_send_voice_note(state, peer_id, channel_id, duration_secs, data, db).await;
        }
        SignalMessage::AudioQualityReport {
            capture_callback_median_ms,
            playback_callback_median_ms,
            glitches_delta,
            frames_dropped_delta,
            jitter_buffer_ms,
        } => {
            metrics
                .client_audio_capture_callback_seconds
                .observe(capture_callback_median_ms as f64 / 1000.0);
            metrics
                .client_audio_playback_callback_seconds
                .observe(playback_callback_median_ms as f64 / 1000.0);
            metrics
                .client_audio_glitches_total
                .fetch_add(glitches_delta as u64, std::sync::atomic::Ordering::Relaxed);
            metrics
                .client_audio_frames_dropped_total
                .fetch_add(frames_dropped_delta as u64, std::sync::atomic::Ordering::Relaxed);
            metrics
                .client_jitter_buffer_seconds
                .observe(jitter_buffer_ms as f64 / 1000.0);
        }
        other => {
            log::debug!(
                "Unhandled signal from {peer_id}: {:?}",
                std::mem::discriminant(&other)
            );
        }
    }
}
