use net_control::{parse_audio_frame, parse_screen_frame, NetworkClient};
use shared_types::{ChannelInfo, ChannelType, SignalMessage};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use tokio::time::{sleep, Duration};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_HOLD_MS: u64 = 2200;
const DEFAULT_INVITE_TIMEOUT_MS: u64 = 8000;
const DEFAULT_MESSAGE_COUNT: usize = 24;
const DEFAULT_SCREEN_FRAME_COUNT: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutomationScenario {
    SpaceChannelSoak,
    SpaceTextChatSoak,
    SpaceScreenShareSoak,
}

#[derive(Clone, Copy, Debug)]
enum AutomationRole {
    Owner,
    Participant,
}

#[derive(Clone, Debug)]
struct AutomationSpec {
    scenario: AutomationScenario,
    role: AutomationRole,
    server_url: String,
    user_name: String,
    space_name: String,
    shared_path: PathBuf,
    report_path: PathBuf,
    hold_ms: u64,
    invite_timeout_ms: u64,
    expect_peers: usize,
    expect_audio: bool,
    send_audio: bool,
    message_count: usize,
    screen_frame_count: usize,
}

#[derive(Default)]
struct AutomationMetrics {
    authenticated: bool,
    space_ready: bool,
    channel_joined: bool,
    text_channel_selected: bool,
    screen_share_started: bool,
    self_channel_updates: u32,
    peer_join_events: u32,
    member_channel_updates: u32,
    member_online_events: u32,
    audio_frames_sent: u32,
    audio_frames_recv: u32,
    text_messages_sent: u32,
    text_messages_recv: u32,
    screen_frames_sent: u32,
    screen_frames_recv: u32,
    last_status: String,
    failures: Vec<String>,
}

struct SharedJoinInfo {
    invite_code: String,
    voice_channel_id: String,
    voice_channel_name: String,
    text_channel_id: String,
    text_channel_name: String,
}

pub fn maybe_run_from_env() -> Option<i32> {
    let spec = match parse_spec_from_env() {
        Ok(Some(spec)) => spec,
        Ok(None) => {
            return None;
        }
        Err(err) => {
            eprintln!("{err}");
            return Some(1);
        }
    };

    if spec.server_url.is_empty()
        || spec.user_name.is_empty()
        || spec.shared_path.as_os_str().is_empty()
        || spec.report_path.as_os_str().is_empty()
    {
        eprintln!("Automation spec is incomplete");
        return Some(1);
    }

    let started = Instant::now();
    let mut metrics = AutomationMetrics::default();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let message = format!("Failed to build automation runtime: {err}");
            let _ = write_report(
                &spec,
                false,
                started.elapsed(),
                std::slice::from_ref(&message),
                &metrics,
            );
            eprintln!("{message}");
            return Some(1);
        }
    };

    let result = runtime.block_on(run_automation(&spec, &mut metrics));
    let mut failures = metrics.failures.clone();
    if let Err(err) = result {
        failures.push(err);
    }
    let ok = failures.is_empty();
    if let Err(err) = write_report(&spec, ok, started.elapsed(), &failures, &metrics) {
        eprintln!("Failed to write automation report: {err}");
    }

    if ok {
        Some(0)
    } else {
        for failure in &failures {
            eprintln!("{failure}");
        }
        Some(1)
    }
}

