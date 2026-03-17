//! Live stress test — connects multiple concurrent clients to the real server
//! and exercises all major code paths to find crashes and networking bugs.
//!
//! Usage: cargo test --test live_stress_test -- --nocapture --test-threads=1
//!
//! Set VOXLINK_SERVER to override the server address (default: ws://129.158.231.26:9090)
//!
//! Tests run sequentially to avoid overwhelming the remote server.

use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

// Force sequential execution — the remote server is a free-tier VM
use std::sync::Mutex;
static SERIAL: Mutex<()> = Mutex::new(());

static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

fn server_url() -> String {
    std::env::var("VOXLINK_SERVER").unwrap_or_else(|_| "ws://129.158.231.26:9090".to_string())
}

fn unique_name(prefix: &str) -> String {
    let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    // Truncate to 32 chars (server limit)
    let name = format!("{prefix}_{n}_{}", std::process::id());
    if name.len() > 32 {
        name[..32].to_string()
    } else {
        name
    }
}

struct Client {
    sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    name: String,
}

impl Client {
    async fn connect(name: &str) -> Self {
        let url = server_url();
        let mut last_err = String::new();
        for attempt in 0..5 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(200 * attempt as u64)).await;
            }
            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws, _)) => {
                    let (sink, stream) = ws.split();
                    return Client {
                        sink,
                        stream,
                        name: name.to_string(),
                    };
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
        }
        panic!("[{name}] Failed to connect to {url} after 5 retries: {last_err}");
    }

    async fn send(&mut self, msg: &SignalMessage) {
        let json = serde_json::to_string(msg).unwrap();
        self.sink.send(Message::Text(json.into())).await.unwrap();
    }

    async fn recv(&mut self) -> SignalMessage {
        self.recv_timeout(Duration::from_secs(10))
            .await
            .unwrap_or_else(|| panic!("[{}] Timed out waiting for message", self.name))
    }

    async fn recv_timeout(&mut self, dur: Duration) -> Option<SignalMessage> {
        loop {
            match timeout(dur, self.stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(msg) = serde_json::from_str::<SignalMessage>(&text) {
                        match msg {
                            SignalMessage::SpaceAuditLogSnapshot { .. }
                            | SignalMessage::SpaceAuditLogAppended { .. } => continue,
                            other => return Some(other),
                        }
                    }
                }
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
                Ok(Some(Ok(Message::Binary(_)))) => continue, // skip audio
                _ => return None,
            }
        }
    }

    async fn drain(&mut self, dur: Duration) -> Vec<SignalMessage> {
        let mut msgs = Vec::new();
        loop {
            match self.recv_timeout(dur).await {
                Some(msg) => msgs.push(msg),
                None => break,
            }
        }
        msgs
    }

    async fn send_binary(&mut self, data: &[u8]) {
        let mut packet = Vec::with_capacity(data.len() + 1);
        packet.push(shared_types::MEDIA_PACKET_AUDIO);
        packet.extend_from_slice(data);
        self.sink
            .send(Message::Binary(packet.into()))
            .await
            .unwrap();
    }

    async fn authenticate(&mut self) -> (String, String) {
        self.send(&SignalMessage::Authenticate {
            token: None,
            user_name: self.name.clone(),
        })
        .await;
        let (token, user_id) = loop {
            match self.recv().await {
                SignalMessage::Authenticated { token, user_id } => break (token, user_id),
                _ => continue,
            }
        };
        // drain FriendSnapshot
        let _ = self.recv_timeout(Duration::from_secs(2)).await;
        (token, user_id)
    }
}

fn generate_audio() -> Vec<u8> {
    let num_samples = 960;
    let mut bytes = Vec::with_capacity(num_samples * 2);
    for i in 0..num_samples {
        let t = i as f64 / 48000.0;
        let sample = (t * 440.0 * 2.0 * std::f64::consts::PI).sin();
        let s16 = (sample * i16::MAX as f64) as i16;
        bytes.extend_from_slice(&s16.to_le_bytes());
    }
    bytes
}

