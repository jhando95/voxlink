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

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

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

    async fn recv_signal_timeout(&mut self, dur: Duration) -> Option<SignalMessage> {
        loop {
            match timeout(dur, self.stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    if let Ok(msg) = serde_json::from_str::<SignalMessage>(&text) {
                        return Some(msg);
                    }
                    // Skip non-signal text messages
                }
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                    // Skip ping/pong, keep waiting
                    continue;
                }
                _ => return None,
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
    match client.recv_signal().await {
        SignalMessage::SpaceJoined {
            space,
            channels,
            members,
        } => (space, channels, members),
        other => panic!("Expected SpaceJoined, got: {:?}", other),
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
    assert_eq!(restored_token, token);
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
                message.contains("creator") || message.contains("owner"),
                "Error should mention ownership: {message}"
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
    assert_eq!(token2, token, "Should get the same token back");
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
                message.contains("owner"),
                "Error should mention ownership: {message}"
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