fn parse_spec_from_env() -> Result<Option<AutomationSpec>, String> {
    let Ok(scenario) = env::var("VOXLINK_AUTOMATION_SCENARIO") else {
        return Ok(None);
    };
    let scenario = match scenario.as_str() {
        "space_channel_soak" => AutomationScenario::SpaceChannelSoak,
        "space_text_chat_soak" => AutomationScenario::SpaceTextChatSoak,
        "space_screen_share_soak" => AutomationScenario::SpaceScreenShareSoak,
        other => return Err(format!("Unsupported automation scenario: {other}")),
    };

    let role = match env::var("VOXLINK_AUTOMATION_ROLE")
        .unwrap_or_else(|_| "participant".into())
        .as_str()
    {
        "owner" => AutomationRole::Owner,
        "participant" => AutomationRole::Participant,
        other => return Err(format!("Unsupported automation role: {other}")),
    };

    let server_url = env::var("VOXLINK_AUTOMATION_SERVER_URL").unwrap_or_default();
    let user_name = env::var("VOXLINK_AUTOMATION_USER_NAME").unwrap_or_default();
    let shared_path = PathBuf::from(env::var("VOXLINK_AUTOMATION_SHARED_PATH").unwrap_or_default());
    let report_path = PathBuf::from(env::var("VOXLINK_AUTOMATION_REPORT_PATH").unwrap_or_default());
    Ok(Some(AutomationSpec {
        scenario,
        role,
        server_url,
        user_name,
        space_name: env::var("VOXLINK_AUTOMATION_SPACE_NAME")
            .unwrap_or_else(|_| "Automation Space".into()),
        shared_path,
        report_path,
        hold_ms: env_u64("VOXLINK_AUTOMATION_HOLD_MS").unwrap_or(DEFAULT_HOLD_MS),
        invite_timeout_ms: env_u64("VOXLINK_AUTOMATION_INVITE_TIMEOUT_MS")
            .unwrap_or(DEFAULT_INVITE_TIMEOUT_MS),
        expect_peers: env_u64("VOXLINK_AUTOMATION_EXPECT_PEERS").unwrap_or(0) as usize,
        expect_audio: env_bool("VOXLINK_AUTOMATION_EXPECT_AUDIO"),
        send_audio: env_bool("VOXLINK_AUTOMATION_SEND_AUDIO"),
        message_count: env_u64("VOXLINK_AUTOMATION_MESSAGE_COUNT")
            .unwrap_or(DEFAULT_MESSAGE_COUNT as u64) as usize,
        screen_frame_count: env_u64("VOXLINK_AUTOMATION_SCREEN_FRAME_COUNT")
            .unwrap_or(DEFAULT_SCREEN_FRAME_COUNT as u64) as usize,
    }))
}

fn env_u64(key: &str) -> Option<u64> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
}

fn env_bool(key: &str) -> bool {
    matches!(
        env::var(key).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

async fn run_automation(
    spec: &AutomationSpec,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    if spec.server_url.is_empty()
        || spec.user_name.is_empty()
        || spec.shared_path.as_os_str().is_empty()
        || spec.report_path.as_os_str().is_empty()
    {
        return Err("Automation spec is incomplete".into());
    }

    let mut network = NetworkClient::new();
    network
        .connect(&spec.server_url)
        .await
        .map_err(|err| format!("Failed to connect automation client: {err}"))?;
    metrics.last_status = "connected".into();

    network
        .send_signal(&SignalMessage::Authenticate {
            token: None,
            user_name: spec.user_name.clone(),
        })
        .await
        .map_err(|err| format!("Failed to send authentication: {err}"))?;

    wait_for_authenticated(&mut network, metrics).await?;

    match (spec.scenario, spec.role) {
        (AutomationScenario::SpaceChannelSoak, AutomationRole::Owner) => {
            run_owner_audio(spec, &mut network, metrics).await?
        }
        (AutomationScenario::SpaceChannelSoak, AutomationRole::Participant) => {
            run_participant_audio(spec, &mut network, metrics).await?
        }
        (AutomationScenario::SpaceTextChatSoak, AutomationRole::Owner) => {
            run_owner_text(spec, &mut network, metrics).await?
        }
        (AutomationScenario::SpaceTextChatSoak, AutomationRole::Participant) => {
            run_participant_text(spec, &mut network, metrics).await?
        }
        (AutomationScenario::SpaceScreenShareSoak, AutomationRole::Owner) => {
            run_owner_screen(spec, &mut network, metrics).await?
        }
        (AutomationScenario::SpaceScreenShareSoak, AutomationRole::Participant) => {
            run_participant_screen(spec, &mut network, metrics).await?
        }
    }

    network.disconnect().await;
    metrics.last_status = "complete".into();
    Ok(())
}

async fn wait_for_authenticated(
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match recv_signal_until(network, deadline).await? {
            SignalMessage::Authenticated { .. } => {
                metrics.authenticated = true;
                metrics.last_status = "authenticated".into();
                return Ok(());
            }
            signal => handle_nonblocking_signal(signal, metrics)?,
        }
    }
}

async fn run_owner_audio(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let shared = create_space_and_share(spec, network, metrics).await?;

    join_channel(network, &shared.voice_channel_id).await?;
    wait_for_channel_join(network, metrics, &shared.voice_channel_id).await?;

    let observe_deadline = Instant::now() + Duration::from_millis(spec.hold_ms);
    let mut audio_sent = false;
    while Instant::now() < observe_deadline {
        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }

        if spec.send_audio && !audio_sent && metrics.peer_join_events as usize >= spec.expect_peers
        {
            let frame = generate_test_audio();
            for _ in 0..4 {
                network
                    .send_audio(&frame)
                    .await
                    .map_err(|err| format!("Failed to send automation audio: {err}"))?;
                metrics.audio_frames_sent += 1;
                sleep(Duration::from_millis(80)).await;
            }
            audio_sent = true;
            metrics.last_status = "audio-sent".into();
            continue;
        }

        if !network.is_connected() {
            return Err("Automation owner disconnected unexpectedly".into());
        }

        sleep(POLL_INTERVAL).await;
    }

    if spec.expect_peers > 0 && (metrics.peer_join_events as usize) < spec.expect_peers {
        return Err(format!(
            "Owner expected {} peer joins in the channel but saw {}",
            spec.expect_peers, metrics.peer_join_events
        ));
    }
    if spec.send_audio && metrics.audio_frames_sent == 0 {
        return Err("Owner never sent automation audio".into());
    }

    Ok(())
}

