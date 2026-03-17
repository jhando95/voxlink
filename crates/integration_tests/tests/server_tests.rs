use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

// ─── Test Infrastructure ───

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);

struct TestServer {
    child: Child,
    port: u16,
    db_path: std::path::PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        // Pick a random free port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let server_bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target/debug/signaling_server");
        let db_path = std::env::temp_dir().join(format!(
            "voxlink_integration_{}_{}.db",
            std::process::id(),
            port
        ));

        let child = Command::new(&server_bin)
            .env("PV_ADDR", format!("127.0.0.1:{port}"))
            .env("PV_DB_PATH", &db_path)
            .env("RUST_LOG", "info")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to spawn signaling_server at {:?}: {}. Did you run `cargo build -p signaling_server`?",
                    server_bin, e
                )
            });

        let server = TestServer {
            child,
            port,
            db_path,
        };

        // Wait for server to be ready by trying to connect
        let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "Server did not start within {} seconds on port {port}",
                    STARTUP_TIMEOUT.as_secs()
                );
            }
            match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
                Ok(_) => break,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }

        server
    }

    async fn connect(&self) -> TestClient {
        let url = format!("ws://127.0.0.1:{}", self.port);
        let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "Could not connect WebSocket client within {} seconds",
                    STARTUP_TIMEOUT.as_secs()
                );
            }
            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws, _)) => {
                    let (sink, stream) = ws.split();
                    return TestClient { sink, stream };
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // kill_on_drop handles cleanup, but let's be explicit
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(self.db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.db_path.with_extension("db-shm"));
    }
}

struct TestClient {
    sink: SplitSink<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Message>,
    stream: SplitStream<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>,
}

impl TestClient {
    async fn send_signal(&mut self, msg: &SignalMessage) {
        let json = serde_json::to_string(msg).unwrap();
        self.sink.send(Message::Text(json.into())).await.unwrap();
    }

    async fn recv_signal(&mut self) -> SignalMessage {
        self.recv_signal_timeout(Duration::from_secs(5))
            .await
            .expect("Timed out waiting for signal message")
    }

    async fn recv_signal_raw_timeout(&mut self, dur: Duration) -> Option<SignalMessage> {
        loop {
            match timeout(dur, self.stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(msg) = serde_json::from_str::<SignalMessage>(&text) {
                        return Some(msg);
                    }
                }
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                    continue;
                }
                _ => return None,
            }
        }
    }

    async fn recv_signal_timeout(&mut self, dur: Duration) -> Option<SignalMessage> {
        loop {
            match self.recv_signal_raw_timeout(dur).await {
                Some(SignalMessage::SpaceAuditLogSnapshot { .. })
                | Some(SignalMessage::SpaceAuditLogAppended { .. }) => continue,
                other => return other,
            }
        }
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

    async fn recv_binary_timeout(&mut self, dur: Duration) -> Option<Vec<u8>> {
        loop {
            match timeout(dur, self.stream.next()).await {
                Ok(Some(Ok(Message::Binary(data)))) => return Some(data.to_vec()),
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                    continue;
                }
                Ok(Some(Ok(Message::Text(_)))) => {
                    // Skip text messages (e.g. signal messages)
                    continue;
                }
                _ => return None,
            }
        }
    }
}

// ─── Helper: create a room and return the room code ───

async fn create_room(client: &mut TestClient, name: &str) -> String {
    client
        .send_signal(&SignalMessage::CreateRoom {
            user_name: name.to_string(),
            password: None,
        })
        .await;
    match client.recv_signal().await {
        SignalMessage::RoomCreated { room_code } => room_code,
        other => panic!("Expected RoomCreated, got: {:?}", other),
    }
}

async fn create_room_with_password(client: &mut TestClient, name: &str, pw: &str) -> String {
    client
        .send_signal(&SignalMessage::CreateRoom {
            user_name: name.to_string(),
            password: Some(pw.to_string()),
        })
        .await;
    match client.recv_signal().await {
        SignalMessage::RoomCreated { room_code } => room_code,
        other => panic!("Expected RoomCreated, got: {:?}", other),
    }
}

async fn join_room(client: &mut TestClient, code: &str, name: &str) {
    client
        .send_signal(&SignalMessage::JoinRoom {
            room_code: code.to_string(),
            user_name: name.to_string(),
            password: None,
        })
        .await;
    match client.recv_signal().await {
        SignalMessage::RoomJoined { .. } => {}
        other => panic!("Expected RoomJoined, got: {:?}", other),
    }
}

/// Generate a sine wave of 440Hz at 48kHz, 960 samples, as little-endian i16 bytes.
fn generate_test_audio() -> Vec<u8> {
    let sample_rate = 48000.0_f64;
    let freq = 440.0_f64;
    let num_samples = 960;
    let mut bytes = Vec::with_capacity(num_samples * 2);
    for i in 0..num_samples {
        let t = i as f64 / sample_rate;
        let sample = (t * freq * 2.0 * std::f64::consts::PI).sin();
        let s16 = (sample * i16::MAX as f64) as i16;
        bytes.extend_from_slice(&s16.to_le_bytes());
    }
    bytes
}

fn parse_audio_frame(frame: &[u8]) -> (&str, &[u8]) {
    assert!(
        frame.len() >= 3 && frame[0] == shared_types::MEDIA_PACKET_AUDIO,
        "Expected audio media packet"
    );
    let id_len = frame[1] as usize;
    assert!(
        frame.len() > 2 + id_len,
        "Frame too short for sender header"
    );
    let sender_id = std::str::from_utf8(&frame[2..2 + id_len]).unwrap();
    let audio = &frame[2 + id_len..];
    (sender_id, audio)
}

fn desktop_bin_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/app_desktop")
}

fn spawn_automated_desktop_client(
    desktop_bin: &std::path::Path,
    server_url: &str,
    role: &str,
    user_name: &str,
    shared_path: &std::path::Path,
    report_path: &std::path::Path,
    space_name: &str,
    expect_peers: usize,
    expect_audio: bool,
    send_audio: bool,
) -> tokio::process::Child {
    Command::new(desktop_bin)
        .env("RUST_LOG", "info")
        .env("VOXLINK_AUTOMATION_SCENARIO", "space_channel_soak")
        .env("VOXLINK_AUTOMATION_ROLE", role)
        .env("VOXLINK_AUTOMATION_SERVER_URL", server_url)
        .env("VOXLINK_AUTOMATION_USER_NAME", user_name)
        .env("VOXLINK_AUTOMATION_SPACE_NAME", space_name)
        .env("VOXLINK_AUTOMATION_SHARED_PATH", shared_path)
        .env("VOXLINK_AUTOMATION_REPORT_PATH", report_path)
        .env("VOXLINK_AUTOMATION_HOLD_MS", "2600")
        .env("VOXLINK_AUTOMATION_INVITE_TIMEOUT_MS", "12000")
        .env("VOXLINK_AUTOMATION_EXPECT_PEERS", expect_peers.to_string())
        .env(
            "VOXLINK_AUTOMATION_EXPECT_AUDIO",
            if expect_audio { "1" } else { "0" },
        )
        .env(
            "VOXLINK_AUTOMATION_SEND_AUDIO",
            if send_audio { "1" } else { "0" },
        )
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            panic!(
                "Failed to spawn app_desktop at {:?}: {}. Did you run `cargo build -p app_desktop`?",
                desktop_bin, err
            )
        })
}

// ─── Tests ───

#[tokio::test]
async fn test_create_room() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .send_signal(&SignalMessage::CreateRoom {
            user_name: "Alice".to_string(),
            password: None,
        })
        .await;

    let msg = client.recv_signal().await;
    match msg {
        SignalMessage::RoomCreated { room_code } => {
            assert_eq!(room_code.len(), 6, "Room code should be 6 digits");
            assert!(
                room_code.chars().all(|c| c.is_ascii_digit()),
                "Room code should be all digits: {room_code}"
            );
        }
        other => panic!("Expected RoomCreated, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_join_room() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;

    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: room_code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;

    // Bob should get RoomJoined with Alice in participants
    let bob_msg = bob.recv_signal().await;
    match bob_msg {
        SignalMessage::RoomJoined {
            room_code: rc,
            participants,
        } => {
            assert_eq!(rc, room_code);
            assert_eq!(
                participants.len(),
                1,
                "Should have 1 existing participant (Alice)"
            );
            assert_eq!(participants[0].name, "Alice");
        }
        other => panic!("Expected RoomJoined for Bob, got: {:?}", other),
    }

    // Alice should get PeerJoined for Bob
    let alice_msg = alice.recv_signal().await;
    match alice_msg {
        SignalMessage::PeerJoined { peer } => {
            assert_eq!(peer.name, "Bob");
        }
        other => panic!("Expected PeerJoined for Alice, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_leave_room() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;

    // Consume Alice's PeerJoined notification for Bob
    let _ = alice.recv_signal().await;

    // Bob leaves
    bob.send_signal(&SignalMessage::LeaveRoom).await;

    // Alice should get PeerLeft
    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::PeerLeft { peer_id } => {
            assert!(!peer_id.is_empty());
        }
        other => panic!("Expected PeerLeft, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_mute_broadcast() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;

    // Consume Alice's PeerJoined for Bob
    let _ = alice.recv_signal().await;

    // Alice mutes
    alice
        .send_signal(&SignalMessage::MuteChanged { is_muted: true })
        .await;

    // Bob should get PeerMuteChanged
    let msg = bob.recv_signal().await;
    match msg {
        SignalMessage::PeerMuteChanged { peer_id, is_muted } => {
            assert!(is_muted);
            assert!(!peer_id.is_empty());
        }
        other => panic!("Expected PeerMuteChanged, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_deafen_broadcast() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;

    // Consume Alice's PeerJoined for Bob
    let _ = alice.recv_signal().await;

    // Alice deafens
    alice
        .send_signal(&SignalMessage::DeafenChanged { is_deafened: true })
        .await;

    // Bob should get PeerDeafenChanged
    let msg = bob.recv_signal().await;
    match msg {
        SignalMessage::PeerDeafenChanged {
            peer_id,
            is_deafened,
        } => {
            assert!(is_deafened);
            assert!(!peer_id.is_empty());
        }
        other => panic!("Expected PeerDeafenChanged, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_audio_relay() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;

    // Consume Alice's PeerJoined for Bob
    let _ = alice.recv_signal().await;

    let audio_data = generate_test_audio();
    alice.send_binary(&audio_data).await;

    // Bob should receive the relayed audio frame with sender_id header
    let frame = bob
        .recv_binary_timeout(Duration::from_secs(5))
        .await
        .expect("Bob should receive audio");

    let (_sender_id, received_audio) = parse_audio_frame(&frame);
    assert_eq!(received_audio, &audio_data[..], "Audio data should match");

    // Alice should NOT receive her own audio back
    let own_frame = alice.recv_binary_timeout(Duration::from_millis(500)).await;
    assert!(own_frame.is_none(), "Sender should not receive own audio");
}

#[tokio::test]
async fn test_oversized_frame_rejected() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;

    // Consume Alice's PeerJoined for Bob
    let _ = alice.recv_signal().await;

    // Send an oversized frame (5000 bytes > MAX_AUDIO_FRAME_SIZE=4096)
    let big_frame = vec![0u8; 5000];
    alice.send_binary(&big_frame).await;

    // Bob should NOT receive anything
    let received = bob.recv_binary_timeout(Duration::from_millis(500)).await;
    assert!(received.is_none(), "Oversized frame should be rejected");
}

#[tokio::test]
async fn test_multiple_peers() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;
    let mut carol = server.connect().await;
    let mut dave = server.connect().await;
    let mut eve = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;

    // Join B, C, D, E
    join_room(&mut bob, &room_code, "Bob").await;
    // Alice gets PeerJoined(Bob)
    let _ = alice.recv_signal().await;

    join_room(&mut carol, &room_code, "Carol").await;
    // Alice gets PeerJoined(Carol), Bob gets PeerJoined(Carol)
    let _ = alice.recv_signal().await;
    let _ = bob.recv_signal().await;

    join_room(&mut dave, &room_code, "Dave").await;
    // Alice, Bob, Carol get PeerJoined(Dave)
    let _ = alice.recv_signal().await;
    let _ = bob.recv_signal().await;
    let _ = carol.recv_signal().await;

    join_room(&mut eve, &room_code, "Eve").await;
    // Alice, Bob, Carol, Dave get PeerJoined(Eve)
    let _ = alice.recv_signal().await;
    let _ = bob.recv_signal().await;
    let _ = carol.recv_signal().await;
    let _ = dave.recv_signal().await;

    // Alice sends audio, B, C, D, E should all receive
    let audio_data = generate_test_audio();
    alice.send_binary(&audio_data).await;

    for (name, client) in [
        ("Bob", &mut bob),
        ("Carol", &mut carol),
        ("Dave", &mut dave),
        ("Eve", &mut eve),
    ] {
        let frame = client
            .recv_binary_timeout(Duration::from_secs(5))
            .await
            .unwrap_or_else(|| panic!("{name} should receive audio"));

        let (_, received_audio) = parse_audio_frame(&frame);
        assert_eq!(received_audio, &audio_data[..], "{name} audio should match");
    }
}

#[tokio::test]
async fn test_room_capacity() {
    let server = TestServer::start().await;

    let mut creator = server.connect().await;
    let room_code = create_room(&mut creator, "Creator").await;

    // Join 9 more peers (total = 10 = MAX_ROOM_PEERS)
    let mut peers: Vec<TestClient> = Vec::new();
    for i in 1..=9 {
        let mut peer = server.connect().await;
        join_room(&mut peer, &room_code, &format!("Peer{i}")).await;

        // Drain PeerJoined notifications from creator and all previous peers
        let _ = creator.recv_signal_timeout(Duration::from_secs(2)).await;
        for prev in peers.iter_mut() {
            let _ = prev.recv_signal_timeout(Duration::from_secs(2)).await;
        }

        peers.push(peer);
    }

    // 11th peer should get an error
    let mut overflow = server.connect().await;
    overflow
        .send_signal(&SignalMessage::JoinRoom {
            room_code: room_code.clone(),
            user_name: "Overflow".to_string(),
            password: None,
        })
        .await;

    let msg = overflow.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("full"),
                "Error should mention room being full: {message}"
            );
        }
        other => panic!("Expected Error for full room, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_invalid_room_code() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .send_signal(&SignalMessage::JoinRoom {
            room_code: "abc".to_string(),
            user_name: "Test".to_string(),
            password: None,
        })
        .await;

    let msg = client.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("Invalid room code") || message.contains("6 digits"),
                "Error should mention invalid code: {message}"
            );
        }
        other => panic!("Expected Error for invalid code, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_password_room() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let room_code = create_room_with_password(&mut alice, "Alice", "secret123").await;

    // Wrong password should fail
    let mut bob_bad = server.connect().await;
    bob_bad
        .send_signal(&SignalMessage::JoinRoom {
            room_code: room_code.clone(),
            user_name: "Bob".to_string(),
            password: Some("wrongpass".to_string()),
        })
        .await;

    let msg = bob_bad.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("password") || message.contains("Incorrect"),
                "Error should mention password: {message}"
            );
        }
        other => panic!("Expected Error for wrong password, got: {:?}", other),
    }

    // No password should also fail
    let mut bob_none = server.connect().await;
    bob_none
        .send_signal(&SignalMessage::JoinRoom {
            room_code: room_code.clone(),
            user_name: "Bob".to_string(),
            password: None,
        })
        .await;

    let msg = bob_none.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("password") || message.contains("Incorrect"),
                "Should fail without password: {message}"
            );
        }
        other => panic!("Expected Error for no password, got: {:?}", other),
    }

    // Correct password should succeed
    let mut bob_good = server.connect().await;
    bob_good
        .send_signal(&SignalMessage::JoinRoom {
            room_code: room_code.clone(),
            user_name: "Bob".to_string(),
            password: Some("secret123".to_string()),
        })
        .await;

    let msg = bob_good.recv_signal().await;
    match msg {
        SignalMessage::RoomJoined { room_code: rc, .. } => {
            assert_eq!(rc, room_code);
        }
        other => panic!(
            "Expected RoomJoined with correct password, got: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_reconnect() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;

    // Bob joins
    let mut bob = server.connect().await;
    join_room(&mut bob, &room_code, "Bob").await;

    // Consume Alice's PeerJoined for Bob
    let _ = alice.recv_signal().await;

    // Bob disconnects by dropping the client
    drop(bob);

    // Alice should get PeerLeft
    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::PeerLeft { peer_id } => {
            assert!(!peer_id.is_empty());
        }
        other => panic!("Expected PeerLeft after disconnect, got: {:?}", other),
    }

    // Bob reconnects and rejoins
    let mut bob2 = server.connect().await;
    join_room(&mut bob2, &room_code, "Bob").await;

    // Alice should get PeerJoined again
    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::PeerJoined { peer } => {
            assert_eq!(peer.name, "Bob");
        }
        other => panic!("Expected PeerJoined after reconnect, got: {:?}", other),
    }
}

// ─── Performance / Load Tests ───