// ─── Test Scenarios ───

/// Scenario 1: 10 clients connect, create rooms, join each other, exchange audio, leave.
#[tokio::test]
async fn live_stress_room_churn() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Room Churn Test (10 clients) → {url} ===");

    let errors = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();

    for i in 0..10 {
        let errors = errors.clone();
        let handle = tokio::spawn(async move {
            let name = unique_name(&format!("rc{i}"));
            let mut client = Client::connect(&name).await;

            // Create room
            client
                .send(&SignalMessage::CreateRoom {
                    user_name: name.clone(),
                    password: None,
                })
                .await;
            let room_code = loop {
                match client.recv().await {
                    SignalMessage::RoomCreated { room_code } => break room_code,
                    SignalMessage::Error { message } => {
                        eprintln!("[{name}] Error creating room: {message}");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    _ => continue,
                }
            };

            // Send some audio
            let audio = generate_audio();
            for _ in 0..10 {
                client.send_binary(&audio).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            // Leave room
            client.send(&SignalMessage::LeaveRoom).await;
            let _ = client.drain(Duration::from_millis(500)).await;

            // Join own room again (should fail — empty rooms get cleaned)
            client
                .send(&SignalMessage::JoinRoom {
                    room_code,
                    user_name: name.clone(),
                    password: None,
                })
                .await;
            let _ = client.drain(Duration::from_millis(500)).await;

            eprintln!("[{name}] ✓ room churn complete");
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }
    let err_count = errors.load(Ordering::Relaxed);
    assert_eq!(err_count, 0, "Got {err_count} errors during room churn");
    eprintln!("=== Room Churn: PASSED ===\n");
}

/// Scenario 2: Create a space, 8 clients join, chat, join voice, send audio, leave.
#[tokio::test]
async fn live_stress_space_full_lifecycle() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Space Lifecycle Test (8 clients) → {url} ===");

    // Owner creates space
    let owner_name = unique_name("own");
    let mut owner = Client::connect(&owner_name).await;
    owner.authenticate().await;

    owner
        .send(&SignalMessage::CreateSpace {
            name: format!("StressSpace_{}", std::process::id()),
            user_name: owner_name.clone(),
        })
        .await;

    let (invite_code, space_channels) = loop {
        match owner.recv().await {
            SignalMessage::SpaceCreated { space, channels } => break (space.invite_code, channels),
            _ => continue,
        }
    };
    eprintln!("[{owner_name}] Space created, invite: {invite_code}");

    // Create a text channel
    owner
        .send(&SignalMessage::CreateChannel {
            channel_name: "stress-chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = loop {
        match owner.recv().await {
            SignalMessage::ChannelCreated { channel } => break channel.id,
            _ => continue,
        }
    };

    // Find voice channel
    let voice_ch_id = space_channels
        .iter()
        .find(|c| c.channel_type == shared_types::ChannelType::Voice)
        .map(|c| c.id.clone())
        .expect("Should have voice channel");

    let errors = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();

    for i in 0..8 {
        let invite = invite_code.clone();
        let text_id = text_ch_id.clone();
        let voice_id = voice_ch_id.clone();
        let errors = errors.clone();

        let handle = tokio::spawn(async move {
            let name = unique_name(&format!("u{i}"));
            let mut client = Client::connect(&name).await;
            client.authenticate().await;

            // Join space
            client
                .send(&SignalMessage::JoinSpace {
                    invite_code: invite,
                    user_name: name.clone(),
                })
                .await;
            loop {
                match client.recv().await {
                    SignalMessage::SpaceJoined { .. } => break,
                    SignalMessage::Error { message } => {
                        eprintln!("[{name}] Error joining space: {message}");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    _ => continue,
                }
            }

            // Select text channel and send messages
            client
                .send(&SignalMessage::SelectTextChannel {
                    channel_id: text_id.clone(),
                })
                .await;
            loop {
                match client.recv().await {
                    SignalMessage::TextChannelSelected { .. } => break,
                    _ => continue,
                }
            }

            for j in 0..5 {
                client
                    .send(&SignalMessage::SendTextMessage {
                        channel_id: text_id.clone(),
                        content: format!("Stress msg {j} from {name} 🔥✨日本語"),
                        reply_to_message_id: None,
                    })
                    .await;
                // Brief pause to avoid rate limit
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            // Drain messages
            let _ = client.drain(Duration::from_millis(500)).await;

            // Join voice channel
            client
                .send(&SignalMessage::JoinChannel {
                    channel_id: voice_id.clone(),
                })
                .await;
            loop {
                match client.recv_timeout(Duration::from_secs(5)).await {
                    Some(SignalMessage::ChannelJoined { .. }) => break,
                    Some(SignalMessage::Error { message }) => {
                        eprintln!("[{name}] Error joining voice: {message}");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Some(_) => continue,
                    None => {
                        eprintln!("[{name}] Timeout joining voice channel");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            }

            // Send audio frames
            let audio = generate_audio();
            for _ in 0..20 {
                client.send_binary(&audio).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            // Leave voice
            client.send(&SignalMessage::LeaveChannel).await;
            let _ = client.drain(Duration::from_millis(500)).await;

            // Leave space
            client.send(&SignalMessage::LeaveSpace).await;
            let _ = client.drain(Duration::from_millis(500)).await;

            eprintln!("[{name}] ✓ full lifecycle complete");
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    let err_count = errors.load(Ordering::Relaxed);

    // Owner cleans up
    owner.send(&SignalMessage::DeleteSpace).await;
    let _ = owner.drain(Duration::from_millis(500)).await;

    assert_eq!(
        err_count, 0,
        "Got {err_count} errors during space lifecycle"
    );
    eprintln!("=== Space Lifecycle: PASSED ===\n");
}

/// Scenario 3: Rapid connect/disconnect — 20 clients connect and immediately drop.
#[tokio::test]
async fn live_stress_rapid_disconnect() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Rapid Disconnect Test (20 clients) → {url} ===");

    let mut handles = Vec::new();
    for i in 0..20 {
        let handle = tokio::spawn(async move {
            let name = unique_name(&format!("rd{i}"));
            let mut client = Client::connect(&name).await;

            // Authenticate
            client
                .send(&SignalMessage::Authenticate {
                    token: None,
                    user_name: name.clone(),
                })
                .await;

            // Don't even wait for response — drop immediately
            drop(client);
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    // Verify server is still alive
    tokio::time::sleep(Duration::from_millis(500)).await;
    let name = unique_name("check");
    let mut check = Client::connect(&name).await;
    check
        .send(&SignalMessage::CreateRoom {
            user_name: name,
            password: None,
        })
        .await;
    match check.recv().await {
        SignalMessage::RoomCreated { .. } => {}
        other => panic!("Server broken after rapid disconnects: {:?}", other),
    }
    eprintln!("=== Rapid Disconnect: PASSED ===\n");
}

/// Scenario 4: Reconnect with same token — verify identity persistence.
#[tokio::test]
async fn live_stress_reconnect_identity() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Reconnect Identity Test → {url} ===");

    let name = unique_name("recon");
    let mut client1 = Client::connect(&name).await;
    let (mut token, user_id) = client1.authenticate().await;
    eprintln!("[{name}] First connect: uid={user_id}");

    // Disconnect
    drop(client1);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Reconnect with same token 5 times
    for attempt in 0..5 {
        let mut client = Client::connect(&name).await;
        client
            .send(&SignalMessage::Authenticate {
                token: Some(token.clone()),
                user_name: name.clone(),
            })
            .await;
        let (t2, uid2) = loop {
            match client.recv().await {
                SignalMessage::Authenticated { token, user_id } => break (token, user_id),
                _ => continue,
            }
        };
        assert_eq!(uid2, user_id, "Attempt {attempt}: user_id should persist");
        assert_ne!(t2, token, "Attempt {attempt}: token should rotate");
        token = t2;
        let _ = client.drain(Duration::from_millis(300)).await;
        drop(client);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    eprintln!("[{name}] ✓ identity stable across 5 reconnects");
    eprintln!("=== Reconnect Identity: PASSED ===\n");
}

/// Scenario 5: Malformed data — garbage JSON, oversized frames, binary noise.
#[tokio::test]
async fn live_stress_malformed_data() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Malformed Data Test → {url} ===");

    let name = unique_name("mal");
    let mut client = Client::connect(&name).await;

    // Send garbage text
    client
        .sink
        .send(Message::Text("{{{{not json".into()))
        .await
        .unwrap();

    // Send empty text
    client.sink.send(Message::Text("".into())).await.unwrap();

    // Send unknown variant
    client
        .sink
        .send(Message::Text(
            r#"{"TotallyFakeMessage":{"foo":"bar"}}"#.into(),
        ))
        .await
        .unwrap();

    // Send huge binary (should be rejected, not crash)
    let huge = vec![0u8; 100_000];
    let mut packet = Vec::with_capacity(huge.len() + 1);
    packet.push(shared_types::MEDIA_PACKET_AUDIO);
    packet.extend_from_slice(&huge);
    client
        .sink
        .send(Message::Binary(packet.into()))
        .await
        .unwrap();

    // Send binary with invalid media type byte
    client
        .sink
        .send(Message::Binary(vec![0xFF, 0x00, 0x01].into()))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Server should still work
    client
        .send(&SignalMessage::CreateRoom {
            user_name: name.clone(),
            password: None,
        })
        .await;
    match client.recv_timeout(Duration::from_secs(5)).await {
        Some(SignalMessage::RoomCreated { .. }) => {}
        other => panic!("Server broken after malformed data: {:?}", other),
    }

    eprintln!("[{name}] ✓ server survived all malformed input");
    eprintln!("=== Malformed Data: PASSED ===\n");
}

/// Scenario 6: Concurrent space create + join race condition.
#[tokio::test]
async fn live_stress_space_race() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Space Race Condition Test (5 spaces × 4 joiners) → {url} ===");

    let errors = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();

    for s in 0..5 {
        let errors = errors.clone();
        let handle = tokio::spawn(async move {
            let owner_name = unique_name(&format!("sro{s}"));
            let mut owner = Client::connect(&owner_name).await;
            owner.authenticate().await;

            owner
                .send(&SignalMessage::CreateSpace {
                    name: format!("RaceSpace{s}_{}", std::process::id()),
                    user_name: owner_name.clone(),
                })
                .await;

            let invite = loop {
                match owner.recv().await {
                    SignalMessage::SpaceCreated { space, .. } => break space.invite_code,
                    _ => continue,
                }
            };

            // 4 joiners race to join simultaneously
            let mut join_handles = Vec::new();
            for j in 0..4 {
                let invite = invite.clone();
                let errors = errors.clone();
                let jh = tokio::spawn(async move {
                    let name = unique_name(&format!("sj{s}_{j}"));
                    let mut client = Client::connect(&name).await;
                    client.authenticate().await;

                    client
                        .send(&SignalMessage::JoinSpace {
                            invite_code: invite,
                            user_name: name.clone(),
                        })
                        .await;

                    match client.recv_timeout(Duration::from_secs(10)).await {
                        Some(SignalMessage::SpaceJoined { .. }) => {
                            eprintln!("[{name}] ✓ joined");
                        }
                        Some(SignalMessage::Error { message }) => {
                            eprintln!("[{name}] join error: {message}");
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                        other => {
                            eprintln!("[{name}] unexpected: {:?}", other);
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    // Leave
                    client.send(&SignalMessage::LeaveSpace).await;
                    let _ = client.drain(Duration::from_millis(300)).await;
                });
                join_handles.push(jh);
            }

            for jh in join_handles {
                jh.await.unwrap();
            }

            // Drain owner messages
            let _ = owner.drain(Duration::from_millis(500)).await;

            // Clean up
            owner.send(&SignalMessage::DeleteSpace).await;
            let _ = owner.drain(Duration::from_millis(300)).await;
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    let err_count = errors.load(Ordering::Relaxed);
    assert_eq!(err_count, 0, "Got {err_count} errors during space race");
    eprintln!("=== Space Race: PASSED ===\n");
}

/// Scenario 7: Voice channel with many participants sending audio simultaneously.
#[tokio::test]
async fn live_stress_voice_channel_load() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Voice Channel Load Test (6 clients, 50 frames each) → {url} ===");

    let owner_name = unique_name("vown");
    let mut owner = Client::connect(&owner_name).await;
    owner.authenticate().await;

    owner
        .send(&SignalMessage::CreateSpace {
            name: format!("VoiceLoad_{}", std::process::id()),
            user_name: owner_name.clone(),
        })
        .await;

    let (invite, voice_ch_id) = loop {
        match owner.recv().await {
            SignalMessage::SpaceCreated { space, channels } => {
                let vc = channels
                    .iter()
                    .find(|c| c.channel_type == shared_types::ChannelType::Voice)
                    .unwrap();
                break (space.invite_code, vc.id.clone());
            }
            _ => continue,
        }
    };

    // Owner joins voice
    owner
        .send(&SignalMessage::JoinChannel {
            channel_id: voice_ch_id.clone(),
        })
        .await;
    loop {
        match owner.recv().await {
            SignalMessage::ChannelJoined { .. } => break,
            _ => continue,
        }
    }

    let errors = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();

    for i in 0..6 {
        let invite = invite.clone();
        let vc_id = voice_ch_id.clone();
        let errors = errors.clone();

        let handle = tokio::spawn(async move {
            let name = unique_name(&format!("vc{i}"));
            let mut client = Client::connect(&name).await;
            client.authenticate().await;

            // Join space
            client
                .send(&SignalMessage::JoinSpace {
                    invite_code: invite,
                    user_name: name.clone(),
                })
                .await;
            loop {
                match client.recv_timeout(Duration::from_secs(10)).await {
                    Some(SignalMessage::SpaceJoined { .. }) => break,
                    Some(SignalMessage::Error { message }) => {
                        eprintln!("[{name}] space join error: {message}");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Some(_) => continue,
                    None => {
                        eprintln!("[{name}] timeout joining space");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            }

            // Join voice channel
            client
                .send(&SignalMessage::JoinChannel {
                    channel_id: vc_id.clone(),
                })
                .await;
            loop {
                match client.recv_timeout(Duration::from_secs(5)).await {
                    Some(SignalMessage::ChannelJoined { .. }) => break,
                    Some(SignalMessage::Error { message }) => {
                        eprintln!("[{name}] voice join error: {message}");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Some(_) => continue,
                    None => {
                        eprintln!("[{name}] timeout joining voice");
                        errors.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            }

            // Send 50 audio frames (simulating ~1 second of voice)
            let audio = generate_audio();
            for _ in 0..50 {
                client.send_binary(&audio).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            // Leave
            client.send(&SignalMessage::LeaveChannel).await;
            let _ = client.drain(Duration::from_millis(300)).await;
            client.send(&SignalMessage::LeaveSpace).await;
            let _ = client.drain(Duration::from_millis(300)).await;

            eprintln!("[{name}] ✓ voice load complete");
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    // Owner sends audio too while others are active
    let audio = generate_audio();
    for _ in 0..20 {
        owner.send_binary(&audio).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let err_count = errors.load(Ordering::Relaxed);

    // Cleanup
    owner.send(&SignalMessage::LeaveChannel).await;
    let _ = owner.drain(Duration::from_millis(300)).await;
    owner.send(&SignalMessage::DeleteSpace).await;
    let _ = owner.drain(Duration::from_millis(300)).await;

    assert_eq!(
        err_count, 0,
        "Got {err_count} errors during voice load test"
    );
    eprintln!("=== Voice Channel Load: PASSED ===\n");
}

/// Scenario 8: Chat message flood — many messages, edits, and deletes.
#[tokio::test]
async fn live_stress_chat_flood() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Chat Flood Test → {url} ===");

    let owner_name = unique_name("cfown");
    let mut owner = Client::connect(&owner_name).await;
    owner.authenticate().await;

    owner
        .send(&SignalMessage::CreateSpace {
            name: format!("ChatFlood_{}", std::process::id()),
            user_name: owner_name.clone(),
        })
        .await;

    let _invite = loop {
        match owner.recv().await {
            SignalMessage::SpaceCreated { space, .. } => break space.invite_code,
            _ => continue,
        }
    };

    // Create text channel
    owner
        .send(&SignalMessage::CreateChannel {
            channel_name: "flood".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = loop {
        match owner.recv().await {
            SignalMessage::ChannelCreated { channel } => break channel.id,
            _ => continue,
        }
    };

    // Owner selects channel
    owner
        .send(&SignalMessage::SelectTextChannel {
            channel_id: text_ch_id.clone(),
        })
        .await;
    loop {
        match owner.recv().await {
            SignalMessage::TextChannelSelected { .. } => break,
            _ => continue,
        }
    }

    // Send 30 messages rapidly
    for i in 0..30 {
        owner
            .send(&SignalMessage::SendTextMessage {
                channel_id: text_ch_id.clone(),
                content: format!("Flood message {i}: 🔥🌍🎉 mixed UTF-8 content αβγ"),
                reply_to_message_id: None,
            })
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Drain and collect message IDs for edit/delete
    let mut message_ids = Vec::new();
    let msgs = owner.drain(Duration::from_millis(2000)).await;
    for msg in msgs {
        if let SignalMessage::TextMessage { message, .. } = msg {
            message_ids.push(message.message_id.clone());
        }
    }

    eprintln!(
        "[{owner_name}] Sent 30 messages, got {} back",
        message_ids.len()
    );

    // Edit first 5 messages
    for id in message_ids.iter().take(5) {
        owner
            .send(&SignalMessage::EditTextMessage {
                channel_id: text_ch_id.clone(),
                message_id: id.clone(),
                new_content: format!("EDITED: {id}"),
            })
            .await;
    }

    // Delete next 5
    for id in message_ids.iter().skip(5).take(5) {
        owner
            .send(&SignalMessage::DeleteTextMessage {
                channel_id: text_ch_id.clone(),
                message_id: id.clone(),
            })
            .await;
    }

    let _ = owner.drain(Duration::from_millis(1000)).await;

    // Cleanup
    owner.send(&SignalMessage::DeleteSpace).await;
    let _ = owner.drain(Duration::from_millis(300)).await;

    eprintln!("[{owner_name}] ✓ chat flood + edits + deletes complete");
    eprintln!("=== Chat Flood: PASSED ===\n");
}

/// Scenario 9: Friend system — add friends, send DMs, remove friends.
#[tokio::test]
async fn live_stress_friend_system() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Friend System Test → {url} ===");

    let mut alice = Client::connect(&unique_name("fa")).await;
    let (_, alice_uid) = alice.authenticate().await;

    let mut bob = Client::connect(&unique_name("fb")).await;
    let (_, bob_uid) = bob.authenticate().await;

    // Alice sends friend request
    alice
        .send(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;
    let _ = alice.drain(Duration::from_millis(500)).await;
    let _ = bob.drain(Duration::from_millis(500)).await;

    // Bob accepts
    bob.send(&SignalMessage::RespondFriendRequest {
        user_id: alice_uid.clone(),
        accept: true,
    })
    .await;
    let _ = bob.drain(Duration::from_millis(500)).await;
    let _ = alice.drain(Duration::from_millis(500)).await;

    // Send DMs both ways
    alice
        .send(&SignalMessage::SelectDirectMessage {
            user_id: bob_uid.clone(),
        })
        .await;
    let _ = alice.drain(Duration::from_millis(500)).await;

    bob.send(&SignalMessage::SelectDirectMessage {
        user_id: alice_uid.clone(),
    })
    .await;
    let _ = bob.drain(Duration::from_millis(500)).await;

    for i in 0..5 {
        alice
            .send(&SignalMessage::SendDirectMessage {
                user_id: bob_uid.clone(),
                content: format!("Alice says {i} 🎈"),
                reply_to_message_id: None,
            })
            .await;
        bob.send(&SignalMessage::SendDirectMessage {
            user_id: alice_uid.clone(),
            content: format!("Bob says {i} ✨"),
            reply_to_message_id: None,
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = alice.drain(Duration::from_millis(1000)).await;
    let _ = bob.drain(Duration::from_millis(1000)).await;

    // Remove friend
    alice
        .send(&SignalMessage::RemoveFriend {
            user_id: bob_uid.clone(),
        })
        .await;
    let _ = alice.drain(Duration::from_millis(500)).await;
    let _ = bob.drain(Duration::from_millis(500)).await;

    eprintln!("=== Friend System: PASSED ===\n");
}

/// Scenario 10: Everything at once — rooms, spaces, friends, chat, voice, disconnects.
#[tokio::test]
async fn live_stress_combined_chaos() {
    let _lock = SERIAL.lock().unwrap();
    let url = server_url();
    eprintln!("=== Combined Chaos Test (15 clients) → {url} ===");

    let errors = Arc::new(AtomicU32::new(0));
    let mut handles = Vec::new();

    // 5 room users
    for i in 0..5 {
        let errors = errors.clone();
        handles.push(tokio::spawn(async move {
            let name = unique_name(&format!("cr{i}"));
            let mut client = Client::connect(&name).await;
            client
                .send(&SignalMessage::CreateRoom {
                    user_name: name.clone(),
                    password: None,
                })
                .await;
            match client.recv_timeout(Duration::from_secs(5)).await {
                Some(SignalMessage::RoomCreated { .. }) => {
                    let audio = generate_audio();
                    for _ in 0..15 {
                        client.send_binary(&audio).await;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    client.send(&SignalMessage::LeaveRoom).await;
                }
                Some(SignalMessage::Error { message }) => {
                    eprintln!("[{name}] room error: {message}");
                    errors.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }));
    }

    // 5 space users
    let space_owner_name = unique_name("cso");
    let mut space_owner = Client::connect(&space_owner_name).await;
    space_owner.authenticate().await;
    space_owner
        .send(&SignalMessage::CreateSpace {
            name: format!("Chaos_{}", std::process::id()),
            user_name: space_owner_name.clone(),
        })
        .await;
    let invite = loop {
        match space_owner.recv().await {
            SignalMessage::SpaceCreated { space, .. } => break space.invite_code,
            _ => continue,
        }
    };

    for i in 0..5 {
        let invite = invite.clone();
        let errors = errors.clone();
        handles.push(tokio::spawn(async move {
            let name = unique_name(&format!("cs{i}"));
            let mut client = Client::connect(&name).await;
            client.authenticate().await;
            client
                .send(&SignalMessage::JoinSpace {
                    invite_code: invite,
                    user_name: name.clone(),
                })
                .await;
            match client.recv_timeout(Duration::from_secs(10)).await {
                Some(SignalMessage::SpaceJoined { .. }) => {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    client.send(&SignalMessage::LeaveSpace).await;
                }
                Some(SignalMessage::Error { message }) => {
                    eprintln!("[{name}] space error: {message}");
                    errors.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }));
    }

    // 5 rapid disconnectors
    for i in 0..5 {
        handles.push(tokio::spawn(async move {
            let name = unique_name(&format!("cd{i}"));
            let mut client = Client::connect(&name).await;
            client.authenticate().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(client);
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // Cleanup space
    let _ = space_owner.drain(Duration::from_millis(500)).await;
    space_owner.send(&SignalMessage::DeleteSpace).await;
    let _ = space_owner.drain(Duration::from_millis(300)).await;

    let err_count = errors.load(Ordering::Relaxed);
    assert_eq!(err_count, 0, "Got {err_count} errors during chaos test");
    eprintln!("=== Combined Chaos: PASSED ===\n");
}