async fn run_participant_audio(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let shared = wait_for_shared_info(&spec.shared_path, spec.invite_timeout_ms).await?;
    join_space(spec, network, metrics, &shared).await?;

    join_channel(network, &shared.voice_channel_id).await?;
    wait_for_channel_join(network, metrics, &shared.voice_channel_id).await?;

    let observe_deadline = Instant::now() + Duration::from_millis(spec.hold_ms);
    while Instant::now() < observe_deadline {
        if let Some(frame) = network.try_recv_audio() {
            if parse_audio_frame(&frame).is_some() {
                metrics.audio_frames_recv += 1;
                metrics.last_status = "audio-received".into();
            }
        }

        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }

        if !network.is_connected() {
            return Err(format!(
                "Automation participant {} disconnected unexpectedly",
                spec.user_name
            ));
        }

        sleep(POLL_INTERVAL).await;
    }

    if spec.expect_audio && metrics.audio_frames_recv == 0 {
        return Err(format!(
            "Automation participant {} did not receive any audio frames",
            spec.user_name
        ));
    }

    Ok(())
}

async fn run_owner_text(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let shared = create_space_and_share(spec, network, metrics).await?;
    wait_for_member_online_count(network, metrics, spec.expect_peers, spec.invite_timeout_ms).await?;

    select_text_channel(network, &shared.text_channel_id).await?;
    wait_for_text_channel_selected(network, metrics, &shared.text_channel_id).await?;

    for idx in 0..spec.message_count {
        let content = format!("automation-message-{}-{idx}", spec.user_name);
        network
            .send_signal(&SignalMessage::SendTextMessage {
                channel_id: shared.text_channel_id.clone(),
                content,
                reply_to_message_id: None,
            })
            .await
            .map_err(|err| format!("Failed to send automation text message: {err}"))?;
        metrics.text_messages_sent += 1;
        metrics.last_status = "text-message-sent".into();
        sleep(Duration::from_millis(30)).await;
    }

    let observe_deadline = Instant::now() + Duration::from_millis(spec.hold_ms);
    while Instant::now() < observe_deadline {
        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }
        if !network.is_connected() {
            return Err("Automation text owner disconnected unexpectedly".into());
        }
        sleep(POLL_INTERVAL).await;
    }

    if spec.expect_peers > 0 && (metrics.member_online_events as usize) < spec.expect_peers {
        return Err(format!(
            "Owner expected {} members online before text burst but saw {}",
            spec.expect_peers, metrics.member_online_events
        ));
    }

    Ok(())
}