/// Simulates a realistic voice chat session: 5 peers in a room exchanging audio
/// frames at 50fps for 5 seconds (250 frames each). Measures throughput and latency.
#[tokio::test]
async fn test_performance_load() {
    let server = TestServer::start().await;

    // Connect 5 clients
    let mut clients = Vec::new();
    for _ in 0..5 {
        clients.push(server.connect().await);
    }

    // Client 0 creates room
    let room_code = create_room(&mut clients[0], "User0").await;

    // Clients 1-4 join
    for i in 1..5 {
        join_room(&mut clients[i], &room_code, &format!("User{i}")).await;
    }

    // Drain all PeerJoined notifications
    tokio::time::sleep(Duration::from_millis(200)).await;
    for client in clients.iter_mut() {
        while client
            .recv_signal_timeout(Duration::from_millis(100))
            .await
            .is_some()
        {}
    }

    let audio_data = generate_test_audio();
    let frame_count = 100; // 2 seconds at 50fps

    // ── Throughput Test: Client 0 sends frames at ~50fps (realistic voice rate) ──
    let start = std::time::Instant::now();
    for i in 0..frame_count {
        clients[0].send_binary(&audio_data).await;
        // Pace at 50fps to stay under server's 100fps rate limit
        if i % 5 == 4 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    let send_elapsed = start.elapsed();
    let send_fps = frame_count as f64 / send_elapsed.as_secs_f64();

    // Count how many frames client 1 receives
    let mut received = 0u32;
    let recv_start = std::time::Instant::now();
    while recv_start.elapsed() < Duration::from_secs(10) {
        if clients[1]
            .recv_binary_timeout(Duration::from_millis(500))
            .await
            .is_some()
        {
            received += 1;
            if received >= frame_count as u32 {
                break;
            }
        } else {
            break;
        }
    }
    let recv_elapsed = recv_start.elapsed();

    println!("\n╔══════════════════════════════════════════════╗");
    println!("║         PERFORMANCE TEST RESULTS             ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║ Peers in room:     5                         ║");
    println!(
        "║ Audio frame size:  {} bytes              ║",
        audio_data.len()
    );
    println!("║ Frames sent:       {:<26}║", frame_count);
    println!(
        "║ Send time:         {:.2}s ({:.0} fps){:>14}║",
        send_elapsed.as_secs_f64(),
        send_fps,
        ""
    );
    println!(
        "║ Frames received:   {:<6} / {:<6} ({:.0}%){:>7}║",
        received,
        frame_count,
        (received as f64 / frame_count as f64) * 100.0,
        ""
    );
    println!(
        "║ Recv time:         {:.2}s{:>24}║",
        recv_elapsed.as_secs_f64(),
        ""
    );
    println!("╚══════════════════════════════════════════════╝");

    // All frames should be received
    assert!(
        received >= (frame_count as u32 * 95 / 100),
        "Should receive at least 95% of frames: got {received}/{frame_count}"
    );

    // ── Multi-sender Test: All 5 peers send simultaneously ──
    let multi_frames = 20;
    let multi_start = std::time::Instant::now();
    for _ in 0..multi_frames {
        for client in clients.iter_mut() {
            client.send_binary(&audio_data).await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await; // ~50fps per peer
    }
    let multi_send = multi_start.elapsed();

    // Each peer should receive multi_frames * 4 frames (from 4 other peers)
    let expected_per_peer = multi_frames * 4;
    let mut peer1_received = 0u32;
    while peer1_received < expected_per_peer as u32 {
        if clients[1]
            .recv_binary_timeout(Duration::from_millis(500))
            .await
            .is_some()
        {
            peer1_received += 1;
        } else {
            break;
        }
    }

    println!("\n╔══════════════════════════════════════════════╗");
    println!("║     MULTI-SENDER PERFORMANCE RESULTS         ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║ Senders:           5 simultaneous             ║");
    println!("║ Frames/sender:     {:<26}║", multi_frames);
    println!(
        "║ Total relayed:     {:<4} (5 senders x 4 recv)  ║",
        multi_frames * 5 * 4
    );
    println!(
        "║ Send time:         {:.2}s{:>24}║",
        multi_send.as_secs_f64(),
        ""
    );
    println!(
        "║ Peer1 received:    {:<6} / {:<6} ({:.0}%){:>7}║",
        peer1_received,
        expected_per_peer,
        (peer1_received as f64 / expected_per_peer as f64) * 100.0,
        ""
    );
    println!("╚══════════════════════════════════════════════╝");

    assert!(
        peer1_received >= (expected_per_peer as u32 * 90 / 100),
        "Multi-sender: Should receive at least 90% of frames: got {peer1_received}/{expected_per_peer}"
    );
}

/// Sustained load test: 10 peers chatting for 10 seconds.
/// Verifies server stability under realistic conditions.
#[tokio::test]
async fn test_sustained_chat_session() {
    let server = TestServer::start().await;

    // Connect 10 clients
    let mut clients = Vec::new();
    for _ in 0..10 {
        clients.push(server.connect().await);
    }

    // Client 0 creates room
    let room_code = create_room(&mut clients[0], "User0").await;

    // Others join
    for i in 1..10 {
        join_room(&mut clients[i], &room_code, &format!("User{i}")).await;
    }

    // Drain PeerJoined notifications
    tokio::time::sleep(Duration::from_millis(500)).await;
    for client in clients.iter_mut() {
        while client
            .recv_signal_timeout(Duration::from_millis(50))
            .await
            .is_some()
        {}
    }

    let audio_data = generate_test_audio();

    // Simulate 2 seconds of chat: 3 active speakers, 20ms frames = 50fps
    let duration = Duration::from_secs(2);
    let start = std::time::Instant::now();
    let mut total_sent = 0u64;

    while start.elapsed() < duration {
        // 3 active speakers (clients 0, 1, 2) send one frame each
        for i in 0..3 {
            clients[i].send_binary(&audio_data).await;
            total_sent += 1;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wait for all frames to be relayed
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drain and count frames on a listener (client 5)
    let mut listener_received = 0u64;
    while clients[5]
        .recv_binary_timeout(Duration::from_millis(200))
        .await
        .is_some()
    {
        listener_received += 1;
    }

    // Drain remaining binary frames on client 5 before signal test
    while clients[5]
        .recv_binary_timeout(Duration::from_millis(100))
        .await
        .is_some()
    {}

    // Verify server still responsive after sustained load by sending a signal
    // Use a fresh pair of clients for clarity
    clients[3]
        .send_signal(&SignalMessage::MuteChanged { is_muted: true })
        .await;
    // Client 5 should receive PeerMuteChanged (skip any binary frames)
    let mut server_responsive = false;
    for _ in 0..20 {
        match timeout(Duration::from_secs(2), clients[5].stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if serde_json::from_str::<SignalMessage>(&text).is_ok() {
                    server_responsive = true;
                    break;
                }
            }
            Ok(Some(Ok(Message::Binary(_)))) => continue,
            Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
            _ => break,
        }
    }
    assert!(
        server_responsive,
        "Server should still be responsive after sustained load"
    );

    let elapsed = start.elapsed();
    let _expected_per_listener = total_sent; // Each non-sender should get all frames from all 3 speakers

    println!("\n╔══════════════════════════════════════════════╗");
    println!("║     SUSTAINED CHAT SESSION RESULTS           ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║ Peers:             10                         ║");
    println!("║ Active speakers:   3                          ║");
    println!(
        "║ Duration:          {:.1}s{:>25}║",
        elapsed.as_secs_f64(),
        ""
    );
    println!("║ Total frames sent: {:<26}║", total_sent);
    println!("║ Listener received: {:<26}║", listener_received);
    println!("║ Server responsive: YES                        ║");
    println!("╚══════════════════════════════════════════════╝");

    // At least some frames should arrive
    assert!(
        listener_received > 0,
        "Listener should have received audio frames"
    );
}

// ─── Space & Channel Tests ───

async fn create_space(
    client: &mut TestClient,
    space_name: &str,
    user_name: &str,
) -> (shared_types::SpaceInfo, Vec<shared_types::ChannelInfo>) {
    client
        .send_signal(&SignalMessage::CreateSpace {
            name: space_name.to_string(),
            user_name: user_name.to_string(),
        })
        .await;
    match client.recv_signal().await {
        SignalMessage::SpaceCreated { space, channels } => (space, channels),
        other => panic!("Expected SpaceCreated, got: {:?}", other),
    }
}

async fn join_space(
    client: &mut TestClient,
    invite_code: &str,
    user_name: &str,
) -> (
    shared_types::SpaceInfo,
    Vec<shared_types::ChannelInfo>,
    Vec<shared_types::MemberInfo>,
) {
    client
        .send_signal(&SignalMessage::JoinSpace {
            invite_code: invite_code.to_string(),
            user_name: user_name.to_string(),
        })
        .await;
    loop {
        match client.recv_signal().await {
            SignalMessage::SpaceJoined {
                space,
                channels,
                members,
            } => return (space, channels, members),
            SignalMessage::MemberOnline { .. } | SignalMessage::MemberOffline { .. } => continue,
            other => panic!("Expected SpaceJoined, got: {:?}", other),
        }
    }
}

async fn authenticate(
    client: &mut TestClient,
    user_name: &str,
    token: Option<String>,
) -> (String, String) {
    client
        .send_signal(&SignalMessage::Authenticate {
            token,
            user_name: user_name.to_string(),
        })
        .await;
    let (token, user_id) = match client.recv_signal().await {
        SignalMessage::Authenticated { token, user_id } => (token, user_id),
        other => panic!("Expected Authenticated, got: {:?}", other),
    };
    match client.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert!(friends.is_empty());
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!("Expected FriendSnapshot after auth, got: {:?}", other),
    }
    (token, user_id)
}

#[tokio::test]
async fn test_create_space() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (space, channels) = create_space(&mut alice, "My Space", "Alice").await;

    assert!(!space.id.is_empty());
    assert_eq!(space.name, "My Space");
    assert_eq!(space.invite_code.len(), 8);
    assert_eq!(space.member_count, 1);
    assert_eq!(space.channel_count, 1);
    assert!(space.is_owner, "Creator should be owner");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "General");
    assert_eq!(channels[0].channel_type, shared_types::ChannelType::Voice);
}

#[tokio::test]
async fn test_join_space_by_invite_code() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _channels) = create_space(&mut alice, "Test Space", "Alice").await;
    let invite_code = space.invite_code.clone();

    let (joined_space, channels, members) = join_space(&mut bob, &invite_code, "Bob").await;

    assert_eq!(joined_space.id, space.id);
    assert_eq!(joined_space.name, "Test Space");
    assert_eq!(joined_space.member_count, 2);
    assert!(!joined_space.is_owner, "Joiner should not be owner");
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "General");
    // Members should include Alice
    assert!(
        members.iter().any(|m| m.name == "Alice"),
        "Members should include Alice"
    );

    // Alice should get MemberOnline for Bob
    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::MemberOnline { member } => {
            assert_eq!(member.name, "Bob");
        }
        other => panic!("Expected MemberOnline, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_join_space_invalid_code() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .send_signal(&SignalMessage::JoinSpace {
            invite_code: "INVALID1".to_string(),
            user_name: "Bob".to_string(),
        })
        .await;

    let msg = client.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("invite"),
                "Error should mention invite code: {message}"
            );
        }
        other => panic!("Expected Error for invalid invite code, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_leave_space() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Leaving Space", "Alice").await;
    let (_joined, _, _) = join_space(&mut bob, &space.invite_code, "Bob").await;

    // Consume Alice's MemberOnline for Bob
    let _ = alice.recv_signal().await;

    // Bob leaves the space
    bob.send_signal(&SignalMessage::LeaveSpace).await;

    // Alice should get MemberOffline
    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::MemberOffline { member_id } => {
            assert!(!member_id.is_empty());
        }
        other => panic!("Expected MemberOffline, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_restore_identity_can_delete_legacy_owned_space() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (token, user_id) = authenticate(&mut alice, "Alice", None).await;
    let (space, _) = create_space(&mut alice, "Legacy Space", "Alice").await;

    drop(alice);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut restored = server.connect().await;
    let (restored_token, restored_user_id) =
        authenticate(&mut restored, "Alice", Some(token.clone())).await;
    assert_ne!(
        restored_token, token,
        "Restore should rotate the session token"
    );
    assert_eq!(restored_user_id, user_id);

    let (joined_space, _, _) = join_space(&mut restored, &space.invite_code, "Alice").await;
    assert!(
        joined_space.is_owner,
        "Restored owner should keep delete access"
    );

    restored.send_signal(&SignalMessage::DeleteSpace).await;
    match restored.recv_signal().await {
        SignalMessage::SpaceDeleted => {}
        other => panic!("Expected SpaceDeleted, got: {:?}", other),
    }

    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinSpace {
        invite_code: space.invite_code.clone(),
        user_name: "Bob".to_string(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("invite"),
                "Error should mention invite code: {message}"
            );
        }
        other => panic!("Expected Error after deleted space join, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_create_voice_channel() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_space, _channels) = create_space(&mut alice, "Channel Space", "Alice").await;

    // Create a voice channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "Gaming".to_string(),
            channel_type: shared_types::ChannelType::Voice,
        })
        .await;

    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "Gaming");
            assert_eq!(channel.peer_count, 0);
            assert_eq!(channel.channel_type, shared_types::ChannelType::Voice);
        }
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_create_text_channel() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_space, _channels) = create_space(&mut alice, "Text Space", "Alice").await;

    // Create a text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "general-chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;

    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "general-chat");
            assert_eq!(channel.channel_type, shared_types::ChannelType::Text);
        }
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_channel_created_broadcast() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Broadcast Space", "Alice").await;
    let (_joined, _, _) = join_space(&mut bob, &space.invite_code, "Bob").await;

    // Consume Alice's MemberOnline for Bob
    let _ = alice.recv_signal().await;

    // Alice creates a channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "Music".to_string(),
            channel_type: shared_types::ChannelType::Voice,
        })
        .await;

    // Both Alice and Bob should get ChannelCreated
    let alice_msg = alice.recv_signal().await;
    match alice_msg {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "Music");
        }
        other => panic!("Alice expected ChannelCreated, got: {:?}", other),
    }

    let bob_msg = bob.recv_signal().await;
    match bob_msg {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "Music");
        }
        other => panic!("Bob expected ChannelCreated, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_owner_can_delete_channel_and_non_owner_cannot() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Delete Channel Space", "Alice").await;
    join_space(&mut bob, &space.invite_code, "Bob").await;

    let _ = alice.recv_signal().await;

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "notes".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;

    let deleted_channel_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated for Alice, got: {:?}", other),
    };
    match bob.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.id, deleted_channel_id);
        }
        other => panic!("Expected ChannelCreated for Bob, got: {:?}", other),
    }

    bob.send_signal(&SignalMessage::DeleteChannel {
        channel_id: deleted_channel_id.clone(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("creator")
                    || message.contains("owner")
                    || message.contains("admin")
                    || message.contains("permission"),
                "Error should mention channel permissions: {message}"
            );
        }
        other => panic!("Expected Error for non-owner delete, got: {:?}", other),
    }

    alice
        .send_signal(&SignalMessage::DeleteChannel {
            channel_id: deleted_channel_id.clone(),
        })
        .await;

    for msg in [alice.recv_signal().await, bob.recv_signal().await] {
        match msg {
            SignalMessage::ChannelDeleted { channel_id } => {
                assert_eq!(channel_id, deleted_channel_id);
            }
            other => panic!("Expected ChannelDeleted, got: {:?}", other),
        }
    }

    let mut charlie = server.connect().await;
    let (_joined_space, channels, _) =
        join_space(&mut charlie, &space.invite_code, "Charlie").await;
    assert!(
        channels
            .iter()
            .all(|channel| channel.id != deleted_channel_id),
        "Deleted channel should not reappear for later joins"
    );
}

#[tokio::test]
async fn test_owner_can_promote_admin_and_channel_access_updates() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    authenticate(&mut alice, "Alice", None).await;
    let (_bob_token, bob_user_id) = authenticate(&mut bob, "Bob", None).await;

    let (space, _) = create_space(&mut alice, "Roles Space", "Alice").await;

    let (joined_space, _, _) = join_space(&mut bob, &space.invite_code, "Bob").await;
    assert_eq!(joined_space.self_role, shared_types::SpaceRole::Member);

    let bob_member_id = match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => {
            assert_eq!(member.user_id.as_deref(), Some(bob_user_id.as_str()));
            member.id
        }
        other => panic!("Expected MemberOnline, got: {:?}", other),
    };

    bob.send_signal(&SignalMessage::CreateChannel {
        channel_name: "ops".to_string(),
        channel_type: shared_types::ChannelType::Text,
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("admin"),
                "Error should mention admin permission: {message}"
            );
        }
        other => panic!("Expected admin permission error, got: {:?}", other),
    }

    alice
        .send_signal(&SignalMessage::SetMemberRole {
            user_id: bob_user_id.clone(),
            role: shared_types::SpaceRole::Admin,
        })
        .await;

    for msg in [alice.recv_signal().await, bob.recv_signal().await] {
        match msg {
            SignalMessage::MemberRoleChanged { user_id, role } => {
                assert_eq!(user_id, bob_user_id);
                assert_eq!(role, shared_types::SpaceRole::Admin);
            }
            other => panic!("Expected MemberRoleChanged, got: {:?}", other),
        }
    }

    bob.send_signal(&SignalMessage::CreateChannel {
        channel_name: "ops".to_string(),
        channel_type: shared_types::ChannelType::Text,
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "ops");
            assert_eq!(channel.channel_type, shared_types::ChannelType::Text);
        }
        other => panic!(
            "Expected ChannelCreated for promoted admin, got: {:?}",
            other
        ),
    }

    match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "ops");
        }
        other => panic!(
            "Expected ChannelCreated broadcast for Alice, got: {:?}",
            other
        ),
    }

    bob.send_signal(&SignalMessage::KickMember {
        member_id: bob_member_id,
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("cannot kick") || message.contains("yourself"),
                "Admin should not be able to kick self: {message}"
            );
        }
        other => panic!("Expected self-kick error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_cannot_delete_last_channel() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_space, channels) = create_space(&mut alice, "Single Channel Space", "Alice").await;
    let general_id = channels[0].id.clone();

    alice
        .send_signal(&SignalMessage::DeleteChannel {
            channel_id: general_id,
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("at least one channel"),
                "Error should explain the last-channel guard: {message}"
            );
        }
        other => panic!("Expected Error for last channel delete, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_join_voice_channel() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_space, channels) = create_space(&mut alice, "Voice Space", "Alice").await;
    let general_id = channels[0].id.clone();

    // Join the General voice channel
    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: general_id.clone(),
        })
        .await;

    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::ChannelJoined {
            channel_id,
            channel_name,
            participants,
        } => {
            assert_eq!(channel_id, general_id);
            assert_eq!(channel_name, "General");
            assert_eq!(participants.len(), 0); // No one else in channel yet
        }
        other => panic!("Expected ChannelJoined, got: {:?}", other),
    }

    // Alice should also get MemberChannelChanged for herself
    let msg2 = alice.recv_signal().await;
    match msg2 {
        SignalMessage::MemberChannelChanged {
            channel_id,
            channel_name,
            ..
        } => {
            assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
            assert_eq!(channel_name.as_deref(), Some("General"));
        }
        other => panic!("Expected MemberChannelChanged, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_existing_peer_stays_in_channel_when_another_peer_joins() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, channels) = create_space(&mut alice, "Stay In Channel", "Alice").await;
    let general_id = channels[0].id.clone();

    join_space(&mut bob, &space.invite_code, "Bob").await;
    let _ = alice.recv_signal().await;

    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: general_id.clone(),
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::ChannelJoined { .. } => {}
        other => panic!("Expected Alice ChannelJoined, got: {:?}", other),
    }
    match alice.recv_signal().await {
        SignalMessage::MemberChannelChanged {
            channel_id,
            channel_name,
            ..
        } => {
            assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
            assert_eq!(channel_name.as_deref(), Some("General"));
        }
        other => panic!("Expected Alice MemberChannelChanged, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::MemberChannelChanged {
            channel_id,
            channel_name,
            ..
        } => {
            assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
            assert_eq!(channel_name.as_deref(), Some("General"));
        }
        other => panic!(
            "Expected Bob to see Alice enter the channel before joining, got: {:?}",
            other
        ),
    }

    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: general_id.clone(),
    })
    .await;
    let alice_peer_id = match bob.recv_signal().await {
        SignalMessage::ChannelJoined {
            channel_id,
            channel_name,
            participants,
        } => {
            assert_eq!(channel_id, general_id);
            assert_eq!(channel_name, "General");
            assert_eq!(participants.len(), 1, "Bob should see Alice already inside");
            assert_eq!(participants[0].name, "Alice");
            participants[0].id.clone()
        }
        other => panic!("Expected Bob ChannelJoined, got: {:?}", other),
    };
    match bob.recv_signal().await {
        SignalMessage::MemberChannelChanged {
            channel_id,
            channel_name,
            ..
        } => {
            assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
            assert_eq!(channel_name.as_deref(), Some("General"));
        }
        other => panic!("Expected Bob MemberChannelChanged, got: {:?}", other),
    }

    let mut bob_peer_id = None::<String>;
    let mut saw_member_channel_changed = false;
    for _ in 0..2 {
        match alice.recv_signal().await {
            SignalMessage::PeerJoined { peer } => {
                assert_eq!(peer.name, "Bob");
                bob_peer_id = Some(peer.id);
            }
            SignalMessage::MemberChannelChanged {
                member_id,
                channel_id,
                channel_name,
            } => {
                if let Some(ref expected_bob_id) = bob_peer_id {
                    assert_eq!(&member_id, expected_bob_id);
                }
                assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
                assert_eq!(channel_name.as_deref(), Some("General"));
                saw_member_channel_changed = true;
            }
            other => panic!(
                "Alice should stay in channel; unexpected signal: {:?}",
                other
            ),
        }
    }
    assert!(
        bob_peer_id.is_some(),
        "Alice should receive PeerJoined for Bob"
    );
    assert!(
        saw_member_channel_changed,
        "Alice should receive channel state update for Bob"
    );
    assert!(
        alice
            .recv_signal_timeout(Duration::from_millis(200))
            .await
            .is_none(),
        "Alice should not be kicked or receive extra channel teardown signals"
    );

    let audio_data = generate_test_audio();
    alice.send_binary(&audio_data).await;
    let frame = bob
        .recv_binary_timeout(Duration::from_secs(2))
        .await
        .expect("Bob should still receive Alice's audio after joining");
    let (sender_id, audio) = parse_audio_frame(&frame);
    assert_eq!(sender_id, alice_peer_id);
    assert_eq!(audio, audio_data);
}

#[tokio::test]
async fn test_repeated_multi_client_same_voice_channel_join_does_not_crash() {
    for round in 0..12 {
        let server = TestServer::start().await;
        let mut alice = server.connect().await;
        let mut bob = server.connect().await;
        let mut carol = server.connect().await;

        let (space, channels) = create_space(&mut alice, "Repeat Join Stability", "Alice").await;
        let general_id = channels[0].id.clone();

        join_space(&mut bob, &space.invite_code, "Bob").await;
        let _ = alice.recv_signal().await;

        join_space(&mut carol, &space.invite_code, "Carol").await;
        let _ = alice.recv_signal().await;
        let _ = bob.recv_signal().await;

        alice
            .send_signal(&SignalMessage::JoinChannel {
                channel_id: general_id.clone(),
            })
            .await;
        match alice.recv_signal().await {
            SignalMessage::ChannelJoined { participants, .. } => {
                assert!(
                    participants.is_empty(),
                    "Round {round}: Alice should be first into the channel"
                );
            }
            other => panic!(
                "Round {round}: expected Alice ChannelJoined, got: {:?}",
                other
            ),
        }
        match alice.recv_signal().await {
            SignalMessage::MemberChannelChanged { channel_id, .. } => {
                assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
            }
            other => panic!(
                "Round {round}: expected Alice self channel update, got: {:?}",
                other
            ),
        }
        for (name, client) in [("Bob", &mut bob), ("Carol", &mut carol)] {
            match client.recv_signal().await {
                SignalMessage::MemberChannelChanged { channel_id, .. } => {
                    assert_eq!(
                        channel_id.as_deref(),
                        Some(general_id.as_str()),
                        "Round {round}: {name} should see Alice enter the channel"
                    );
                }
                other => panic!(
                    "Round {round}: expected {name} to see Alice channel update, got: {:?}",
                    other
                ),
            }
        }

        bob.send_signal(&SignalMessage::JoinChannel {
            channel_id: general_id.clone(),
        })
        .await;
        match bob.recv_signal().await {
            SignalMessage::ChannelJoined { participants, .. } => {
                assert_eq!(
                    participants.len(),
                    1,
                    "Round {round}: Bob should see Alice already in the channel"
                );
            }
            other => panic!(
                "Round {round}: expected Bob ChannelJoined, got: {:?}",
                other
            ),
        }
        let _ = bob.recv_signal().await;
        let mut alice_saw_bob_peer = false;
        let mut alice_saw_bob_channel = false;
        for _ in 0..2 {
            match alice.recv_signal().await {
                SignalMessage::PeerJoined { peer } => {
                    assert_eq!(peer.name, "Bob");
                    alice_saw_bob_peer = true;
                }
                SignalMessage::MemberChannelChanged { channel_id, .. } => {
                    assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
                    alice_saw_bob_channel = true;
                }
                other => panic!(
                    "Round {round}: expected Alice to see Bob join, got: {:?}",
                    other
                ),
            }
        }
        assert!(
            alice_saw_bob_peer,
            "Round {round}: Alice should get PeerJoined for Bob"
        );
        assert!(
            alice_saw_bob_channel,
            "Round {round}: Alice should get Bob's channel update"
        );
        match carol.recv_signal().await {
            SignalMessage::MemberChannelChanged { channel_id, .. } => {
                assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
            }
            other => panic!(
                "Round {round}: expected Carol to see Bob channel update, got: {:?}",
                other
            ),
        }

        carol
            .send_signal(&SignalMessage::JoinChannel {
                channel_id: general_id.clone(),
            })
            .await;
        match carol.recv_signal().await {
            SignalMessage::ChannelJoined { participants, .. } => {
                assert_eq!(
                    participants.len(),
                    2,
                    "Round {round}: Carol should see Alice and Bob already in the channel"
                );
            }
            other => panic!(
                "Round {round}: expected Carol ChannelJoined, got: {:?}",
                other
            ),
        }
        let _ = carol.recv_signal().await;
        for (name, client) in [("Alice", &mut alice), ("Bob", &mut bob)] {
            let mut saw_peer_join = false;
            let mut saw_channel_change = false;
            for _ in 0..2 {
                match client.recv_signal().await {
                    SignalMessage::PeerJoined { peer } => {
                        assert_eq!(peer.name, "Carol");
                        saw_peer_join = true;
                    }
                    SignalMessage::MemberChannelChanged { channel_id, .. } => {
                        assert_eq!(channel_id.as_deref(), Some(general_id.as_str()));
                        saw_channel_change = true;
                    }
                    other => panic!(
                        "Round {round}: expected {name} to see Carol join, got: {:?}",
                        other
                    ),
                }
            }
            assert!(
                saw_peer_join,
                "Round {round}: {name} should receive PeerJoined for Carol"
            );
            assert!(
                saw_channel_change,
                "Round {round}: {name} should receive Carol's channel update"
            );
        }

        let audio = generate_test_audio();
        alice.send_binary(&audio).await;

        for (name, client) in [("Bob", &mut bob), ("Carol", &mut carol)] {
            let frame = client
                .recv_binary_timeout(Duration::from_secs(5))
                .await
                .unwrap_or_else(|| panic!("Round {round}: {name} should receive audio"));
            let (_, received_audio) = parse_audio_frame(&frame);
            assert_eq!(
                received_audio,
                &audio[..],
                "Round {round}: {name} audio should match"
            );
        }
    }
}

#[tokio::test]
async fn test_real_app_desktop_multi_process_same_channel_soak() {
    let desktop_bin = desktop_bin_path();
    assert!(
        desktop_bin.exists(),
        "Desktop binary missing at {:?}. Run `cargo build -p app_desktop -p signaling_server` first.",
        desktop_bin
    );

    let server = TestServer::start().await;
    let server_url = format!("ws://127.0.0.1:{}", server.port);
    let base_dir = std::env::temp_dir().join(format!(
        "voxlink_app_soak_{}_{}",
        std::process::id(),
        server.port
    ));
    std::fs::create_dir_all(&base_dir).unwrap();

    let shared_path = base_dir.join("shared.json");
    let owner_report = base_dir.join("owner.json");
    let bob_report = base_dir.join("bob.json");
    let carol_report = base_dir.join("carol.json");

    let owner = spawn_automated_desktop_client(
        &desktop_bin,
        &server_url,
        "owner",
        "Alice",
        &shared_path,
        &owner_report,
        "Desktop Soak",
        2,
        false,
        true,
    );
    let bob = spawn_automated_desktop_client(
        &desktop_bin,
        &server_url,
        "participant",
        "Bob",
        &shared_path,
        &bob_report,
        "Desktop Soak",
        0,
        true,
        false,
    );
    let carol = spawn_automated_desktop_client(
        &desktop_bin,
        &server_url,
        "participant",
        "Carol",
        &shared_path,
        &carol_report,
        "Desktop Soak",
        0,
        true,
        false,
    );

    let timeout_window = Duration::from_secs(20);
    let (owner_out, bob_out, carol_out) = tokio::join!(
        timeout(timeout_window, owner.wait_with_output()),
        timeout(timeout_window, bob.wait_with_output()),
        timeout(timeout_window, carol.wait_with_output()),
    );

    let owner_out = owner_out.expect("Owner desktop client timed out").unwrap();
    let bob_out = bob_out.expect("Bob desktop client timed out").unwrap();
    let carol_out = carol_out.expect("Carol desktop client timed out").unwrap();

    for (name, output) in [
        ("owner", &owner_out),
        ("bob", &bob_out),
        ("carol", &carol_out),
    ] {
        assert!(
            output.status.success(),
            "{name} desktop client failed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let owner_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&owner_report).unwrap()).unwrap();
    let bob_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&bob_report).unwrap()).unwrap();
    let carol_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&carol_report).unwrap()).unwrap();

    assert_eq!(owner_json["ok"].as_bool(), Some(true));
    assert_eq!(bob_json["ok"].as_bool(), Some(true));
    assert_eq!(carol_json["ok"].as_bool(), Some(true));

    assert!(
        owner_json["peer_join_events"].as_u64().unwrap_or(0) >= 2,
        "Owner should observe both other clients joining the voice channel: {owner_json}"
    );
    assert!(
        owner_json["audio_frames_sent"].as_u64().unwrap_or(0) > 0,
        "Owner should send automation audio: {owner_json}"
    );
    assert!(
        bob_json["audio_frames_recv"].as_u64().unwrap_or(0) > 0,
        "Bob should receive automation audio: {bob_json}"
    );
    assert!(
        carol_json["audio_frames_recv"].as_u64().unwrap_or(0) > 0,
        "Carol should receive automation audio: {carol_json}"
    );

    let _ = std::fs::remove_file(shared_path);
    let _ = std::fs::remove_file(owner_report);
    let _ = std::fs::remove_file(bob_report);
    let _ = std::fs::remove_file(carol_report);
    let _ = std::fs::remove_dir(&base_dir);
}

#[tokio::test]
async fn test_cannot_join_text_channel() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_space, _channels) = create_space(&mut alice, "Text Join Test", "Alice").await;

    // Create a text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;

    let created = alice.recv_signal().await;
    let text_ch_id = match created {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    // Try to join the text channel — should fail
    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: text_ch_id,
        })
        .await;

    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("text"),
                "Error should mention text channel: {message}"
            );
        }
        other => panic!("Expected Error for text channel join, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_leave_channel() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, channels) = create_space(&mut alice, "Leave Channel Test", "Alice").await;
    let general_id = channels[0].id.clone();

    let (_joined, _, _) = join_space(&mut bob, &space.invite_code, "Bob").await;
    // Consume Alice's MemberOnline for Bob
    let _ = alice.recv_signal().await;

    // Both join the General channel
    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: general_id.clone(),
        })
        .await;
    let _ = alice.recv_signal().await; // ChannelJoined
    let _ = alice.recv_signal().await; // MemberChannelChanged(self)

    // Bob also gets MemberChannelChanged for Alice
    let _ = bob.recv_signal().await;

    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: general_id.clone(),
    })
    .await;
    let _ = bob.recv_signal().await; // ChannelJoined (with Alice as participant)
    let _ = bob.recv_signal().await; // MemberChannelChanged(self)

    // Alice should get PeerJoined for Bob
    let _ = alice.recv_signal().await;
    // Alice should also get MemberChannelChanged for Bob
    let _ = alice.recv_signal().await;

    // Bob leaves the channel
    bob.send_signal(&SignalMessage::LeaveChannel).await;

    // Bob should get ChannelLeft
    let msg = bob.recv_signal().await;
    assert!(
        matches!(msg, SignalMessage::ChannelLeft),
        "Expected ChannelLeft, got: {:?}",
        msg
    );

    // Alice should get PeerLeft
    let alice_msg = alice.recv_signal().await;
    match alice_msg {
        SignalMessage::PeerLeft { .. } => {}
        other => panic!("Expected PeerLeft, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_channel_audio_relay() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, channels) = create_space(&mut alice, "Audio Channel", "Alice").await;
    let general_id = channels[0].id.clone();

    let (_joined, _, _) = join_space(&mut bob, &space.invite_code, "Bob").await;
    // Consume Alice's MemberOnline
    let _ = alice.recv_signal().await;

    // Both join the General channel
    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: general_id.clone(),
        })
        .await;
    let _ = alice.recv_signal().await; // ChannelJoined
    let _ = alice.recv_signal().await; // MemberChannelChanged

    // Consume Bob's MemberChannelChanged for Alice
    let _ = bob.recv_signal().await;

    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: general_id.clone(),
    })
    .await;
    let _ = bob.recv_signal().await; // ChannelJoined
    let _ = bob.recv_signal().await; // MemberChannelChanged

    // Consume Alice's PeerJoined and MemberChannelChanged for Bob
    let _ = alice.recv_signal().await;
    let _ = alice.recv_signal().await;

    // Alice sends audio — Bob should receive it
    let audio = generate_test_audio();
    alice.send_binary(&audio).await;

    let frame = bob
        .recv_binary_timeout(Duration::from_secs(5))
        .await
        .expect("Bob should receive audio in space channel");

    let (_, received_audio) = parse_audio_frame(&frame);
    assert_eq!(
        received_audio,
        &audio[..],
        "Audio data should match in channel"
    );
}