async fn run_participant_text(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let shared = wait_for_shared_info(&spec.shared_path, spec.invite_timeout_ms).await?;
    join_space(spec, network, metrics, &shared).await?;

    select_text_channel(network, &shared.text_channel_id).await?;
    wait_for_text_channel_selected(network, metrics, &shared.text_channel_id).await?;

    let observe_deadline = Instant::now()
        + Duration::from_millis(spec.hold_ms.max((spec.message_count as u64 * 40) + 1200));
    while Instant::now() < observe_deadline
        && (metrics.text_messages_recv as usize) < spec.message_count
    {
        if let Some(signal) = network.try_recv_signal() {
            match signal {
                SignalMessage::TextMessage { channel_id, .. } if channel_id == shared.text_channel_id => {
                    metrics.text_messages_recv += 1;
                    metrics.last_status = "text-message-received".into();
                }
                other => handle_nonblocking_signal(other, metrics)?,
            }
        }

        if !network.is_connected() {
            return Err(format!(
                "Automation text participant {} disconnected unexpectedly",
                spec.user_name
            ));
        }

        sleep(POLL_INTERVAL).await;
    }

    if metrics.text_messages_recv as usize != spec.message_count {
        return Err(format!(
            "Automation participant {} expected {} text messages but received {}",
            spec.user_name, spec.message_count, metrics.text_messages_recv
        ));
    }

    Ok(())
}

async fn run_owner_screen(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let shared = create_space_and_share(spec, network, metrics).await?;

    join_channel(network, &shared.voice_channel_id).await?;
    wait_for_channel_join(network, metrics, &shared.voice_channel_id).await?;

    let peer_deadline = Instant::now() + Duration::from_millis(spec.invite_timeout_ms);
    while Instant::now() < peer_deadline && (metrics.peer_join_events as usize) < spec.expect_peers {
        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }
        sleep(POLL_INTERVAL).await;
    }

    network
        .send_signal(&SignalMessage::StartScreenShare)
        .await
        .map_err(|err| format!("Failed to start automation screen share: {err}"))?;

    let start_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < start_deadline && !metrics.screen_share_started {
        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }
        sleep(POLL_INTERVAL).await;
    }

    for idx in 0..spec.screen_frame_count {
        let frame = generate_test_screen_frame(idx);
        network
            .send_screen_frame(&frame)
            .await
            .map_err(|err| format!("Failed to send automation screen frame: {err}"))?;
        metrics.screen_frames_sent += 1;
        metrics.last_status = "screen-frame-sent".into();
        sleep(Duration::from_millis(60)).await;
    }

    network
        .send_signal(&SignalMessage::StopScreenShare)
        .await
        .map_err(|err| format!("Failed to stop automation screen share: {err}"))?;

    if spec.expect_peers > 0 && (metrics.peer_join_events as usize) < spec.expect_peers {
        return Err(format!(
            "Owner expected {} peer joins before screen share but saw {}",
            spec.expect_peers, metrics.peer_join_events
        ));
    }
    if metrics.screen_frames_sent == 0 {
        return Err("Automation owner never sent any screen frames".into());
    }

    Ok(())
}

async fn run_participant_screen(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    let shared = wait_for_shared_info(&spec.shared_path, spec.invite_timeout_ms).await?;
    join_space(spec, network, metrics, &shared).await?;

    join_channel(network, &shared.voice_channel_id).await?;
    wait_for_channel_join(network, metrics, &shared.voice_channel_id).await?;

    let observe_deadline = Instant::now()
        + Duration::from_millis(spec.hold_ms.max((spec.screen_frame_count as u64 * 70) + 1200));
    while Instant::now() < observe_deadline
        && (metrics.screen_frames_recv as usize) < spec.screen_frame_count
    {
        if let Some(frame) = network.try_recv_screen_frame() {
            if parse_screen_frame(&frame).is_some() {
                metrics.screen_frames_recv += 1;
                metrics.last_status = "screen-frame-received".into();
            }
        }
        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }

        if !network.is_connected() {
            return Err(format!(
                "Automation screen participant {} disconnected unexpectedly",
                spec.user_name
            ));
        }

        sleep(POLL_INTERVAL).await;
    }

    if metrics.screen_frames_recv as usize != spec.screen_frame_count {
        return Err(format!(
            "Automation participant {} expected {} screen frames but received {}",
            spec.user_name, spec.screen_frame_count, metrics.screen_frames_recv
        ));
    }

    Ok(())
}