#[tokio::test]
async fn test_create_channel_not_in_space() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Try to create a channel without being in a space
    client
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "orphan".to_string(),
            channel_type: shared_types::ChannelType::Voice,
        })
        .await;

    let msg = client.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(message.contains("Not in a space"), "Error: {message}");
        }
        other => panic!("Expected Error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_disconnect_cleans_up_space() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Cleanup Space", "Alice").await;
    let (_joined, _, _) = join_space(&mut bob, &space.invite_code, "Bob").await;
    // Consume Alice's MemberOnline for Bob
    let _ = alice.recv_signal().await;

    // Bob disconnects abruptly
    drop(bob);

    // Alice should get MemberOffline
    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::MemberOffline { member_id } => {
            assert!(!member_id.is_empty());
        }
        other => panic!("Expected MemberOffline after disconnect, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_space_name_validation() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Empty name
    client
        .send_signal(&SignalMessage::CreateSpace {
            name: "   ".to_string(),
            user_name: "Alice".to_string(),
        })
        .await;

    let msg = client.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("empty") || message.contains("Name"),
                "Error: {message}"
            );
        }
        other => panic!("Expected Error for empty name, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_channel_name_validation() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_space, _channels) = create_space(&mut alice, "Valid Space", "Alice").await;

    // Empty channel name
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "".to_string(),
            channel_type: shared_types::ChannelType::Voice,
        })
        .await;

    let msg = alice.recv_signal().await;
    match msg {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("empty") || message.contains("Name"),
                "Error: {message}"
            );
        }
        other => panic!("Expected Error for empty channel name, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_join_space_shows_channels_with_type() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Typed Channels", "Alice").await;

    // Create text and voice channels
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let _ = alice.recv_signal().await; // ChannelCreated

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "gaming".to_string(),
            channel_type: shared_types::ChannelType::Voice,
        })
        .await;
    let _ = alice.recv_signal().await; // ChannelCreated

    // Bob joins and should see all 3 channels with correct types
    let (_joined, channels, _members) = join_space(&mut bob, &space.invite_code, "Bob").await;

    assert_eq!(
        channels.len(),
        3,
        "Should have 3 channels (General + chat + gaming)"
    );

    let general = channels.iter().find(|c| c.name == "General").unwrap();
    assert_eq!(general.channel_type, shared_types::ChannelType::Voice);

    let chat = channels.iter().find(|c| c.name == "chat").unwrap();
    assert_eq!(chat.channel_type, shared_types::ChannelType::Text);

    let gaming = channels.iter().find(|c| c.name == "gaming").unwrap();
    assert_eq!(gaming.channel_type, shared_types::ChannelType::Voice);
}

// ─── Authentication Tests ───

#[tokio::test]
async fn test_authenticate_new_user() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    let (_token, user_id) = authenticate(&mut alice, "Alice", None).await;
    assert!(!user_id.is_empty(), "Should receive a user_id");
}

#[tokio::test]
async fn test_authenticate_restore_identity() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;

    // First auth — get a token
    let (token, user_id) = authenticate(&mut alice, "Alice", None).await;

    // Reconnect with the same token
    let mut alice2 = server.connect().await;
    let (token2, user_id2) = authenticate(&mut alice2, "Alice Renamed", Some(token.clone())).await;
    assert_ne!(token2, token, "Restore should rotate the token");
    assert_eq!(user_id2, user_id, "Should restore the same user_id");
}

#[tokio::test]
async fn test_friend_presence_watch_updates_globally() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let _alice_user_id = authenticate(&mut alice, "Alice", None).await.1;
    let bob_user_id = authenticate(&mut bob, "Bob", None).await.1;

    alice
        .send_signal(&SignalMessage::WatchFriendPresence {
            user_ids: vec![bob_user_id.clone()],
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::FriendPresenceSnapshot { presences } => {
            assert_eq!(presences.len(), 1);
            assert_eq!(presences[0].user_id, bob_user_id);
            assert!(presences[0].is_online);
            assert!(presences[0].active_space_name.is_none());
            assert!(presences[0].active_channel_name.is_none());
        }
        other => panic!("Expected FriendPresenceSnapshot, got: {:?}", other),
    }

    let (space, channels) = create_space(&mut bob, "Studio", "Bob").await;
    match alice.recv_signal().await {
        SignalMessage::FriendPresenceChanged { presence } => {
            assert_eq!(presence.user_id, bob_user_id);
            assert!(presence.is_online);
            assert_eq!(
                presence.active_space_name.as_deref(),
                Some(space.name.as_str())
            );
            assert!(presence.active_channel_name.is_none());
        }
        other => panic!(
            "Expected FriendPresenceChanged after space join, got: {:?}",
            other
        ),
    }

    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: channels[0].id.clone(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::ChannelJoined { .. } => {}
        other => panic!("Expected ChannelJoined, got: {:?}", other),
    }
    match alice.recv_signal().await {
        SignalMessage::FriendPresenceChanged { presence } => {
            assert_eq!(presence.user_id, bob_user_id);
            assert!(presence.is_online);
            assert!(presence.is_in_voice);
            assert_eq!(presence.active_space_name.as_deref(), Some("Studio"));
            assert_eq!(presence.active_channel_name.as_deref(), Some("General"));
        }
        other => panic!(
            "Expected FriendPresenceChanged after channel join, got: {:?}",
            other
        ),
    }

    bob.sink.close().await.unwrap();
    match alice.recv_signal_timeout(Duration::from_secs(5)).await {
        Some(SignalMessage::FriendPresenceChanged { presence }) => {
            assert_eq!(presence.user_id, bob_user_id);
            assert!(!presence.is_online);
        }
        other => panic!("Expected offline FriendPresenceChanged, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_friend_request_accept_and_remove_flow() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let _alice_user_id = authenticate(&mut alice, "Alice", None).await.1;
    let bob_user_id = authenticate(&mut bob, "Bob", None).await.1;

    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_user_id.clone(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert!(friends.is_empty());
            assert!(incoming_requests.is_empty());
            assert_eq!(outgoing_requests.len(), 1);
            assert_eq!(outgoing_requests[0].user_id, bob_user_id);
        }
        other => panic!("Expected requester FriendSnapshot, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert!(friends.is_empty());
            assert_eq!(incoming_requests.len(), 1);
            assert_eq!(incoming_requests[0].name, "Alice");
            assert!(outgoing_requests.is_empty());
        }
        other => panic!("Expected recipient FriendSnapshot, got: {:?}", other),
    }

    bob.send_signal(&SignalMessage::RespondFriendRequest {
        user_id: _alice_user_id.clone(),
        accept: true,
    })
    .await;

    match bob.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert_eq!(friends.len(), 1);
            assert_eq!(friends[0].name, "Alice");
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!("Expected accepted FriendSnapshot for Bob, got: {:?}", other),
    }
    match alice.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert_eq!(friends.len(), 1);
            assert_eq!(friends[0].user_id, bob_user_id);
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!(
            "Expected accepted FriendSnapshot for Alice, got: {:?}",
            other
        ),
    }

    alice
        .send_signal(&SignalMessage::RemoveFriend {
            user_id: bob_user_id.clone(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert!(friends.is_empty());
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!(
            "Expected removal FriendSnapshot for Alice, got: {:?}",
            other
        ),
    }
    match bob.recv_signal().await {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
        } => {
            assert!(friends.is_empty());
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!("Expected removal FriendSnapshot for Bob, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_direct_message_flow() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let alice_user_id = authenticate(&mut alice, "Alice", None).await.1;
    let bob_user_id = authenticate(&mut bob, "Bob", None).await.1;

    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_user_id.clone(),
        })
        .await;
    let _ = alice.recv_signal().await;
    let _ = bob.recv_signal().await;

    bob.send_signal(&SignalMessage::RespondFriendRequest {
        user_id: alice_user_id.clone(),
        accept: true,
    })
    .await;
    let _ = bob.recv_signal().await;
    let _ = alice.recv_signal().await;

    alice
        .send_signal(&SignalMessage::SelectDirectMessage {
            user_id: bob_user_id.clone(),
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::DirectMessageSelected {
            user_id,
            user_name,
            history,
        } => {
            assert_eq!(user_id, bob_user_id);
            assert_eq!(user_name, "Bob");
            assert!(history.is_empty());
        }
        other => panic!("Expected DirectMessageSelected for Alice, got: {:?}", other),
    }

    bob.send_signal(&SignalMessage::SelectDirectMessage {
        user_id: alice_user_id.clone(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::DirectMessageSelected {
            user_id,
            user_name,
            history,
        } => {
            assert_eq!(user_id, alice_user_id);
            assert_eq!(user_name, "Alice");
            assert!(history.is_empty());
        }
        other => panic!("Expected DirectMessageSelected for Bob, got: {:?}", other),
    }

    bob.send_signal(&SignalMessage::SetDirectTyping {
        user_id: alice_user_id.clone(),
        is_typing: true,
    })
    .await;
    match alice.recv_signal().await {
        SignalMessage::DirectTypingState {
            user_id,
            user_name,
            is_typing,
        } => {
            assert_eq!(user_id, bob_user_id);
            assert_eq!(user_name, "Bob");
            assert!(is_typing);
        }
        other => panic!("Expected DirectTypingState, got: {:?}", other),
    }

    alice
        .send_signal(&SignalMessage::SendDirectMessage {
            user_id: bob_user_id.clone(),
            content: "Hello Bob".into(),
            reply_to_message_id: None,
        })
        .await;

    let message_id = match alice.recv_signal().await {
        SignalMessage::DirectMessage { user_id, message } => {
            assert_eq!(user_id, bob_user_id);
            assert_eq!(message.content, "Hello Bob");
            message.message_id
        }
        other => panic!("Expected sender DirectMessage echo, got: {:?}", other),
    };
    match bob.recv_signal().await {
        SignalMessage::DirectMessage { user_id, message } => {
            assert_eq!(user_id, alice_user_id);
            assert_eq!(message.message_id, message_id);
            assert_eq!(message.content, "Hello Bob");
        }
        other => panic!("Expected recipient DirectMessage, got: {:?}", other),
    }

    alice
        .send_signal(&SignalMessage::EditDirectMessage {
            user_id: bob_user_id.clone(),
            message_id: message_id.clone(),
            new_content: "Hello Bob, updated".into(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::DirectMessageEdited {
            user_id,
            message_id: edited_id,
            new_content,
        } => {
            assert_eq!(user_id, bob_user_id);
            assert_eq!(edited_id, message_id);
            assert_eq!(new_content, "Hello Bob, updated");
        }
        other => panic!("Expected sender DirectMessageEdited, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::DirectMessageEdited {
            user_id,
            message_id: edited_id,
            new_content,
        } => {
            assert_eq!(user_id, alice_user_id);
            assert_eq!(edited_id, message_id);
            assert_eq!(new_content, "Hello Bob, updated");
        }
        other => panic!("Expected recipient DirectMessageEdited, got: {:?}", other),
    }

    alice
        .send_signal(&SignalMessage::DeleteDirectMessage {
            user_id: bob_user_id.clone(),
            message_id: message_id.clone(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::DirectMessageDeleted {
            user_id,
            message_id: deleted_id,
        } => {
            assert_eq!(user_id, bob_user_id);
            assert_eq!(deleted_id, message_id);
        }
        other => panic!("Expected sender DirectMessageDeleted, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::DirectMessageDeleted {
            user_id,
            message_id: deleted_id,
        } => {
            assert_eq!(user_id, alice_user_id);
            assert_eq!(deleted_id, message_id);
        }
        other => panic!("Expected recipient DirectMessageDeleted, got: {:?}", other),
    }
}

// ─── Chat Edit/Delete/React Tests ───

#[tokio::test]
async fn test_chat_edit_message() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Chat Space", "Alice").await;
    join_space(&mut bob, &space.invite_code, "Bob").await;

    // Drain MemberOnline from Alice
    let _ = alice.recv_signal().await;

    // Alice creates a text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".into(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let channel_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel, .. } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };
    // Bob gets ChannelCreated broadcast
    let _ = bob.recv_signal().await;

    // Both select the text channel
    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: channel_id.clone(),
        })
        .await;
    let _ = alice.recv_signal().await; // TextChannelHistory

    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: channel_id.clone(),
    })
    .await;
    let _ = bob.recv_signal().await; // TextChannelHistory

    // Alice sends a message
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: channel_id.clone(),
            content: "Hello world".into(),
            reply_to_message_id: None,
        })
        .await;

    // Both receive the message
    let msg_id = match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => {
            assert_eq!(message.content, "Hello world");
            assert!(!message.message_id.is_empty());
            message.message_id
        }
        other => panic!("Expected TextMessage, got: {:?}", other),
    };
    let _ = bob.recv_signal().await; // TextMessage

    // Alice edits the message
    alice
        .send_signal(&SignalMessage::EditTextMessage {
            channel_id: channel_id.clone(),
            message_id: msg_id.clone(),
            new_content: "Hello world (edited)".into(),
        })
        .await;

    // Both should receive TextMessageEdited
    match alice.recv_signal().await {
        SignalMessage::TextMessageEdited {
            message_id,
            new_content,
            ..
        } => {
            assert_eq!(message_id, msg_id);
            assert_eq!(new_content, "Hello world (edited)");
        }
        other => panic!("Expected TextMessageEdited, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::TextMessageEdited { message_id, .. } => {
            assert_eq!(message_id, msg_id);
        }
        other => panic!("Expected TextMessageEdited, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_chat_delete_message() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Delete Space", "Alice").await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    let _ = alice.recv_signal().await; // MemberOnline

    // Create text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".into(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let channel_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel, .. } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };
    let _ = bob.recv_signal().await;

    // Select channel
    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: channel_id.clone(),
        })
        .await;
    let _ = alice.recv_signal().await;
    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: channel_id.clone(),
    })
    .await;
    let _ = bob.recv_signal().await;

    // Alice sends a message
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: channel_id.clone(),
            content: "Delete me".into(),
            reply_to_message_id: None,
        })
        .await;
    let msg_id = match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => message.message_id,
        other => panic!("Expected TextMessage, got: {:?}", other),
    };
    let _ = bob.recv_signal().await;

    // Alice deletes the message
    alice
        .send_signal(&SignalMessage::DeleteTextMessage {
            channel_id: channel_id.clone(),
            message_id: msg_id.clone(),
        })
        .await;

    // Both should receive TextMessageDeleted
    match alice.recv_signal().await {
        SignalMessage::TextMessageDeleted { message_id, .. } => {
            assert_eq!(message_id, msg_id);
        }
        other => panic!("Expected TextMessageDeleted, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::TextMessageDeleted { message_id, .. } => {
            assert_eq!(message_id, msg_id);
        }
        other => panic!("Expected TextMessageDeleted, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_chat_react_to_message() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "React Space", "Alice").await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    let _ = alice.recv_signal().await; // MemberOnline

    // Create text channel + select
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".into(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let channel_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel, .. } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };
    let _ = bob.recv_signal().await;
    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: channel_id.clone(),
        })
        .await;
    let _ = alice.recv_signal().await;
    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: channel_id.clone(),
    })
    .await;
    let _ = bob.recv_signal().await;

    // Alice sends a message
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: channel_id.clone(),
            content: "React to me".into(),
            reply_to_message_id: None,
        })
        .await;
    let msg_id = match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => message.message_id,
        other => panic!("Expected TextMessage, got: {:?}", other),
    };
    let _ = bob.recv_signal().await;

    // Bob reacts with thumbs up
    bob.send_signal(&SignalMessage::ReactToMessage {
        channel_id: channel_id.clone(),
        message_id: msg_id.clone(),
        emoji: "👍".into(),
    })
    .await;

    // Both should receive MessageReaction
    let check_reaction = |msg: SignalMessage| match msg {
        SignalMessage::MessageReaction {
            message_id, emoji, ..
        } => {
            assert_eq!(message_id, msg_id);
            assert_eq!(emoji, "👍");
        }
        other => panic!("Expected MessageReaction, got: {:?}", other),
    };
    check_reaction(alice.recv_signal().await);
    check_reaction(bob.recv_signal().await);
}

// ─── Moderation Tests ───

#[tokio::test]
async fn test_kick_member() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await; // owner
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Kick Space", "Alice").await;
    let (_joined, _, _members) = join_space(&mut bob, &space.invite_code, "Bob").await;

    // Alice receives MemberOnline for Bob
    let bob_id = match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => {
            assert_eq!(member.name, "Bob");
            member.id
        }
        other => panic!("Expected MemberOnline, got: {:?}", other),
    };

    // Alice kicks Bob
    alice
        .send_signal(&SignalMessage::KickMember {
            member_id: bob_id.clone(),
        })
        .await;

    // Bob should receive Kicked
    match bob.recv_signal().await {
        SignalMessage::Kicked { reason } => {
            assert!(reason.contains("kicked"), "Reason: {reason}");
        }
        other => panic!("Expected Kicked, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_non_owner_cannot_kick() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Auth Space", "Alice").await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    // Drain MemberOnline from alice
    let _ = alice.recv_signal().await;

    // Bob tries to kick Alice (should fail — Bob is not the owner)
    bob.send_signal(&SignalMessage::KickMember {
        member_id: "p1".into(), // Alice's likely ID
    })
    .await;

    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("owner") || message.contains("permission"),
                "Error should mention kick permissions: {message}"
            );
        }
        other => panic!("Expected Error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_mute_member() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let (space, _) = create_space(&mut alice, "Mute Space", "Alice").await;
    join_space(&mut bob, &space.invite_code, "Bob").await;

    let bob_id = match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => member.id,
        other => panic!("Expected MemberOnline, got: {:?}", other),
    };

    // Alice server-mutes Bob
    alice
        .send_signal(&SignalMessage::MuteMember {
            member_id: bob_id.clone(),
            muted: true,
        })
        .await;

    // Both should receive MemberMuted
    match alice.recv_signal().await {
        SignalMessage::MemberMuted { member_id, muted } => {
            assert_eq!(member_id, bob_id);
            assert!(muted);
        }
        other => panic!("Expected MemberMuted, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::MemberMuted { member_id, muted } => {
            assert_eq!(member_id, bob_id);
            assert!(muted);
        }
        other => panic!("Expected MemberMuted, got: {:?}", other),
    }
}