async fn create_space_and_share(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
) -> Result<SharedJoinInfo, String> {
    network
        .send_signal(&SignalMessage::CreateSpace {
            name: spec.space_name.clone(),
            user_name: spec.user_name.clone(),
        })
        .await
        .map_err(|err| format!("Failed to create automation space: {err}"))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let shared = loop {
        match recv_signal_until(network, deadline).await? {
            SignalMessage::SpaceCreated { space, channels } => {
                let voice_channel = select_voice_channel(&channels)?;
                let text_channel = match select_text_channel_info(&channels) {
                    Ok(channel) => channel.clone(),
                    Err(_) => create_text_channel(network, metrics, "chat").await?,
                };
                let shared = SharedJoinInfo {
                    invite_code: space.invite_code,
                    voice_channel_id: voice_channel.id.clone(),
                    voice_channel_name: voice_channel.name.clone(),
                    text_channel_id: text_channel.id.clone(),
                    text_channel_name: text_channel.name.clone(),
                };
                write_shared_info(&spec.shared_path, &shared)?;
                metrics.space_ready = true;
                metrics.last_status = "space-created".into();
                break shared;
            }
            signal => handle_nonblocking_signal(signal, metrics)?,
        }
    };
    Ok(shared)
}

async fn join_space(
    spec: &AutomationSpec,
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
    shared: &SharedJoinInfo,
) -> Result<(), String> {
    network
        .send_signal(&SignalMessage::JoinSpace {
            invite_code: shared.invite_code.clone(),
            user_name: spec.user_name.clone(),
        })
        .await
        .map_err(|err| format!("Failed to join automation space: {err}"))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match recv_signal_until(network, deadline).await? {
            SignalMessage::SpaceJoined { .. } => {
                metrics.space_ready = true;
                metrics.last_status = "space-joined".into();
                return Ok(());
            }
            signal => handle_nonblocking_signal(signal, metrics)?,
        }
    }
}

async fn join_channel(network: &mut NetworkClient, channel_id: &str) -> Result<(), String> {
    network
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: channel_id.to_string(),
        })
        .await
        .map_err(|err| format!("Failed to join automation channel: {err}"))
}

async fn create_text_channel(
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
    channel_name: &str,
) -> Result<ChannelInfo, String> {
    network
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: channel_name.to_string(),
            channel_type: ChannelType::Text,
        })
        .await
        .map_err(|err| format!("Failed to create automation text channel: {err}"))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match recv_signal_until(network, deadline).await? {
            SignalMessage::ChannelCreated { channel } if channel.channel_type == ChannelType::Text => {
                metrics.last_status = "text-channel-created".into();
                return Ok(channel);
            }
            signal => handle_nonblocking_signal(signal, metrics)?,
        }
    }
}

async fn select_text_channel(network: &mut NetworkClient, channel_id: &str) -> Result<(), String> {
    network
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: channel_id.to_string(),
        })
        .await
        .map_err(|err| format!("Failed to select automation text channel: {err}"))
}

async fn wait_for_channel_join(
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
    channel_id: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_channel_joined = false;
    let mut saw_channel_update = false;
    while !saw_channel_joined || !saw_channel_update {
        match recv_signal_until(network, deadline).await? {
            SignalMessage::ChannelJoined {
                channel_id: joined_channel,
                ..
            } => {
                if joined_channel == channel_id {
                    saw_channel_joined = true;
                    metrics.channel_joined = true;
                    metrics.last_status = "channel-joined".into();
                }
            }
            SignalMessage::MemberChannelChanged {
                channel_id: changed_channel,
                ..
            } => {
                if changed_channel.as_deref() == Some(channel_id) {
                    saw_channel_update = true;
                    metrics.self_channel_updates += 1;
                    metrics.member_channel_updates += 1;
                }
            }
            SignalMessage::PeerJoined { .. } => {
                metrics.peer_join_events += 1;
            }
            other => handle_nonblocking_signal(other, metrics)?,
        }
    }
    Ok(())
}

async fn wait_for_text_channel_selected(
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
    channel_id: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match recv_signal_until(network, deadline).await? {
            SignalMessage::TextChannelSelected {
                channel_id: selected_channel,
                ..
            } if selected_channel == channel_id => {
                metrics.text_channel_selected = true;
                metrics.last_status = "text-channel-selected".into();
                return Ok(());
            }
            signal => handle_nonblocking_signal(signal, metrics)?,
        }
    }
}

async fn wait_for_member_online_count(
    network: &mut NetworkClient,
    metrics: &mut AutomationMetrics,
    expected: usize,
    timeout_ms: u64,
) -> Result<(), String> {
    if expected == 0 {
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if metrics.member_online_events as usize >= expected {
            return Ok(());
        }
        if let Some(signal) = network.try_recv_signal() {
            handle_nonblocking_signal(signal, metrics)?;
        }
        if !network.is_connected() {
            return Err("Automation client disconnected while waiting for members".into());
        }
        sleep(POLL_INTERVAL).await;
    }

    Err(format!(
        "Timed out waiting for {expected} members to come online; saw {}",
        metrics.member_online_events
    ))
}

async fn recv_signal_until(
    network: &mut NetworkClient,
    deadline: Instant,
) -> Result<SignalMessage, String> {
    loop {
        if let Some(signal) = network.try_recv_signal() {
            return Ok(signal);
        }
        if !network.is_connected() {
            return Err("Automation client disconnected".into());
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for automation signal".into());
        }
        sleep(POLL_INTERVAL).await;
    }
}

fn handle_nonblocking_signal(
    signal: SignalMessage,
    metrics: &mut AutomationMetrics,
) -> Result<(), String> {
    match signal {
        SignalMessage::Error { message } => Err(format!("Server returned error: {message}")),
        SignalMessage::Authenticated { .. } => {
            metrics.authenticated = true;
            Ok(())
        }
        SignalMessage::MemberOnline { .. } => {
            metrics.member_online_events += 1;
            Ok(())
        }
        SignalMessage::TextMessage { .. } => Ok(()),
        SignalMessage::TextChannelSelected { .. } => {
            metrics.text_channel_selected = true;
            Ok(())
        }
        SignalMessage::ScreenShareStarted { .. } => {
            metrics.screen_share_started = true;
            Ok(())
        }
        SignalMessage::ScreenShareStopped { .. } => Ok(()),
        SignalMessage::FriendSnapshot { .. }
        | SignalMessage::FriendPresenceSnapshot { .. }
        | SignalMessage::FriendPresenceChanged { .. }
        | SignalMessage::TypingState { .. }
        | SignalMessage::DirectTypingState { .. }
        | SignalMessage::MessageReaction { .. }
        | SignalMessage::MessagePinned { .. }
        | SignalMessage::TextMessageEdited { .. }
        | SignalMessage::TextMessageDeleted { .. }
        | SignalMessage::DirectMessage { .. }
        | SignalMessage::DirectMessageEdited { .. }
        | SignalMessage::DirectMessageDeleted { .. }
        | SignalMessage::DirectMessageSelected { .. }
        | SignalMessage::PeerLeft { .. }
        | SignalMessage::PeerMuteChanged { .. }
        | SignalMessage::PeerDeafenChanged { .. }
        | SignalMessage::ChannelCreated { .. }
        | SignalMessage::ChannelDeleted { .. }
        | SignalMessage::SpaceDeleted
        | SignalMessage::RoomCreated { .. }
        | SignalMessage::RoomJoined { .. }
        | SignalMessage::ChannelLeft
        | SignalMessage::SpaceJoined { .. }
        | SignalMessage::SpaceCreated { .. } => Ok(()),
        SignalMessage::PeerJoined { .. } => {
            metrics.peer_join_events += 1;
            Ok(())
        }
        SignalMessage::MemberChannelChanged { .. } => {
            metrics.member_channel_updates += 1;
            Ok(())
        }
        _ => Ok(()),
    }
}

fn select_voice_channel(channels: &[ChannelInfo]) -> Result<&ChannelInfo, String> {
    channels
        .iter()
        .find(|channel| channel.channel_type == ChannelType::Voice)
        .or_else(|| channels.first())
        .ok_or_else(|| "Space did not contain any channels".into())
}

fn select_text_channel_info(channels: &[ChannelInfo]) -> Result<&ChannelInfo, String> {
    channels
        .iter()
        .find(|channel| channel.channel_type == ChannelType::Text)
        .ok_or_else(|| "Space did not contain a text channel".into())
}

fn write_shared_info(path: &PathBuf, shared: &SharedJoinInfo) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create shared info directory: {err}"))?;
    }
    let temp_path = path.with_extension("tmp");
    let payload = serde_json::json!({
        "invite_code": shared.invite_code,
        "voice_channel_id": shared.voice_channel_id,
        "voice_channel_name": shared.voice_channel_name,
        "text_channel_id": shared.text_channel_id,
        "text_channel_name": shared.text_channel_name,
    });
    fs::write(
        &temp_path,
        serde_json::to_vec_pretty(&payload)
            .map_err(|err| format!("Failed to encode shared info: {err}"))?,
    )
    .map_err(|err| format!("Failed to write shared info: {err}"))?;
    fs::rename(&temp_path, path).map_err(|err| format!("Failed to finalize shared info: {err}"))
}