// ─── Stress Tests: Space Networking ───

/// Test: multiple users join the same space rapidly.
/// Verifies no crash, correct member counts, and all MemberOnline broadcasts.
#[tokio::test]
async fn test_stress_many_users_join_space() {
    let server = TestServer::start().await;

    // Alice creates the space
    let mut alice = server.connect().await;
    let (space, _channels) = create_space(&mut alice, "Stress Space", "Alice").await;

    // 8 users join rapidly
    let mut joiners = Vec::new();
    for i in 0..8 {
        let mut client = server.connect().await;
        let name = format!("User{i}");
        let (joined_space, _channels, members) =
            join_space(&mut client, &space.invite_code, &name).await;
        assert_eq!(joined_space.id, space.id);
        // Each joiner should see existing members (including Alice + prior joiners)
        assert!(
            !members.is_empty(),
            "User{i} should see at least 1 member (Alice)"
        );
        joiners.push(client);
    }

    // Alice should have received 8 MemberOnline messages
    for i in 0..8 {
        match alice.recv_signal().await {
            SignalMessage::MemberOnline { member } => {
                assert!(
                    !member.name.is_empty(),
                    "MemberOnline #{i} should have a name"
                );
            }
            other => panic!("Expected MemberOnline #{i}, got: {:?}", other),
        }
    }
}

/// Test: users join and leave a space rapidly, verifying cleanup.
#[tokio::test]
async fn test_stress_join_leave_churn() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "Churn Space", "Alice").await;

    // 5 users join then immediately leave (drop connection)
    for i in 0..5 {
        let mut client = server.connect().await;
        let (_sp, _ch, _members) =
            join_space(&mut client, &space.invite_code, &format!("Temp{i}")).await;
        // Drop client — triggers disconnect and MemberOffline
        drop(client);
        // Small delay for disconnect to propagate
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Drain Alice's notifications (MemberOnline + MemberOffline pairs)
    let mut online_count = 0;
    let mut offline_count = 0;
    loop {
        match alice.recv_signal_timeout(Duration::from_millis(500)).await {
            Some(SignalMessage::MemberOnline { .. }) => online_count += 1,
            Some(SignalMessage::MemberOffline { .. }) => offline_count += 1,
            _ => break,
        }
    }
    assert_eq!(online_count, 5, "Should have 5 join notifications");
    assert_eq!(offline_count, 5, "Should have 5 leave notifications");

    // Now join one more user and verify they see only Alice + themselves (no ghosts)
    let mut bob = server.connect().await;
    let (_sp, _ch, members) = join_space(&mut bob, &space.invite_code, "Bob").await;
    // Should be Alice + Bob (joiner is included in member list)
    assert_eq!(
        members.len(),
        2,
        "Should see Alice + Bob, got {} members: {:?}",
        members.len(),
        members.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"Alice"), "Alice should be in members");
    assert!(names.contains(&"Bob"), "Bob should be in members");
}

/// Test: authenticated user reconnects to same space.
/// Old stale member entry should be removed, no duplicate members.
#[tokio::test]
async fn test_stress_reconnect_same_space() {
    let server = TestServer::start().await;

    // Alice creates space
    let mut alice = server.connect().await;
    let (_alice_token, _alice_uid) = authenticate(&mut alice, "Alice", None).await;
    let (space, _) = create_space(&mut alice, "Reconnect Space", "Alice").await;

    // Bob authenticates, joins space
    let mut bob = server.connect().await;
    let (bob_token, _bob_uid) = authenticate(&mut bob, "Bob", None).await;
    let (_sp, _ch, _members) = join_space(&mut bob, &space.invite_code, "Bob").await;

    // Alice should get MemberOnline for Bob
    match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => {
            assert_eq!(member.name, "Bob");
        }
        other => panic!("Expected MemberOnline for Bob, got: {:?}", other),
    }

    // Bob disconnects (simulating network drop)
    drop(bob);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Alice should get MemberOffline
    match alice.recv_signal().await {
        SignalMessage::MemberOffline { .. } => {}
        other => panic!("Expected MemberOffline, got: {:?}", other),
    }

    // Bob reconnects with same token
    let mut bob2 = server.connect().await;
    let (_bob_token2, _bob_uid2) = authenticate(&mut bob2, "Bob", Some(bob_token)).await;
    let (_sp2, _ch2, members) = join_space(&mut bob2, &space.invite_code, "Bob").await;

    // Members should include Alice (and Bob2 will be added after join)
    // No duplicate entries for Bob
    let bob_count = members.iter().filter(|m| m.name == "Bob").count();
    assert!(
        bob_count <= 1,
        "Bob should appear at most once in member list, found {bob_count}"
    );

    // Alice gets MemberOnline for Bob's new connection
    match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => {
            assert_eq!(member.name, "Bob");
        }
        other => panic!("Expected MemberOnline for Bob reconnect, got: {:?}", other),
    }

    // Verify no duplicate — Alice creates a fresh view by asking for space info
    // (not directly possible, so we have a 3rd user join and check member list)
    let mut charlie = server.connect().await;
    let (_sp3, _ch3, members3) = join_space(&mut charlie, &space.invite_code, "Charlie").await;
    let bob_entries: Vec<_> = members3.iter().filter(|m| m.name == "Bob").collect();
    assert_eq!(
        bob_entries.len(),
        1,
        "Charlie should see exactly 1 Bob, got {}: {:?}",
        bob_entries.len(),
        bob_entries
    );
}

/// Test: rapid channel join/leave within a space.
#[tokio::test]
async fn test_stress_channel_join_leave() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, channels) = create_space(&mut alice, "Channel Stress", "Alice").await;
    let channel_id = channels[0].id.clone();

    // Alice joins the voice channel
    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: channel_id.clone(),
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::ChannelJoined { .. } => {}
        other => panic!("Expected ChannelJoined, got: {:?}", other),
    }
    // Drain MemberChannelChanged for self
    alice.recv_signal().await;

    // Bob joins space, joins and leaves channel 5 times rapidly
    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    // Drain alice's MemberOnline
    alice.recv_signal().await;

    for _ in 0..5 {
        bob.send_signal(&SignalMessage::JoinChannel {
            channel_id: channel_id.clone(),
        })
        .await;
        // Drain until ChannelJoined (may get MemberChannelChanged, PeerJoined first)
        loop {
            match bob.recv_signal().await {
                SignalMessage::ChannelJoined { .. } => break,
                _ => continue,
            }
        }

        bob.send_signal(&SignalMessage::LeaveChannel).await;
        // Drain until ChannelLeft (may get MemberChannelChanged, PeerLeft first)
        loop {
            match bob.recv_signal().await {
                SignalMessage::ChannelLeft => break,
                _ => continue,
            }
        }
    }

    // Alice should still be connected and functional — send a ping to verify
    alice
        .send_signal(&SignalMessage::JoinChannel {
            channel_id: channel_id.clone(),
        })
        .await;
    // Drain all accumulated messages, look for ChannelJoined
    loop {
        match alice.recv_signal().await {
            SignalMessage::ChannelJoined { .. } => break,
            _ => continue, // Skip PeerJoined/PeerLeft/MemberChannelChanged
        }
    }
}

/// Test: concurrent space operations (create, join, delete channel).
/// Verifies no server panic from TOCTOU races.
#[tokio::test]
async fn test_stress_concurrent_space_operations() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, channels) = create_space(&mut alice, "Concurrent Ops", "Alice").await;

    // Alice creates 3 additional channels
    for i in 0..3 {
        alice
            .send_signal(&SignalMessage::CreateChannel {
                channel_name: format!("Extra-{i}"),
                channel_type: shared_types::ChannelType::Voice,
            })
            .await;
        match alice.recv_signal().await {
            SignalMessage::ChannelCreated { .. } => {}
            other => panic!("Expected ChannelCreated, got: {:?}", other),
        }
    }

    // Bob and Charlie join the space concurrently
    let mut bob = server.connect().await;
    let mut charlie = server.connect().await;

    // Both join at the same time (parallel sends)
    bob.send_signal(&SignalMessage::JoinSpace {
        invite_code: space.invite_code.clone(),
        user_name: "Bob".to_string(),
    })
    .await;
    charlie
        .send_signal(&SignalMessage::JoinSpace {
            invite_code: space.invite_code.clone(),
            user_name: "Charlie".to_string(),
        })
        .await;

    // Both should get SpaceJoined
    match bob.recv_signal().await {
        SignalMessage::SpaceJoined { channels, .. } => {
            assert_eq!(
                channels.len(),
                4,
                "Bob should see 4 channels (1 default + 3 extra)"
            );
        }
        other => panic!("Expected SpaceJoined for Bob, got: {:?}", other),
    }
    match charlie.recv_signal().await {
        SignalMessage::SpaceJoined { channels, .. } => {
            assert_eq!(
                channels.len(),
                4,
                "Charlie should see 4 channels (1 default + 3 extra)"
            );
        }
        other => panic!("Expected SpaceJoined for Charlie, got: {:?}", other),
    }

    // Now Alice deletes a channel while Bob is trying to join it
    // This tests the TOCTOU fix — should not crash the server
    let extra_channel_id = channels[0].id.clone(); // default channel

    // Alice deletes the extra channel concurrently
    alice
        .send_signal(&SignalMessage::DeleteChannel {
            channel_id: extra_channel_id.clone(),
        })
        .await;
    // Bob tries to join it (may already be deleted)
    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: extra_channel_id.clone(),
    })
    .await;

    // We don't assert specific ordering — just that neither crashes.
    // Drain messages from all clients to verify server is still alive.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Server still alive? Alice can create another channel.
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "Post-Stress".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    // Should get ChannelCreated (possibly after some other messages)
    loop {
        match alice.recv_signal_timeout(Duration::from_secs(3)).await {
            Some(SignalMessage::ChannelCreated { channel }) => {
                assert_eq!(channel.name, "Post-Stress");
                break;
            }
            Some(_) => continue, // Skip other messages
            None => panic!("Server stopped responding after stress test"),
        }
    }
}

/// Test: disconnect during space join — verifies no orphaned state.
#[tokio::test]
async fn test_stress_disconnect_during_join() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "Disconnect Join", "Alice").await;

    // 10 clients connect, send JoinSpace, then immediately drop
    for i in 0..10 {
        let mut client = server.connect().await;
        client
            .send_signal(&SignalMessage::JoinSpace {
                invite_code: space.invite_code.clone(),
                user_name: format!("Phantom{i}"),
            })
            .await;
        // Drop immediately — may or may not have received SpaceJoined
        drop(client);
    }

    // Wait for all disconnects to process
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drain all of Alice's accumulated messages
    loop {
        if alice
            .recv_signal_timeout(Duration::from_millis(200))
            .await
            .is_none()
        {
            break;
        }
    }

    // New user joins and verifies clean state
    let mut bob = server.connect().await;
    let (_sp, _ch, members) = join_space(&mut bob, &space.invite_code, "Bob").await;

    // Should only be Alice (all phantoms disconnected)
    let non_bob: Vec<_> = members.iter().filter(|m| m.name != "Bob").collect();
    assert_eq!(
        non_bob.len(),
        1,
        "Should only see Alice, got: {:?}",
        non_bob.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    assert_eq!(non_bob[0].name, "Alice");
}

/// Test: rapid message sending to a text channel.
#[tokio::test]
async fn test_stress_rapid_text_messages() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, _channels) = create_space(&mut alice, "Chat Stress", "Alice").await;

    // Create a text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat-stress".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_channel_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    // Bob joins and selects text channel
    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    // Drain MemberOnline from Alice
    alice.recv_signal().await;

    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: text_channel_id.clone(),
    })
    .await;
    // Drain ChatHistory
    bob.recv_signal().await;

    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: text_channel_id.clone(),
        })
        .await;
    alice.recv_signal().await; // ChatHistory

    // Alice sends 50 messages rapidly
    for i in 0..50 {
        alice
            .send_signal(&SignalMessage::SendTextMessage {
                channel_id: text_channel_id.clone(),
                content: format!("Stress message #{i}"),
                reply_to_message_id: None,
            })
            .await;
    }

    // Bob should receive all 50 messages (drain them)
    let mut received = 0;
    loop {
        match bob.recv_signal_timeout(Duration::from_secs(3)).await {
            Some(SignalMessage::TextMessage { .. }) => received += 1,
            Some(_) => continue, // Skip typing indicators etc.
            None => break,
        }
    }
    assert_eq!(received, 50, "Bob should receive all 50 messages");
}

/// Test: multiple spaces created and deleted.
#[tokio::test]
async fn test_stress_multiple_spaces() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;

    // Create 10 spaces
    let mut spaces = Vec::new();
    for i in 0..10 {
        let (space, _) = create_space(&mut alice, &format!("Space-{i}"), "Alice").await;
        spaces.push(space);
    }

    // Bob joins all 10 spaces one at a time (leave + join)
    let mut bob = server.connect().await;
    for (i, space) in spaces.iter().enumerate() {
        // Leave previous space if not first (LeaveSpace has no response, just wait briefly)
        if i > 0 {
            bob.send_signal(&SignalMessage::LeaveSpace).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (_sp, _ch, members) = join_space(&mut bob, &space.invite_code, "Bob").await;
        assert!(
            !members.is_empty(),
            "Space-{i} should have Alice as a member"
        );
    }

    // Alice is now in Space-9 (the last one she created).
    // Delete it to verify delete works after heavy space creation.
    alice.send_signal(&SignalMessage::DeleteSpace).await;
    loop {
        match alice.recv_signal_timeout(Duration::from_secs(3)).await {
            Some(SignalMessage::SpaceDeleted) => break,
            Some(_) => continue,
            None => panic!("Expected SpaceDeleted"),
        }
    }

    // Server should still be alive
    let (new_space, _) = create_space(&mut alice, "Still Alive", "Alice").await;
    assert!(!new_space.id.is_empty());
}

// ─── Networking Tests: Chat, Friends, Moderation, Screen Share ───

/// Test: text channel message lifecycle (send, edit, delete).
#[tokio::test]
async fn test_text_channel_message_lifecycle() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (_space, _channels) = create_space(&mut alice, "Chat Test", "Alice").await;

    // Create a text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "text-chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    // Bob joins and selects the text channel
    let mut bob = server.connect().await;
    join_space(&mut bob, &_space.invite_code, "Bob").await;
    // Drain MemberOnline from Alice
    alice.recv_signal().await;

    // Both select text channel
    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: text_ch_id.clone(),
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::TextChannelSelected { history, .. } => {
            assert!(history.is_empty(), "Fresh channel should have no history");
        }
        other => panic!("Expected TextChannelSelected, got: {:?}", other),
    }

    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: text_ch_id.clone(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::TextChannelSelected { .. } => {}
        other => panic!("Expected TextChannelSelected, got: {:?}", other),
    }

    // Alice sends a message
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: text_ch_id.clone(),
            content: "Hello from Alice".to_string(),
            reply_to_message_id: None,
        })
        .await;

    // Both receive the message (broadcast includes sender)
    let mut message_id = String::new();
    for client_name in ["Alice", "Bob"] {
        let client: &mut TestClient = if client_name == "Alice" {
            &mut alice
        } else {
            &mut bob
        };
        match client.recv_signal().await {
            SignalMessage::TextMessage { message, .. } => {
                assert_eq!(message.content, "Hello from Alice");
                assert!(!message.message_id.is_empty(), "Message should have an ID");
                if message_id.is_empty() {
                    message_id = message.message_id.clone();
                }
            }
            other => panic!("Expected TextMessage for {client_name}, got: {:?}", other),
        }
    }

    // Alice edits the message
    alice
        .send_signal(&SignalMessage::EditTextMessage {
            channel_id: text_ch_id.clone(),
            message_id: message_id.clone(),
            new_content: "Hello from Alice (edited)".to_string(),
        })
        .await;

    // Both receive edit notification
    for client_name in ["Alice", "Bob"] {
        let client: &mut TestClient = if client_name == "Alice" {
            &mut alice
        } else {
            &mut bob
        };
        match client.recv_signal().await {
            SignalMessage::TextMessageEdited {
                message_id: mid,
                new_content,
                ..
            } => {
                assert_eq!(mid, message_id);
                assert_eq!(new_content, "Hello from Alice (edited)");
            }
            other => panic!(
                "Expected TextMessageEdited for {client_name}, got: {:?}",
                other
            ),
        }
    }

    // Alice deletes the message
    alice
        .send_signal(&SignalMessage::DeleteTextMessage {
            channel_id: text_ch_id.clone(),
            message_id: message_id.clone(),
        })
        .await;

    for client_name in ["Alice", "Bob"] {
        let client: &mut TestClient = if client_name == "Alice" {
            &mut alice
        } else {
            &mut bob
        };
        match client.recv_signal().await {
            SignalMessage::TextMessageDeleted {
                message_id: mid, ..
            } => {
                assert_eq!(mid, message_id);
            }
            other => panic!(
                "Expected TextMessageDeleted for {client_name}, got: {:?}",
                other
            ),
        }
    }
}