async fn wait_for_shared_info(path: &PathBuf, timeout_ms: u64) -> Result<SharedJoinInfo, String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            let parsed: serde_json::Value = serde_json::from_str(&contents)
                .map_err(|err| format!("Failed to parse shared info file: {err}"))?;
            let invite_code = parsed
                .get("invite_code")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let voice_channel_id = parsed
                .get("voice_channel_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let voice_channel_name = parsed
                .get("voice_channel_name")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let text_channel_id = parsed
                .get("text_channel_id")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let text_channel_name = parsed
                .get("text_channel_name")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            if !invite_code.is_empty() && !voice_channel_id.is_empty() && !text_channel_id.is_empty()
            {
                return Ok(SharedJoinInfo {
                    invite_code,
                    voice_channel_id,
                    voice_channel_name,
                    text_channel_id,
                    text_channel_name,
                });
            }
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for owner shared info".into());
        }
        sleep(POLL_INTERVAL).await;
    }
}

fn write_report(
    spec: &AutomationSpec,
    ok: bool,
    elapsed: std::time::Duration,
    failures: &[String],
    metrics: &AutomationMetrics,
) -> Result<(), String> {
    if let Some(parent) = spec.report_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create report directory: {err}"))?;
    }
    let payload = serde_json::json!({
        "ok": ok,
        "scenario": scenario_label(spec.scenario),
        "role": role_label(spec.role),
        "user_name": spec.user_name,
        "server_url": spec.server_url,
        "space_name": spec.space_name,
        "hold_ms": spec.hold_ms,
        "expect_peers": spec.expect_peers,
        "expect_audio": spec.expect_audio,
        "send_audio": spec.send_audio,
        "message_count": spec.message_count,
        "screen_frame_count": spec.screen_frame_count,
        "elapsed_ms": elapsed.as_millis() as u64,
        "last_status": metrics.last_status,
        "authenticated": metrics.authenticated,
        "space_ready": metrics.space_ready,
        "channel_joined": metrics.channel_joined,
        "text_channel_selected": metrics.text_channel_selected,
        "screen_share_started": metrics.screen_share_started,
        "self_channel_updates": metrics.self_channel_updates,
        "peer_join_events": metrics.peer_join_events,
        "member_channel_updates": metrics.member_channel_updates,
        "member_online_events": metrics.member_online_events,
        "audio_frames_sent": metrics.audio_frames_sent,
        "audio_frames_recv": metrics.audio_frames_recv,
        "text_messages_sent": metrics.text_messages_sent,
        "text_messages_recv": metrics.text_messages_recv,
        "screen_frames_sent": metrics.screen_frames_sent,
        "screen_frames_recv": metrics.screen_frames_recv,
        "failures": failures,
    });
    fs::write(
        &spec.report_path,
        serde_json::to_vec_pretty(&payload)
            .map_err(|err| format!("Failed to encode report: {err}"))?,
    )
    .map_err(|err| format!("Failed to write automation report: {err}"))
}

fn role_label(role: AutomationRole) -> &'static str {
    match role {
        AutomationRole::Owner => "owner",
        AutomationRole::Participant => "participant",
    }
}

fn scenario_label(scenario: AutomationScenario) -> &'static str {
    match scenario {
        AutomationScenario::SpaceChannelSoak => "space_channel_soak",
        AutomationScenario::SpaceTextChatSoak => "space_text_chat_soak",
        AutomationScenario::SpaceScreenShareSoak => "space_screen_share_soak",
    }
}

fn generate_test_audio() -> Vec<u8> {
    let sample_rate = 48_000.0_f64;
    let freq = 440.0_f64;
    let num_samples = 960;
    let mut bytes = Vec::with_capacity(num_samples * 2);
    for idx in 0..num_samples {
        let t = idx as f64 / sample_rate;
        let sample = (t * freq * 2.0 * std::f64::consts::PI).sin();
        let s16 = (sample * i16::MAX as f64) as i16;
        bytes.extend_from_slice(&s16.to_le_bytes());
    }
    bytes
}

fn generate_test_screen_frame(idx: usize) -> Vec<u8> {
    let payload = format!("synthetic-screen-frame-{idx}");
    payload.into_bytes()
}