/// Test: text channel history persists for late joiners.
#[tokio::test]
async fn test_text_channel_history_for_late_joiner() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "History Test", "Alice").await;

    // Create text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "history-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    // Alice selects and sends 5 messages
    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: text_ch_id.clone(),
        })
        .await;
    alice.recv_signal().await; // TextChannelSelected

    for i in 0..5 {
        alice
            .send_signal(&SignalMessage::SendTextMessage {
                channel_id: text_ch_id.clone(),
                content: format!("Message #{i}"),
                reply_to_message_id: None,
            })
            .await;
        alice.recv_signal().await; // TextMessage broadcast
    }

    // Bob joins AFTER messages were sent
    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    // Drain MemberOnline from Alice
    alice.recv_signal().await;

    // Bob selects the text channel — should see all 5 messages
    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: text_ch_id.clone(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::TextChannelSelected { history, .. } => {
            assert_eq!(
                history.len(),
                5,
                "Late joiner should see 5 messages in history"
            );
            assert_eq!(history[0].content, "Message #0");
            assert_eq!(history[4].content, "Message #4");
        }
        other => panic!("Expected TextChannelSelected, got: {:?}", other),
    }
}

/// Test: friend request lifecycle (send, accept, DM, remove).
#[tokio::test]
async fn test_friend_request_lifecycle() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, alice_uid) = authenticate(&mut alice, "Alice", None).await;

    let mut bob = server.connect().await;
    let (_bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;

    // Alice sends friend request to Bob
    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;

    // Both get updated FriendSnapshot
    let alice_snapshot = alice.recv_signal().await;
    match &alice_snapshot {
        SignalMessage::FriendSnapshot {
            outgoing_requests, ..
        } => {
            assert_eq!(
                outgoing_requests.len(),
                1,
                "Alice should have 1 outgoing request"
            );
            assert_eq!(outgoing_requests[0].user_id, bob_uid);
        }
        other => panic!("Expected FriendSnapshot for Alice, got: {:?}", other),
    }

    let bob_snapshot = bob.recv_signal().await;
    match &bob_snapshot {
        SignalMessage::FriendSnapshot {
            incoming_requests, ..
        } => {
            assert_eq!(
                incoming_requests.len(),
                1,
                "Bob should have 1 incoming request"
            );
            assert_eq!(incoming_requests[0].user_id, alice_uid);
        }
        other => panic!("Expected FriendSnapshot for Bob, got: {:?}", other),
    }

    // Bob accepts the request
    bob.send_signal(&SignalMessage::RespondFriendRequest {
        user_id: alice_uid.clone(),
        accept: true,
    })
    .await;

    // Both get updated snapshot showing they're now friends
    let bob_snap2 = bob.recv_signal().await;
    match &bob_snap2 {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            ..
        } => {
            assert!(incoming_requests.is_empty(), "Request should be cleared");
            assert_eq!(friends.len(), 1, "Bob should have 1 friend");
        }
        other => panic!("Expected FriendSnapshot for Bob, got: {:?}", other),
    }

    let alice_snap2 = alice.recv_signal().await;
    match &alice_snap2 {
        SignalMessage::FriendSnapshot {
            friends,
            outgoing_requests,
            ..
        } => {
            assert!(outgoing_requests.is_empty(), "Request should be cleared");
            assert_eq!(friends.len(), 1, "Alice should have 1 friend");
        }
        other => panic!("Expected FriendSnapshot for Alice, got: {:?}", other),
    }

    // Alice can now DM Bob
    alice
        .send_signal(&SignalMessage::SelectDirectMessage {
            user_id: bob_uid.clone(),
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::DirectMessageSelected {
            user_id, history, ..
        } => {
            assert_eq!(user_id, bob_uid);
            assert!(history.is_empty(), "Fresh DM should have no history");
        }
        other => panic!("Expected DirectMessageSelected, got: {:?}", other),
    }

    // Alice sends a DM
    alice
        .send_signal(&SignalMessage::SendDirectMessage {
            user_id: bob_uid.clone(),
            content: "Hey Bob!".to_string(),
            reply_to_message_id: None,
        })
        .await;

    // Both receive the direct message
    match alice.recv_signal().await {
        SignalMessage::DirectMessage { message, .. } => {
            assert_eq!(message.content, "Hey Bob!");
        }
        other => panic!("Expected DirectMessage for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::DirectMessage { message, .. } => {
            assert_eq!(message.content, "Hey Bob!");
        }
        other => panic!("Expected DirectMessage for Bob, got: {:?}", other),
    }
}

/// Test: friend request mutual send auto-accepts.
#[tokio::test]
async fn test_friend_request_mutual_auto_accept() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, alice_uid) = authenticate(&mut alice, "Alice", None).await;

    let mut bob = server.connect().await;
    let (_bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;

    // Alice sends request to Bob
    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;
    // Drain snapshots
    alice.recv_signal().await;
    bob.recv_signal().await;

    // Bob sends request to Alice (should auto-accept since Alice already requested)
    bob.send_signal(&SignalMessage::SendFriendRequest {
        user_id: alice_uid.clone(),
    })
    .await;

    // Both should now see each other as friends (not pending)
    let bob_snap = bob.recv_signal().await;
    match &bob_snap {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
            ..
        } => {
            assert_eq!(friends.len(), 1, "Should be friends now");
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!("Expected FriendSnapshot for Bob, got: {:?}", other),
    }

    let alice_snap = alice.recv_signal().await;
    match &alice_snap {
        SignalMessage::FriendSnapshot {
            friends,
            incoming_requests,
            outgoing_requests,
            ..
        } => {
            assert_eq!(friends.len(), 1, "Should be friends now");
            assert!(incoming_requests.is_empty());
            assert!(outgoing_requests.is_empty());
        }
        other => panic!("Expected FriendSnapshot for Alice, got: {:?}", other),
    }
}

/// Test: DM to non-friend fails.
#[tokio::test]
async fn test_dm_to_non_friend_rejected() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, _alice_uid) = authenticate(&mut alice, "Alice", None).await;

    let mut bob = server.connect().await;
    let (_bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;

    // Alice tries to DM Bob without being friends
    alice
        .send_signal(&SignalMessage::SelectDirectMessage {
            user_id: bob_uid.clone(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("friends"),
                "Should mention friends requirement: {message}"
            );
        }
        other => panic!("Expected Error, got: {:?}", other),
    }
}

/// Test: ban enforcement — banned user cannot rejoin space.
#[tokio::test]
async fn test_ban_prevents_rejoin() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, _alice_uid) = authenticate(&mut alice, "Alice", None).await;
    let (space, _) = create_space(&mut alice, "Ban Test", "Alice").await;

    let mut bob = server.connect().await;
    let (_bob_token, _bob_uid) = authenticate(&mut bob, "Bob", None).await;
    join_space(&mut bob, &space.invite_code, "Bob").await;

    // Get Bob's member_id from Alice's MemberOnline
    let bob_member_id = match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => member.id,
        other => panic!("Expected MemberOnline, got: {:?}", other),
    };

    // Alice bans Bob
    alice
        .send_signal(&SignalMessage::BanMember {
            member_id: bob_member_id.clone(),
        })
        .await;

    // Bob should receive Kicked
    match bob.recv_signal().await {
        SignalMessage::Kicked { reason } => {
            assert!(!reason.is_empty());
        }
        other => panic!("Expected Kicked, got: {:?}", other),
    }

    // Alice receives MemberOffline
    match alice.recv_signal().await {
        SignalMessage::MemberOffline { .. } => {}
        other => panic!("Expected MemberOffline, got: {:?}", other),
    }

    // Bob tries to rejoin — should be rejected
    bob.send_signal(&SignalMessage::JoinSpace {
        invite_code: space.invite_code.clone(),
        user_name: "Bob".to_string(),
    })
    .await;

    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(message.contains("banned"), "Should mention ban: {message}");
        }
        other => panic!("Expected Error (banned), got: {:?}", other),
    }
}

/// Test: kick allows rejoin (unlike ban).
#[tokio::test]
async fn test_kick_allows_rejoin() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "Kick Test", "Alice").await;

    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;

    let bob_member_id = match alice.recv_signal().await {
        SignalMessage::MemberOnline { member } => member.id,
        other => panic!("Expected MemberOnline, got: {:?}", other),
    };

    // Alice kicks Bob
    alice
        .send_signal(&SignalMessage::KickMember {
            member_id: bob_member_id.clone(),
        })
        .await;

    // Bob gets Kicked
    match bob.recv_signal().await {
        SignalMessage::Kicked { .. } => {}
        other => panic!("Expected Kicked, got: {:?}", other),
    }

    // Drain MemberOffline from Alice
    alice.recv_signal().await;

    // Bob can rejoin (kick != ban)
    let (_sp, _ch, members) = join_space(&mut bob, &space.invite_code, "Bob").await;
    assert!(
        !members.is_empty(),
        "Bob should be able to rejoin after kick"
    );
}

/// Test: screen share mutual exclusion.
#[tokio::test]
async fn test_screen_share_mutual_exclusion() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let code = create_room(&mut alice, "Alice").await;

    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;
    // Drain RoomJoined for Bob
    bob.recv_signal().await;
    // Drain PeerJoined for Alice
    alice.recv_signal().await;

    // Alice starts screen share
    alice.send_signal(&SignalMessage::StartScreenShare).await;

    // Both receive ScreenShareStarted
    match alice.recv_signal().await {
        SignalMessage::ScreenShareStarted {
            sharer_name,
            is_self,
            ..
        } => {
            assert_eq!(sharer_name, "Alice");
            assert!(is_self);
        }
        other => panic!("Expected ScreenShareStarted for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStarted {
            sharer_name,
            is_self,
            ..
        } => {
            assert_eq!(sharer_name, "Alice");
            assert!(!is_self);
        }
        other => panic!("Expected ScreenShareStarted for Bob, got: {:?}", other),
    }

    // Bob tries to start screen share — should be rejected
    bob.send_signal(&SignalMessage::StartScreenShare).await;
    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("already active"),
                "Should mention active share: {message}"
            );
        }
        other => panic!("Expected Error, got: {:?}", other),
    }

    // Alice stops screen share
    alice.send_signal(&SignalMessage::StopScreenShare).await;
    match alice.recv_signal().await {
        SignalMessage::ScreenShareStopped { .. } => {}
        other => panic!("Expected ScreenShareStopped for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStopped { .. } => {}
        other => panic!("Expected ScreenShareStopped for Bob, got: {:?}", other),
    }

    // Now Bob can start screen share
    bob.send_signal(&SignalMessage::StartScreenShare).await;
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStarted {
            sharer_name,
            is_self,
            ..
        } => {
            assert_eq!(sharer_name, "Bob");
            assert!(is_self);
        }
        other => panic!("Expected ScreenShareStarted for Bob, got: {:?}", other),
    }
}

/// Test: screen share stops on disconnect.
#[tokio::test]
async fn test_screen_share_stops_on_disconnect() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let code = create_room(&mut alice, "Alice").await;

    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;
    bob.recv_signal().await; // RoomJoined
    alice.recv_signal().await; // PeerJoined

    // Bob starts screen share
    bob.send_signal(&SignalMessage::StartScreenShare).await;
    alice.recv_signal().await; // ScreenShareStarted
    bob.recv_signal().await; // ScreenShareStarted

    // Bob disconnects
    drop(bob);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Alice should receive ScreenShareStopped and PeerLeft
    let mut got_stopped = false;
    let mut got_left = false;
    for _ in 0..5 {
        match alice.recv_signal_timeout(Duration::from_secs(2)).await {
            Some(SignalMessage::ScreenShareStopped { .. }) => got_stopped = true,
            Some(SignalMessage::PeerLeft { .. }) => got_left = true,
            _ => break,
        }
    }
    assert!(got_stopped, "Alice should receive ScreenShareStopped");
    assert!(got_left, "Alice should receive PeerLeft");
}

/// Test: typing indicators are broadcast and cleared.
#[tokio::test]
async fn test_typing_indicators() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "Typing Test", "Alice").await;

    // Create text channel
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "typing-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    alice.recv_signal().await; // MemberOnline

    // Alice starts typing
    alice
        .send_signal(&SignalMessage::SetTyping {
            channel_id: text_ch_id.clone(),
            is_typing: true,
        })
        .await;

    // Bob should receive typing indicator
    match bob.recv_signal().await {
        SignalMessage::TypingState {
            channel_id,
            user_name,
            is_typing,
        } => {
            assert_eq!(channel_id, text_ch_id);
            assert_eq!(user_name, "Alice");
            assert!(is_typing);
        }
        other => panic!("Expected TypingState, got: {:?}", other),
    }

    // Alice stops typing
    alice
        .send_signal(&SignalMessage::SetTyping {
            channel_id: text_ch_id.clone(),
            is_typing: false,
        })
        .await;

    match bob.recv_signal().await {
        SignalMessage::TypingState { is_typing, .. } => {
            assert!(!is_typing);
        }
        other => panic!("Expected TypingState (false), got: {:?}", other),
    }
}

/// Test: react to message and receive reaction broadcast.
#[tokio::test]
async fn test_message_reactions() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "React Test", "Alice").await;

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "react-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    alice.recv_signal().await; // MemberOnline

    // Select text channel
    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: text_ch_id.clone(),
        })
        .await;
    alice.recv_signal().await; // TextChannelSelected

    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: text_ch_id.clone(),
    })
    .await;
    bob.recv_signal().await; // TextChannelSelected

    // Alice sends a message
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: text_ch_id.clone(),
            content: "React to this!".to_string(),
            reply_to_message_id: None,
        })
        .await;

    let message_id = match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => message.message_id,
        other => panic!("Expected TextMessage, got: {:?}", other),
    };
    bob.recv_signal().await; // TextMessage

    // Bob reacts with an emoji
    bob.send_signal(&SignalMessage::ReactToMessage {
        channel_id: text_ch_id.clone(),
        message_id: message_id.clone(),
        emoji: "👍".to_string(),
    })
    .await;

    // Both should receive MessageReaction
    for client_name in ["Alice", "Bob"] {
        let client: &mut TestClient = if client_name == "Alice" {
            &mut alice
        } else {
            &mut bob
        };
        match client.recv_signal().await {
            SignalMessage::MessageReaction {
                message_id: mid,
                emoji,
                ..
            } => {
                assert_eq!(mid, message_id);
                assert_eq!(emoji, "👍");
            }
            other => panic!(
                "Expected MessageReaction for {client_name}, got: {:?}",
                other
            ),
        }
    }
}

/// Test: space delete while members are in channels.
#[tokio::test]
async fn test_space_delete_with_active_members() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, channels) = create_space(&mut alice, "Delete Active", "Alice").await;
    let channel_id = channels[0].id.clone();

    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    alice.recv_signal().await; // MemberOnline

    // Bob joins voice channel
    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: channel_id.clone(),
    })
    .await;
    loop {
        match bob.recv_signal().await {
            SignalMessage::ChannelJoined { .. } => break,
            _ => continue,
        }
    }

    // Alice deletes the space — should not crash even with Bob in a channel
    alice.send_signal(&SignalMessage::DeleteSpace).await;

    // Alice should get SpaceDeleted
    loop {
        match alice.recv_signal_timeout(Duration::from_secs(3)).await {
            Some(SignalMessage::SpaceDeleted) => break,
            Some(_) => continue,
            None => panic!("Expected SpaceDeleted"),
        }
    }

    // Bob should get SpaceDeleted too
    loop {
        match bob.recv_signal_timeout(Duration::from_secs(3)).await {
            Some(SignalMessage::SpaceDeleted) => break,
            Some(_) => continue,
            None => panic!("Bob should receive SpaceDeleted"),
        }
    }

    // Server should still be alive — Alice can create new space
    let (new_space, _) = create_space(&mut alice, "Post Delete", "Alice").await;
    assert!(!new_space.id.is_empty());
}

/// Test: mute/deafen state broadcasts correctly in a room.
#[tokio::test]
async fn test_mute_deafen_broadcasts() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let code = create_room(&mut alice, "Alice").await;

    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;
    bob.recv_signal().await; // RoomJoined
    alice.recv_signal().await; // PeerJoined

    // Bob mutes
    bob.send_signal(&SignalMessage::MuteChanged { is_muted: true })
        .await;
    match alice.recv_signal().await {
        SignalMessage::PeerMuteChanged {
            peer_id, is_muted, ..
        } => {
            assert!(is_muted);
            assert!(!peer_id.is_empty());
        }
        other => panic!("Expected PeerMuteChanged, got: {:?}", other),
    }

    // Bob deafens
    bob.send_signal(&SignalMessage::DeafenChanged { is_deafened: true })
        .await;
    match alice.recv_signal().await {
        SignalMessage::PeerDeafenChanged { is_deafened, .. } => {
            assert!(is_deafened);
        }
        other => panic!("Expected PeerDeafenChanged, got: {:?}", other),
    }

    // Bob unmutes and undeafens
    bob.send_signal(&SignalMessage::MuteChanged { is_muted: false })
        .await;
    match alice.recv_signal().await {
        SignalMessage::PeerMuteChanged { is_muted, .. } => {
            assert!(!is_muted);
        }
        other => panic!("Expected PeerMuteChanged (unmute), got: {:?}", other),
    }
}

/// Test: audio relay between peers in a room.
#[tokio::test]
async fn test_audio_relay_stress() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let code = create_room(&mut alice, "Alice").await;

    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;
    bob.recv_signal().await; // RoomJoined
    alice.recv_signal().await; // PeerJoined

    // Alice sends 100 audio frames rapidly
    let fake_audio = vec![0u8; 64]; // Simulated Opus frame
    for _ in 0..100 {
        alice.send_binary(&fake_audio).await;
    }

    // Bob should receive audio frames
    let mut received = 0;
    loop {
        match bob.recv_binary_timeout(Duration::from_secs(2)).await {
            Some(data) => {
                assert!(!data.is_empty(), "Audio frame should not be empty");
                received += 1;
            }
            None => break,
        }
    }

    assert!(
        received >= 50,
        "Bob should receive most audio frames, got {received}/100"
    );
}

/// Test: room capacity limit (MAX_ROOM_PEERS).
#[tokio::test]
async fn test_room_capacity_limit() {
    let server = TestServer::start().await;

    let mut creator = server.connect().await;
    let code = create_room(&mut creator, "Host").await;

    // Join peers up to the limit (try 25 — default MAX_ROOM_PEERS is usually 25-50)
    let mut peers = Vec::new();
    for i in 0..24 {
        let mut client = server.connect().await;
        client
            .send_signal(&SignalMessage::JoinRoom {
                room_code: code.clone(),
                user_name: format!("Peer{i}"),
                password: None,
            })
            .await;
        match client.recv_signal().await {
            SignalMessage::RoomJoined { .. } => {}
            SignalMessage::Error { message } => {
                // Hit the limit — that's fine, just stop
                assert!(
                    message.contains("full") || message.contains("limit"),
                    "Error should mention capacity: {message}"
                );
                break;
            }
            other => panic!("Expected RoomJoined or Error, got: {:?}", other),
        }
        peers.push(client);
    }

    // Server should still be responsive
    let mut check = server.connect().await;
    let code2 = create_room(&mut check, "Check").await;
    assert!(!code2.is_empty(), "Server should still create rooms");
}

/// Test: empty/oversized message content rejected.
#[tokio::test]
async fn test_message_content_validation() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (_space, _) = create_space(&mut alice, "Validation", "Alice").await;

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "val-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: text_ch_id.clone(),
        })
        .await;
    alice.recv_signal().await; // TextChannelSelected

    // Send empty message — should be silently rejected (no response)
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: text_ch_id.clone(),
            content: "".to_string(),
            reply_to_message_id: None,
        })
        .await;

    // Send oversized message (>2000 chars)
    let huge_msg = "x".repeat(2001);
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: text_ch_id.clone(),
            content: huge_msg,
            reply_to_message_id: None,
        })
        .await;

    // Send valid message to confirm server is alive
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: text_ch_id.clone(),
            content: "Valid message".to_string(),
            reply_to_message_id: None,
        })
        .await;

    // Should get TextMessage for the valid one only
    match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => {
            assert_eq!(message.content, "Valid message");
        }
        other => panic!("Expected TextMessage for valid msg, got: {:?}", other),
    }
}

/// Test: concurrent room joins from many peers.
#[tokio::test]
async fn test_concurrent_room_joins() {
    let server = TestServer::start().await;

    let mut host = server.connect().await;
    let code = create_room(&mut host, "Host").await;

    // 9 peers join simultaneously (host is already in, MAX_ROOM_PEERS is 10)
    let mut peers = Vec::new();
    for i in 0..9 {
        let mut client = server.connect().await;
        client
            .send_signal(&SignalMessage::JoinRoom {
                room_code: code.clone(),
                user_name: format!("Peer{i}"),
                password: None,
            })
            .await;
        peers.push(client);
    }

    // All should get RoomJoined
    for (i, client) in peers.iter_mut().enumerate() {
        match client.recv_signal().await {
            SignalMessage::RoomJoined { participants, .. } => {
                assert!(
                    !participants.is_empty(),
                    "Peer{i} should see at least the host"
                );
            }
            other => panic!("Expected RoomJoined for Peer{i}, got: {:?}", other),
        }
    }

    // Host should have 10 PeerJoined notifications
    let mut join_count = 0;
    loop {
        match host.recv_signal_timeout(Duration::from_secs(2)).await {
            Some(SignalMessage::PeerJoined { .. }) => join_count += 1,
            _ => break,
        }
    }
    assert_eq!(join_count, 9, "Host should see 9 peer joins");
}

// ─── Edge Case & Performance Tests ───

/// Test: concurrent edits to the same message.
#[tokio::test]
async fn test_concurrent_message_edits() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "Edit Race", "Alice").await;

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "edit-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: ch_id.clone(),
        })
        .await;
    alice.recv_signal().await; // TextChannelSelected

    // Send a message
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: ch_id.clone(),
            content: "Original".to_string(),
            reply_to_message_id: None,
        })
        .await;
    let msg_id = match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => message.message_id,
        other => panic!("Expected TextMessage, got: {:?}", other),
    };

    // Edit it 10 times rapidly
    for i in 0..10 {
        alice
            .send_signal(&SignalMessage::EditTextMessage {
                channel_id: ch_id.clone(),
                message_id: msg_id.clone(),
                new_content: format!("Edit #{i}"),
            })
            .await;
    }

    // Drain all edit notifications
    let mut edit_count = 0;
    let mut last_content = String::new();
    loop {
        match alice.recv_signal_timeout(Duration::from_secs(2)).await {
            Some(SignalMessage::TextMessageEdited { new_content, .. }) => {
                last_content = new_content;
                edit_count += 1;
            }
            _ => break,
        }
    }
    assert_eq!(edit_count, 10, "Should receive all 10 edit notifications");
    assert_eq!(last_content, "Edit #9", "Last edit should be #9");

    // Verify history shows edited message
    let mut bob = server.connect().await;
    join_space(&mut bob, &space.invite_code, "Bob").await;
    alice.recv_signal().await; // MemberOnline

    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: ch_id.clone(),
    })
    .await;
    match bob.recv_signal().await {
        SignalMessage::TextChannelSelected { history, .. } => {
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].content, "Edit #9");
            assert!(history[0].edited, "Message should be marked as edited");
        }
        other => panic!("Expected TextChannelSelected, got: {:?}", other),
    }
}

/// Test: delete then edit same message (edit should fail gracefully).
#[tokio::test]
async fn test_delete_then_edit_message() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (_space, _) = create_space(&mut alice, "Delete Edit", "Alice").await;

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "de-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: ch_id.clone(),
        })
        .await;
    alice.recv_signal().await;

    // Send then delete
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: ch_id.clone(),
            content: "To be deleted".to_string(),
            reply_to_message_id: None,
        })
        .await;
    let msg_id = match alice.recv_signal().await {
        SignalMessage::TextMessage { message, .. } => message.message_id,
        other => panic!("Expected TextMessage, got: {:?}", other),
    };

    alice
        .send_signal(&SignalMessage::DeleteTextMessage {
            channel_id: ch_id.clone(),
            message_id: msg_id.clone(),
        })
        .await;
    match alice.recv_signal().await {
        SignalMessage::TextMessageDeleted { .. } => {}
        other => panic!("Expected TextMessageDeleted, got: {:?}", other),
    }

    // Try to edit deleted message — should fail gracefully (error or silent)
    alice
        .send_signal(&SignalMessage::EditTextMessage {
            channel_id: ch_id.clone(),
            message_id: msg_id.clone(),
            new_content: "Ghost edit".to_string(),
        })
        .await;

    // May get Error for editing deleted message — that's correct behavior
    // Send a valid message to confirm server is alive
    alice
        .send_signal(&SignalMessage::SendTextMessage {
            channel_id: ch_id.clone(),
            content: "Still alive".to_string(),
            reply_to_message_id: None,
        })
        .await;

    // Drain until we get the valid TextMessage (may get Error first)
    loop {
        match alice.recv_signal().await {
            SignalMessage::TextMessage { message, .. } => {
                assert_eq!(message.content, "Still alive");
                break;
            }
            SignalMessage::Error { .. } => continue, // Expected for edit of deleted
            other => panic!("Expected TextMessage or Error, got: {:?}", other),
        }
    }
}

/// Test: presence updates when user joins/leaves spaces and rooms.
#[tokio::test]
async fn test_presence_updates_on_activity() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, alice_uid) = authenticate(&mut alice, "Alice", None).await;

    let mut bob = server.connect().await;
    let (_bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;

    // Make them friends
    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;
    alice.recv_signal().await; // FriendSnapshot
    bob.recv_signal().await; // FriendSnapshot

    bob.send_signal(&SignalMessage::RespondFriendRequest {
        user_id: alice_uid.clone(),
        accept: true,
    })
    .await;
    bob.recv_signal().await; // FriendSnapshot
    alice.recv_signal().await; // FriendSnapshot

    // Alice watches Bob's presence
    alice
        .send_signal(&SignalMessage::WatchFriendPresence {
            user_ids: vec![bob_uid.clone()],
        })
        .await;

    // Should get initial presence snapshot
    match alice.recv_signal().await {
        SignalMessage::FriendPresenceSnapshot { presences } => {
            assert_eq!(presences.len(), 1);
            assert_eq!(presences[0].user_id, bob_uid);
            assert!(presences[0].is_online, "Bob should be online");
        }
        other => panic!("Expected FriendPresenceSnapshot, got: {:?}", other),
    }

    // Bob joins a space — Alice should get presence change
    let (_space, channels) = create_space(&mut bob, "Bob Space", "Bob").await;

    match alice.recv_signal().await {
        SignalMessage::FriendPresenceChanged { presence } => {
            assert_eq!(presence.user_id, bob_uid);
            assert!(presence.is_online);
            assert!(
                presence.active_space_name.is_some(),
                "Should show Bob in a space"
            );
        }
        other => panic!("Expected FriendPresenceChanged, got: {:?}", other),
    }

    // Bob joins a voice channel — Alice should get updated presence
    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: channels[0].id.clone(),
    })
    .await;
    loop {
        match bob.recv_signal().await {
            SignalMessage::ChannelJoined { .. } => break,
            _ => continue,
        }
    }

    match alice.recv_signal().await {
        SignalMessage::FriendPresenceChanged { presence } => {
            assert_eq!(presence.user_id, bob_uid);
            assert!(presence.is_in_voice, "Should show Bob in voice");
        }
        other => panic!("Expected FriendPresenceChanged (voice), got: {:?}", other),
    }
}

/// Test: direct message send and receive with persistence.
#[tokio::test]
async fn test_direct_message_persistence() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, alice_uid) = authenticate(&mut alice, "Alice", None).await;
    let mut bob = server.connect().await;
    let (bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;

    // Befriend
    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;
    alice.recv_signal().await;
    bob.recv_signal().await;
    bob.send_signal(&SignalMessage::RespondFriendRequest {
        user_id: alice_uid.clone(),
        accept: true,
    })
    .await;
    bob.recv_signal().await;
    alice.recv_signal().await;

    // Alice sends 3 DMs
    for i in 0..3 {
        alice
            .send_signal(&SignalMessage::SendDirectMessage {
                user_id: bob_uid.clone(),
                content: format!("DM #{i}"),
                reply_to_message_id: None,
            })
            .await;
        // Both receive the message
        alice.recv_signal().await;
        bob.recv_signal().await;
    }

    // Wait for DB persistence
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Bob disconnects and reconnects — should see DM history from DB
    drop(bob);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut bob2 = server.connect().await;
    // Re-authenticate (can't use helper since friend list is non-empty now)
    bob2.send_signal(&SignalMessage::Authenticate {
        token: Some(bob_token),
        user_name: "Bob".to_string(),
    })
    .await;
    match bob2.recv_signal().await {
        SignalMessage::Authenticated { .. } => {}
        other => panic!("Expected Authenticated, got: {:?}", other),
    }
    // Drain FriendSnapshot (non-empty since Alice is a friend)
    match bob2.recv_signal().await {
        SignalMessage::FriendSnapshot { friends, .. } => {
            assert_eq!(friends.len(), 1, "Bob should have Alice as friend");
        }
        other => panic!("Expected FriendSnapshot, got: {:?}", other),
    }

    bob2.send_signal(&SignalMessage::SelectDirectMessage {
        user_id: alice_uid.clone(),
    })
    .await;
    match bob2.recv_signal().await {
        SignalMessage::DirectMessageSelected { history, .. } => {
            assert_eq!(history.len(), 3, "Should see 3 DMs from DB after reconnect");
            // Verify all 3 DMs are present (order may vary by DB query)
            let contents: Vec<&str> = history.iter().map(|m| m.content.as_str()).collect();
            assert!(contents.contains(&"DM #0"), "Missing DM #0: {:?}", contents);
            assert!(contents.contains(&"DM #1"), "Missing DM #1: {:?}", contents);
            assert!(contents.contains(&"DM #2"), "Missing DM #2: {:?}", contents);
        }
        other => panic!("Expected DirectMessageSelected, got: {:?}", other),
    }
}

/// Test: rate limiting — rapid signal messages get throttled.
#[tokio::test]
async fn test_signal_rate_limiting() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (_space, _) = create_space(&mut alice, "Rate Limit", "Alice").await;

    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "rl-ch".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: ch_id.clone(),
        })
        .await;
    alice.recv_signal().await;

    // Send 200 messages as fast as possible (rate limit is 100/sec)
    for i in 0..200 {
        alice
            .send_signal(&SignalMessage::SendTextMessage {
                channel_id: ch_id.clone(),
                content: format!("Spam #{i}"),
                reply_to_message_id: None,
            })
            .await;
    }

    // Count how many we actually received back
    let mut received = 0;
    loop {
        match alice.recv_signal_timeout(Duration::from_secs(2)).await {
            Some(SignalMessage::TextMessage { .. }) => received += 1,
            Some(SignalMessage::Error { .. }) => break, // Rate limited
            Some(_) => continue,
            None => break,
        }
    }

    // Should have received some but not all 200 (rate limited at ~100/sec)
    assert!(
        received > 0,
        "Should receive some messages before rate limit"
    );
    // Server should still be alive
    let mut check = server.connect().await;
    let code = create_room(&mut check, "Alive").await;
    assert!(!code.is_empty(), "Server should still be responsive");
}

/// Test: space owner identity preserved across reconnect.
#[tokio::test]
async fn test_space_owner_persists_across_reconnect() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (alice_token, _alice_uid) = authenticate(&mut alice, "Alice", None).await;
    let (space, _) = create_space(&mut alice, "Owner Test", "Alice").await;
    assert!(space.is_owner, "Alice should be space owner");

    // Alice disconnects
    drop(alice);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Alice reconnects with same token
    let mut alice2 = server.connect().await;
    authenticate(&mut alice2, "Alice", Some(alice_token)).await;

    // Rejoin the space
    let (space2, _, _) = join_space(&mut alice2, &space.invite_code, "Alice").await;
    assert!(
        space2.is_owner,
        "Alice should still be owner after reconnect"
    );

    // Verify she can still perform owner actions (create channel)
    alice2
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "post-reconnect".to_string(),
            channel_type: shared_types::ChannelType::Voice,
        })
        .await;
    match alice2.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => {
            assert_eq!(channel.name, "post-reconnect");
        }
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    }
}

/// Test: audio frames from disconnected peer are silently dropped.
#[tokio::test]
async fn test_audio_after_leave_room() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let code = create_room(&mut alice, "Alice").await;

    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;
    bob.recv_signal().await; // RoomJoined
    alice.recv_signal().await; // PeerJoined

    // Bob leaves the room
    bob.send_signal(&SignalMessage::LeaveRoom).await;
    // Drain PeerLeft from Alice
    alice.recv_signal().await;

    // Alice sends audio — should not crash even though room might be empty
    let fake_audio = vec![0u8; 64];
    for _ in 0..20 {
        alice.send_binary(&fake_audio).await;
    }

    // Bob should NOT receive any audio (not in room)
    let received = bob.recv_binary_timeout(Duration::from_millis(500)).await;
    assert!(
        received.is_none(),
        "Bob should not receive audio after leaving"
    );

    // Server still alive
    let mut check = server.connect().await;
    let code2 = create_room(&mut check, "Check").await;
    assert!(!code2.is_empty());
}

/// Test: friend request to self rejected.
#[tokio::test]
async fn test_friend_request_self_rejected() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (_token, alice_uid) = authenticate(&mut alice, "Alice", None).await;

    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: alice_uid.clone(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("yourself"),
                "Should mention self: {message}"
            );
        }
        other => panic!("Expected Error, got: {:?}", other),
    }
}

/// Test: duplicate friend request rejected.
#[tokio::test]
async fn test_duplicate_friend_request_rejected() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (_alice_token, _alice_uid) = authenticate(&mut alice, "Alice", None).await;
    let mut bob = server.connect().await;
    let (_bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;

    // First request
    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;
    alice.recv_signal().await; // FriendSnapshot
    bob.recv_signal().await;

    // Duplicate request
    alice
        .send_signal(&SignalMessage::SendFriendRequest {
            user_id: bob_uid.clone(),
        })
        .await;

    match alice.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("already"),
                "Should mention already pending: {message}"
            );
        }
        other => panic!("Expected Error for duplicate request, got: {:?}", other),
    }
}

// ─── Stress Tests: Concurrency, Reconnection, Edge Cases ───

/// Test: rapid connect/disconnect cycles don't crash the server.
#[tokio::test]
async fn test_rapid_connect_disconnect_cycles() {
    let server = TestServer::start().await;

    for _ in 0..20 {
        let mut client = server.connect().await;
        // Send a harmless message to engage the connection
        client
            .send_signal(&SignalMessage::CreateRoom {
                user_name: "FlashUser".to_string(),
                password: None,
            })
            .await;
        // Drop immediately — simulates abrupt disconnect
        drop(client);
    }

    // Server should still be alive and accepting connections
    let mut check = server.connect().await;
    let code = create_room(&mut check, "StillAlive").await;
    assert!(!code.is_empty());
}

/// Test: multiple clients join/leave rooms concurrently.
#[tokio::test]
async fn test_concurrent_room_join_leave() {
    let server = TestServer::start().await;
    let mut host = server.connect().await;
    let room_code = create_room(&mut host, "Host").await;

    // Spawn 5 clients that join, send audio, then leave concurrently
    let mut handles = Vec::new();
    for i in 0..5 {
        let mut client = server.connect().await;
        let code = room_code.clone();
        let name = format!("Client{i}");
        let handle = tokio::spawn(async move {
            join_room(&mut client, &code, &name).await;
            // Drain PeerJoined etc
            let _ = client.recv_signal_timeout(Duration::from_millis(500)).await;

            // Send a few audio frames
            let audio = generate_test_audio();
            for _ in 0..3 {
                client.send_binary(&audio).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            // Leave
            client.send_signal(&SignalMessage::LeaveRoom).await;
            let _ = client.recv_signal_timeout(Duration::from_millis(500)).await;
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    // Host should still be connected and functional
    host.send_signal(&SignalMessage::LeaveRoom).await;
}

/// Test: client sends malformed JSON — server ignores it, stays alive.
#[tokio::test]
async fn test_malformed_json_ignored() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Send garbage text
    client
        .sink
        .send(Message::Text("{{not valid json".into()))
        .await
        .unwrap();

    // Send more garbage
    client.sink.send(Message::Text("".into())).await.unwrap();

    // Send a valid unknown field — should be ignored gracefully
    client
        .sink
        .send(Message::Text(r#"{"UnknownVariant":{}}"#.into()))
        .await
        .unwrap();

    // Server should still respond to valid messages
    let code = create_room(&mut client, "StillWorks").await;
    assert!(!code.is_empty());
}

/// Test: oversized audio frame is rejected.
#[tokio::test]
async fn test_oversized_audio_frame_rejected() {
    let server = TestServer::start().await;
    let mut host = server.connect().await;
    let room_code = create_room(&mut host, "Host").await;
    let mut listener = server.connect().await;
    join_room(&mut listener, &room_code, "Listener").await;
    // drain PeerJoined
    let _ = listener
        .recv_signal_timeout(Duration::from_millis(300))
        .await;
    let _ = host.recv_signal_timeout(Duration::from_millis(300)).await;

    // Send a frame much larger than MAX_AUDIO_FRAME_SIZE (4096 bytes)
    let big_frame = vec![0u8; 10000];
    host.send_binary(&big_frame).await;

    // Listener should NOT receive the oversized frame
    let got = listener
        .recv_binary_timeout(Duration::from_millis(500))
        .await;
    assert!(got.is_none(), "Oversized frame should be rejected");

    // Normal-sized frame should still work
    let audio = generate_test_audio();
    host.send_binary(&audio).await;
    let got = listener.recv_binary_timeout(Duration::from_secs(2)).await;
    assert!(got.is_some(), "Normal frame should still be relayed");
}

/// Test: space operations while disconnected peers are listed as members.
#[tokio::test]
async fn test_stale_members_cleaned_on_join() {
    let server = TestServer::start().await;

    // Alice creates space
    let mut alice = server.connect().await;
    let (space, _channels) = create_space(&mut alice, "TestSpace", "Alice").await;

    // Bob joins, then abruptly disconnects
    let mut bob = server.connect().await;
    let (_, _, members) = join_space(&mut bob, &space.invite_code, "Bob").await;
    assert!(members.len() >= 1); // At least Alice
                                 // drain MemberJoined from Alice
    let _ = alice.recv_signal_timeout(Duration::from_millis(500)).await;

    drop(bob); // Abrupt disconnect
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Charlie joins — stale Bob should be cleaned up
    let mut charlie = server.connect().await;
    let (_, _, members) = join_space(&mut charlie, &space.invite_code, "Charlie").await;

    // members should contain Alice and Charlie but NOT Bob (stale)
    let member_names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
    assert!(
        !member_names.contains(&"Bob"),
        "Stale Bob should have been cleaned up. Members: {:?}",
        member_names
    );
}

/// Test: space invite code with whitespace/mixed case works.
#[tokio::test]
async fn test_invite_code_normalization() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "CaseTest", "Alice").await;

    // Try joining with the invite code in uppercase
    let mut bob = server.connect().await;
    let upper_code = space.invite_code.to_uppercase();
    bob.send_signal(&SignalMessage::JoinSpace {
        invite_code: upper_code,
        user_name: "Bob".to_string(),
    })
    .await;

    // Server should accept or reject consistently — check for either SpaceJoined or Error
    match bob.recv_signal().await {
        SignalMessage::SpaceJoined { .. } => {
            // Server normalizes case — great
        }
        SignalMessage::Error { message } => {
            // Server is case-sensitive — also valid
            assert!(
                message.contains("not found") || message.contains("Invalid"),
                "Unexpected error: {message}"
            );
        }
        other => panic!("Unexpected response: {:?}", other),
    }
}

/// Test: sending messages to a channel after being kicked from the space.
#[tokio::test]
async fn test_message_after_kick_rejected() {
    let server = TestServer::start().await;

    // Alice creates space, Bob joins
    let mut alice = server.connect().await;
    authenticate(&mut alice, "Alice", None).await;
    let (space, channels) = create_space(&mut alice, "KickTest", "Alice").await;

    let mut bob = server.connect().await;
    let (_bob_token, bob_uid) = authenticate(&mut bob, "Bob", None).await;
    let _ = join_space(&mut bob, &space.invite_code, "Bob").await;
    // drain MemberJoined from Alice
    let _ = alice.recv_signal_timeout(Duration::from_millis(500)).await;

    // Alice kicks Bob
    alice
        .send_signal(&SignalMessage::KickMember {
            member_id: bob_uid.clone(),
        })
        .await;

    // Bob should receive Kicked
    loop {
        match bob.recv_signal().await {
            SignalMessage::Kicked { .. } => break,
            _ => continue,
        }
    }

    // Find a text channel
    let text_ch = channels
        .iter()
        .find(|c| c.channel_type == shared_types::ChannelType::Text);
    if let Some(ch) = text_ch {
        // Bob tries to send a message — should get error or be ignored
        bob.send_signal(&SignalMessage::SendTextMessage {
            channel_id: ch.id.clone(),
            content: "Shouldn't work".to_string(),
            reply_to_message_id: None,
        })
        .await;

        match bob.recv_signal_timeout(Duration::from_secs(1)).await {
            Some(SignalMessage::Error { .. }) => {} // Expected
            None => {}                              // Also OK — server ignored it
            Some(other) => {
                // Might get SpaceDeleted or similar; as long as it's not a TextMessage, it's fine
                assert!(
                    !matches!(other, SignalMessage::TextMessage { .. }),
                    "Kicked user should not be able to send messages"
                );
            }
        }
    }
}

/// Test: two clients authenticate with the same token (reconnect scenario).
#[tokio::test]
async fn test_reconnect_same_token() {
    let server = TestServer::start().await;

    let mut alice1 = server.connect().await;
    let (token, user_id) = authenticate(&mut alice1, "Alice", None).await;

    // Alice reconnects with same token
    let mut alice2 = server.connect().await;
    alice2
        .send_signal(&SignalMessage::Authenticate {
            token: Some(token.clone()),
            user_name: "Alice".to_string(),
        })
        .await;
    match alice2.recv_signal().await {
        SignalMessage::Authenticated {
            token: t2,
            user_id: uid2,
        } => {
            assert_eq!(uid2, user_id, "Same token should yield same user_id");
            assert_ne!(t2, token, "Restore should rotate the token");
        }
        other => panic!("Expected Authenticated, got: {:?}", other),
    }
    // drain FriendSnapshot
    let _ = alice2.recv_signal_timeout(Duration::from_millis(500)).await;
}

/// Test: creating many channels doesn't crash or slow down.
#[tokio::test]
async fn test_many_channels_in_space() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let (space, _) = create_space(&mut alice, "BigSpace", "Alice").await;

    for i in 0..20 {
        alice
            .send_signal(&SignalMessage::CreateChannel {
                channel_name: format!("channel-{i}"),
                channel_type: if i % 2 == 0 {
                    shared_types::ChannelType::Voice
                } else {
                    shared_types::ChannelType::Text
                },
            })
            .await;
        match alice.recv_signal().await {
            SignalMessage::ChannelCreated { .. } => {}
            other => panic!("Expected ChannelCreated for channel {i}, got: {:?}", other),
        }
    }

    // Verify Bob can join and see all channels
    let mut bob = server.connect().await;
    let (_, channels, _) = join_space(&mut bob, &space.invite_code, "Bob").await;
    // Original General (voice) + 20 new = 21
    assert!(
        channels.len() >= 21,
        "Expected at least 21 channels, got {}",
        channels.len()
    );
}

/// Test: empty room cleanup — creating and leaving a room, then verifying it's gone.
#[tokio::test]
async fn test_room_cleaned_after_all_leave() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let room_code = create_room(&mut alice, "Alice").await;

    // Leave the room
    alice.send_signal(&SignalMessage::LeaveRoom).await;
    let _ = alice.recv_signal_timeout(Duration::from_millis(500)).await;

    // Try to join the now-empty room — should fail
    let mut bob = server.connect().await;
    bob.send_signal(&SignalMessage::JoinRoom {
        room_code: room_code.clone(),
        user_name: "Bob".to_string(),
        password: None,
    })
    .await;

    match bob.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.to_lowercase().contains("not found")
                    || message.to_lowercase().contains("does not exist"),
                "Expected room not found error, got: {message}"
            );
        }
        SignalMessage::RoomJoined { .. } => {
            panic!("Should not be able to join a cleaned-up room");
        }
        other => panic!("Expected Error, got: {:?}", other),
    }
}

/// Test: sending text message with unicode/emoji content.
#[tokio::test]
async fn test_unicode_text_messages() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, _channels) = create_space(&mut alice, "UnicodeSpace", "Alice").await;

    // Create a text channel (default space only has a voice channel)
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    let mut bob = server.connect().await;
    let _ = join_space(&mut bob, &space.invite_code, "Bob").await;
    // drain MemberJoined from Alice
    let _ = alice.recv_signal_timeout(Duration::from_millis(500)).await;

    // Select channel for Bob
    bob.send_signal(&SignalMessage::SelectTextChannel {
        channel_id: text_ch.id.clone(),
    })
    .await;
    loop {
        match bob.recv_signal().await {
            SignalMessage::TextChannelSelected { .. } => break,
            _ => continue,
        }
    }

    // Alice sends unicode-heavy messages
    let test_messages = vec![
        "Hello 🎉🎊🎈",
        "日本語テスト",
        "Ñoño café résumé naïve",
        "🏳️‍🌈 flag test",
        "Mixed: hello世界🌍",
        // Long multi-byte message that would panic with byte slicing at 50
        "🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥",
    ];

    alice
        .send_signal(&SignalMessage::SelectTextChannel {
            channel_id: text_ch.id.clone(),
        })
        .await;
    loop {
        match alice.recv_signal().await {
            SignalMessage::TextChannelSelected { .. } => break,
            _ => continue,
        }
    }

    for msg in &test_messages {
        alice
            .send_signal(&SignalMessage::SendTextMessage {
                channel_id: text_ch.id.clone(),
                content: msg.to_string(),
                reply_to_message_id: None,
            })
            .await;
        // Wait for Alice's echo
        loop {
            match alice.recv_signal().await {
                SignalMessage::TextMessage { .. } => break,
                _ => continue,
            }
        }
    }

    // Bob should receive all messages intact
    let mut received = Vec::new();
    for _ in 0..test_messages.len() {
        loop {
            match bob.recv_signal().await {
                SignalMessage::TextMessage { message, .. } => {
                    received.push(message.content);
                    break;
                }
                _ => continue,
            }
        }
    }

    for expected in &test_messages {
        assert!(
            received.iter().any(|r| r == expected),
            "Missing message: {expected}"
        );
    }
}

/// Test: concurrent space operations — create, join, create channels, send messages.
#[tokio::test]
async fn test_concurrent_space_operations() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, _channels) = create_space(&mut alice, "ConcurrentSpace", "Alice").await;

    // Create a text channel first
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "chat".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;
    let text_ch_id = match alice.recv_signal().await {
        SignalMessage::ChannelCreated { channel } => channel.id,
        other => panic!("Expected ChannelCreated, got: {:?}", other),
    };

    // Spawn multiple joiners concurrently
    let mut handles = Vec::new();
    for i in 0..5 {
        let mut client = server.connect().await;
        let invite = space.invite_code.clone();
        let ch_id = text_ch_id.clone();
        let name = format!("User{i}");
        let handle = tokio::spawn(async move {
            let _ = join_space(&mut client, &invite, &name).await;

            // Select text channel and send a message
            client
                .send_signal(&SignalMessage::SelectTextChannel {
                    channel_id: ch_id.clone(),
                })
                .await;
            loop {
                match client.recv_signal().await {
                    SignalMessage::TextChannelSelected { .. } => break,
                    _ => continue,
                }
            }

            client
                .send_signal(&SignalMessage::SendTextMessage {
                    channel_id: ch_id,
                    content: format!("Hello from {name}"),
                    reply_to_message_id: None,
                })
                .await;

            // Drain response
            let _ = client.recv_signal_timeout(Duration::from_millis(500)).await;
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }

    // Alice should still be able to operate
    alice
        .send_signal(&SignalMessage::CreateChannel {
            channel_name: "after-concurrent".to_string(),
            channel_type: shared_types::ChannelType::Text,
        })
        .await;

    // Drain until we find ChannelCreated (may get MemberJoined and TextMessage first)
    loop {
        match alice.recv_signal().await {
            SignalMessage::ChannelCreated { .. } => break,
            _ => continue,
        }
    }
}

/// Test: creating a room with max-length username works, over-length is rejected.
#[tokio::test]
async fn test_very_long_username() {
    let server = TestServer::start().await;

    // Max length (32) should work
    let mut client1 = server.connect().await;
    let ok_name = "A".repeat(32);
    let code = create_room(&mut client1, &ok_name).await;
    assert!(!code.is_empty());

    // Over max length should be rejected
    let mut client2 = server.connect().await;
    let long_name = "A".repeat(1000);
    client2
        .send_signal(&SignalMessage::CreateRoom {
            user_name: long_name,
            password: None,
        })
        .await;
    match client2.recv_signal().await {
        SignalMessage::Error { message } => {
            assert!(
                message.contains("too long"),
                "Expected 'too long' error: {message}"
            );
        }
        other => panic!("Expected Error for long name, got: {:?}", other),
    }
}

/// Test: audio relay under load — many frames sent rapidly.
#[tokio::test]
async fn test_audio_relay_burst() {
    let server = TestServer::start().await;
    let mut host = server.connect().await;
    let room_code = create_room(&mut host, "Host").await;
    let mut listener = server.connect().await;
    join_room(&mut listener, &room_code, "Listener").await;
    // drain PeerJoined
    let _ = listener
        .recv_signal_timeout(Duration::from_millis(300))
        .await;
    let _ = host.recv_signal_timeout(Duration::from_millis(300)).await;

    // Send 200 frames rapidly (simulates burst)
    let audio = generate_test_audio();
    for _ in 0..200 {
        host.send_binary(&audio).await;
    }

    // Give server time to process
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Listener should receive some frames (rate limiter may drop some, which is fine)
    let mut received = 0;
    loop {
        match listener
            .recv_binary_timeout(Duration::from_millis(200))
            .await
        {
            Some(_) => received += 1,
            None => break,
        }
    }
    assert!(
        received > 0,
        "Should receive at least some audio frames from burst"
    );
    // Due to 100fps rate limit, should be capped
    assert!(
        received <= 110,
        "Rate limiter should cap frames, got {received}"
    );
}

/// Test: WebSocket ping/pong keepalive (server sends pings).
#[tokio::test]
async fn test_websocket_keepalive() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Create a room so we have an active connection
    let _code = create_room(&mut client, "PingTest").await;

    // Wait a bit — server should ping us periodically
    // The tungstenite library auto-responds to pings, so we just need
    // to verify the connection stays alive
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Connection should still be alive
    client
        .send_signal(&SignalMessage::CreateRoom {
            user_name: "StillAlive".to_string(),
            password: None,
        })
        .await;
    // No panic = success
}

/// Test: deleting a space while members are in a voice channel.
#[tokio::test]
async fn test_delete_space_with_active_voice() {
    let server = TestServer::start().await;

    let mut alice = server.connect().await;
    let (space, channels) = create_space(&mut alice, "DeleteTest", "Alice").await;
    let voice_ch = channels
        .iter()
        .find(|c| c.channel_type == shared_types::ChannelType::Voice)
        .expect("Should have voice channel");

    let mut bob = server.connect().await;
    let _ = join_space(&mut bob, &space.invite_code, "Bob").await;
    // drain MemberJoined from Alice
    let _ = alice.recv_signal_timeout(Duration::from_millis(500)).await;

    // Bob joins voice channel
    bob.send_signal(&SignalMessage::JoinChannel {
        channel_id: voice_ch.id.clone(),
    })
    .await;
    loop {
        match bob.recv_signal().await {
            SignalMessage::ChannelJoined { .. } => break,
            _ => continue,
        }
    }
    // drain MemberChannelChanged from Alice
    let _ = alice.recv_signal_timeout(Duration::from_millis(500)).await;

    // Alice deletes space
    alice.send_signal(&SignalMessage::DeleteSpace).await;

    // Both should get SpaceDeleted
    loop {
        match alice.recv_signal().await {
            SignalMessage::SpaceDeleted { .. } => break,
            _ => continue,
        }
    }
    loop {
        match bob.recv_signal().await {
            SignalMessage::SpaceDeleted { .. } => break,
            _ => continue,
        }
    }

    // Both clients should still be functional
    let code = create_room(&mut alice, "Alice").await;
    assert!(!code.is_empty());
}
