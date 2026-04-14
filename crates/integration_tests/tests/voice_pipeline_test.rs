//! Full voice pipeline integration test.
//!
//! Simulates two clients (Alice & Bob) joining a voice room and exchanging
//! Opus-encoded audio. Each client records what it hears to a WAV file for
//! manual verification. Tests both WebSocket and UDP transport paths.
//!
//! Run with:  cargo test -p integration_tests test_voice_pipeline -- --nocapture
#![allow(dead_code)] // Shared test infrastructure — not all helpers used in every test

use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::packet::Packet as OpusPacket;
use audiopus::{Application, Channels as OpusChannels, MutSignals, SampleRate as OpusSampleRate};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use shared_types::SignalMessage;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

// ─── Constants ───

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const SAMPLE_RATE: u32 = 48000;
const FRAME_SIZE: usize = 960; // 20ms at 48kHz
const OPUS_MAX_PACKET: usize = 4096;
const TEST_DURATION_FRAMES: usize = 100; // 2 seconds
const FRAME_INTERVAL_MS: u64 = 20;

// ─── Test Infrastructure (mirrors server_tests.rs) ───

struct TestServer {
    child: Child,
    port: u16,
    db_path: std::path::PathBuf,
}

impl TestServer {
    async fn start() -> Self {
        // Reserve both WS and UDP ports to prevent collisions in parallel tests
        let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = ws_listener.local_addr().unwrap().port();
        let udp_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let udp_port = udp_socket.local_addr().unwrap().port();
        drop(ws_listener);
        drop(udp_socket);

        let server_bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target/debug/signaling_server");
        let db_path = std::env::temp_dir().join(format!(
            "voxlink_voice_test_{}_{}.db",
            std::process::id(),
            port
        ));

        let child = Command::new(&server_bin)
            .env("PV_ADDR", format!("127.0.0.1:{port}"))
            .env("PV_UDP_PORT", udp_port.to_string())
            .env("PV_DB_PATH", &db_path)
            .env("RUST_LOG", "warn")
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to spawn signaling_server at {:?}: {}. Run `cargo build -p signaling_server` first.",
                    server_bin, e
                )
            });

        let server = TestServer {
            child,
            port,
            db_path,
        };

        let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!(
                    "Server did not start within {}s on port {port}",
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
                    "Could not connect WebSocket client within {}s",
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
                        match msg {
                            SignalMessage::SpaceAuditLogSnapshot { .. }
                            | SignalMessage::SpaceAuditLogAppended { .. } => continue,
                            other => return Some(other),
                        }
                    }
                }
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => continue,
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
}

// ─── Helpers ───

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

fn parse_audio_frame(frame: &[u8]) -> (&str, &[u8]) {
    assert!(
        frame.len() >= 3 && frame[0] == shared_types::MEDIA_PACKET_AUDIO,
        "Expected audio media packet, got len={} type={}",
        frame.len(),
        frame.first().copied().unwrap_or(0)
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

fn parse_screen_frame(frame: &[u8]) -> (&str, &[u8]) {
    assert!(
        frame.len() >= 3 && frame[0] == shared_types::MEDIA_PACKET_SCREEN,
        "Expected screen media packet, got len={} type={}",
        frame.len(),
        frame.first().copied().unwrap_or(0)
    );
    let id_len = frame[1] as usize;
    assert!(
        frame.len() > 2 + id_len,
        "Frame too short for sender header"
    );
    let sender_id = std::str::from_utf8(&frame[2..2 + id_len]).unwrap();
    let frame_data = &frame[2 + id_len..];
    (sender_id, frame_data)
}

fn parse_screen_chunk_frame(frame: &[u8]) -> (&str, shared_types::ScreenChunkMetadata, &[u8]) {
    assert!(
        frame.len() >= 3 && frame[0] == shared_types::MEDIA_PACKET_SCREEN_CHUNK,
        "Expected chunked screen media packet, got len={} type={}",
        frame.len(),
        frame.first().copied().unwrap_or(0)
    );
    let id_len = frame[1] as usize;
    assert!(
        frame.len() > 2 + id_len + shared_types::SCREEN_CHUNK_METADATA_LEN,
        "Chunk frame too short for sender header"
    );
    let sender_id = std::str::from_utf8(&frame[2..2 + id_len]).unwrap();
    let (metadata, payload) =
        shared_types::decode_screen_chunk_metadata(&frame[2 + id_len..]).unwrap();
    (sender_id, metadata, payload)
}

/// Generate `frame_count` frames of a sine wave tone at the given frequency.
/// Each frame is FRAME_SIZE (960) f32 samples, continuous phase across frames.
fn generate_tone_frames(freq_hz: f32, frame_count: usize) -> Vec<Vec<f32>> {
    let mut frames = Vec::with_capacity(frame_count);
    let mut phase = 0.0_f32;
    let phase_inc = 2.0 * std::f32::consts::PI * freq_hz / SAMPLE_RATE as f32;

    for _ in 0..frame_count {
        let mut frame = Vec::with_capacity(FRAME_SIZE);
        for _ in 0..FRAME_SIZE {
            frame.push(phase.sin() * 0.8); // 80% amplitude to avoid clipping
            phase += phase_inc;
            if phase > 2.0 * std::f32::consts::PI {
                phase -= 2.0 * std::f32::consts::PI;
            }
        }
        frames.push(frame);
    }
    frames
}

/// Encode a single f32 frame to Opus bytes.
/// Returns the number of encoded bytes.
fn encode_frame(
    encoder: &mut OpusEncoder,
    samples_f32: &[f32],
    out: &mut [u8; OPUS_MAX_PACKET],
) -> usize {
    // Convert f32 -> i16 (same path as audio_core)
    let mut pcm_i16 = [0i16; FRAME_SIZE];
    for (out_s, &s) in pcm_i16.iter_mut().zip(samples_f32.iter()) {
        *out_s = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
    }
    encoder.encode(&pcm_i16, out).expect("Opus encode failed")
}

/// Decode Opus bytes to i16 samples, then convert to f32.
fn decode_frame(decoder: &mut OpusDecoder, opus_data: &[u8]) -> Vec<f32> {
    let mut pcm_i16 = [0i16; FRAME_SIZE];
    let packet = OpusPacket::try_from(opus_data).expect("Invalid Opus packet");
    let output = MutSignals::try_from(&mut pcm_i16[..]).expect("MutSignals conversion failed");
    let n = decoder
        .decode(Some(packet), output, false)
        .expect("Opus decode failed");

    pcm_i16[..n].iter().map(|&s| s as f32 / 32767.0).collect()
}

/// Write f32 PCM samples to a WAV file (48kHz mono).
fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("Failed to create WAV writer");
    for &s in samples {
        writer.write_sample(s).expect("Failed to write WAV sample");
    }
    writer.finalize().expect("Failed to finalize WAV");
}

/// Decode a hex string of length 16 to [u8; 8].
fn hex_decode_8(hex: &str) -> [u8; 8] {
    assert_eq!(hex.len(), 16, "Expected 16 hex chars, got {}", hex.len());
    let mut out = [0u8; 8];
    for i in 0..8 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .unwrap_or_else(|_| panic!("Invalid hex at position {}", i * 2));
    }
    out
}

// ─── WebSocket Full-Duplex Voice Pipeline Test ───

#[tokio::test]
async fn test_voice_pipeline_full_duplex() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    // Setup room
    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;
    // Consume Alice's PeerJoined notification
    let _ = alice.recv_signal().await;

    println!("\n=== Voice Pipeline Test (WebSocket) ===");
    println!("Room: {room_code}");
    println!(
        "Duration: {}s ({TEST_DURATION_FRAMES} frames @ {FRAME_INTERVAL_MS}ms)",
        TEST_DURATION_FRAMES as f64 * FRAME_INTERVAL_MS as f64 / 1000.0
    );

    // Split clients for concurrent send/receive
    let alice_sink = Arc::new(Mutex::new(alice.sink));
    let alice_stream = Arc::new(Mutex::new(alice.stream));
    let bob_sink = Arc::new(Mutex::new(bob.sink));
    let bob_stream = Arc::new(Mutex::new(bob.stream));

    // Generate tone frames
    let alice_frames = generate_tone_frames(440.0, TEST_DURATION_FRAMES); // A4
    let bob_frames = generate_tone_frames(660.0, TEST_DURATION_FRAMES); // E5

    let start_time = Instant::now();

    // Alice sender task
    let alice_sink_clone = alice_sink.clone();
    let alice_sender = tokio::spawn(async move {
        let mut encoder = OpusEncoder::new(
            OpusSampleRate::Hz48000,
            OpusChannels::Mono,
            Application::Voip,
        )
        .expect("Failed to create Opus encoder");
        encoder
            .set_bitrate(audiopus::Bitrate::BitsPerSecond(64000))
            .ok();

        let mut interval = tokio::time::interval(Duration::from_millis(FRAME_INTERVAL_MS));
        let mut opus_buf = [0u8; OPUS_MAX_PACKET];
        let mut sent = 0u64;

        for frame in &alice_frames {
            interval.tick().await;
            let len = encode_frame(&mut encoder, frame, &mut opus_buf);
            let mut packet = Vec::with_capacity(len + 1);
            packet.push(shared_types::MEDIA_PACKET_AUDIO);
            packet.extend_from_slice(&opus_buf[..len]);
            let mut sink = alice_sink_clone.lock().await;
            sink.send(Message::Binary(packet.into())).await.ok();
            sent += 1;
        }
        sent
    });

    // Bob sender task
    let bob_sink_clone = bob_sink.clone();
    let bob_sender = tokio::spawn(async move {
        let mut encoder = OpusEncoder::new(
            OpusSampleRate::Hz48000,
            OpusChannels::Mono,
            Application::Voip,
        )
        .expect("Failed to create Opus encoder");
        encoder
            .set_bitrate(audiopus::Bitrate::BitsPerSecond(64000))
            .ok();

        let mut interval = tokio::time::interval(Duration::from_millis(FRAME_INTERVAL_MS));
        let mut opus_buf = [0u8; OPUS_MAX_PACKET];
        let mut sent = 0u64;

        for frame in &bob_frames {
            interval.tick().await;
            let len = encode_frame(&mut encoder, frame, &mut opus_buf);
            let mut packet = Vec::with_capacity(len + 1);
            packet.push(shared_types::MEDIA_PACKET_AUDIO);
            packet.extend_from_slice(&opus_buf[..len]);
            let mut sink = bob_sink_clone.lock().await;
            sink.send(Message::Binary(packet.into())).await.ok();
            sent += 1;
        }
        sent
    });

    // Bob receiver task (records Alice's audio)
    let bob_stream_clone = bob_stream.clone();
    let bob_receiver = tokio::spawn(async move {
        let mut decoder = OpusDecoder::new(OpusSampleRate::Hz48000, OpusChannels::Mono)
            .expect("Failed to create Opus decoder");
        let mut recorded_pcm: Vec<f32> = Vec::new();
        let mut received = 0u64;
        let mut first_frame_at: Option<Instant> = None;

        loop {
            let mut stream = bob_stream_clone.lock().await;
            match timeout(Duration::from_secs(4), stream.next()).await {
                Ok(Some(Ok(Message::Binary(data)))) => {
                    let data = data.to_vec();
                    drop(stream); // release lock before decoding
                    if data.len() >= 3 && data[0] == shared_types::MEDIA_PACKET_AUDIO {
                        let id_len = data[1] as usize;
                        if data.len() > 2 + id_len {
                            let opus_data = &data[2 + id_len..];
                            let pcm = decode_frame(&mut decoder, opus_data);
                            recorded_pcm.extend_from_slice(&pcm);
                            received += 1;
                            if first_frame_at.is_none() {
                                first_frame_at = Some(Instant::now());
                            }
                        }
                    }
                }
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                    drop(stream);
                    continue;
                }
                Ok(Some(Ok(Message::Text(_)))) => {
                    drop(stream);
                    continue; // skip signal messages
                }
                _ => {
                    drop(stream);
                    break; // timeout or error — done receiving
                }
            }
        }

        (recorded_pcm, received, first_frame_at)
    });

    // Alice receiver task (records Bob's audio)
    let alice_stream_clone = alice_stream.clone();
    let alice_receiver = tokio::spawn(async move {
        let mut decoder = OpusDecoder::new(OpusSampleRate::Hz48000, OpusChannels::Mono)
            .expect("Failed to create Opus decoder");
        let mut recorded_pcm: Vec<f32> = Vec::new();
        let mut received = 0u64;
        let mut first_frame_at: Option<Instant> = None;

        loop {
            let mut stream = alice_stream_clone.lock().await;
            match timeout(Duration::from_secs(4), stream.next()).await {
                Ok(Some(Ok(Message::Binary(data)))) => {
                    let data = data.to_vec();
                    drop(stream);
                    if data.len() >= 3 && data[0] == shared_types::MEDIA_PACKET_AUDIO {
                        let id_len = data[1] as usize;
                        if data.len() > 2 + id_len {
                            let opus_data = &data[2 + id_len..];
                            let pcm = decode_frame(&mut decoder, opus_data);
                            recorded_pcm.extend_from_slice(&pcm);
                            received += 1;
                            if first_frame_at.is_none() {
                                first_frame_at = Some(Instant::now());
                            }
                        }
                    }
                }
                Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                    drop(stream);
                    continue;
                }
                Ok(Some(Ok(Message::Text(_)))) => {
                    drop(stream);
                    continue;
                }
                _ => {
                    drop(stream);
                    break;
                }
            }
        }

        (recorded_pcm, received, first_frame_at)
    });

    // Wait for senders to finish
    let alice_sent = alice_sender.await.unwrap();
    let bob_sent = bob_sender.await.unwrap();

    // Wait for receivers to drain (they'll timeout after 4s of no data)
    let (bob_pcm, bob_received, bob_first) = bob_receiver.await.unwrap();
    let (alice_pcm, alice_received, alice_first) = alice_receiver.await.unwrap();

    let elapsed = start_time.elapsed();

    // Write WAV files
    let output_dir = std::env::temp_dir().join("voxlink_voice_test");
    std::fs::create_dir_all(&output_dir).ok();

    let bob_wav_path = output_dir.join("bob_hears_alice_ws.wav");
    let alice_wav_path = output_dir.join("alice_hears_bob_ws.wav");

    if !bob_pcm.is_empty() {
        write_wav(&bob_wav_path, &bob_pcm);
    }
    if !alice_pcm.is_empty() {
        write_wav(&alice_wav_path, &alice_pcm);
    }

    // Report
    let alice_loss = if alice_sent > 0 {
        (1.0 - bob_received as f64 / alice_sent as f64) * 100.0
    } else {
        100.0
    };
    let bob_loss = if bob_sent > 0 {
        (1.0 - alice_received as f64 / bob_sent as f64) * 100.0
    } else {
        100.0
    };

    println!("\n--- Results ---");
    println!("Total time: {:.1}s", elapsed.as_secs_f64());
    println!(
        "Alice -> Bob: {} sent, {} received ({:.1}% loss)",
        alice_sent, bob_received, alice_loss
    );
    println!(
        "Bob -> Alice: {} sent, {} received ({:.1}% loss)",
        bob_sent, alice_received, bob_loss
    );
    if let Some(t) = bob_first {
        println!(
            "First-frame latency (Alice->Bob): {:.0}ms",
            (t - start_time).as_millis()
        );
    }
    if let Some(t) = alice_first {
        println!(
            "First-frame latency (Bob->Alice): {:.0}ms",
            (t - start_time).as_millis()
        );
    }
    println!(
        "Bob recorded: {} samples ({:.1}s)",
        bob_pcm.len(),
        bob_pcm.len() as f64 / SAMPLE_RATE as f64
    );
    println!(
        "Alice recorded: {} samples ({:.1}s)",
        alice_pcm.len(),
        alice_pcm.len() as f64 / SAMPLE_RATE as f64
    );
    if !bob_pcm.is_empty() {
        println!("WAV: {}", bob_wav_path.display());
    }
    if !alice_pcm.is_empty() {
        println!("WAV: {}", alice_wav_path.display());
    }
    println!("=== End ===\n");

    // Assertions
    assert!(
        bob_received as f64 >= alice_sent as f64 * 0.9,
        "Bob should receive >= 90% of Alice's frames (got {bob_received}/{alice_sent})"
    );
    assert!(
        alice_received as f64 >= bob_sent as f64 * 0.9,
        "Alice should receive >= 90% of Bob's frames (got {alice_received}/{bob_sent})"
    );
    assert!(!bob_pcm.is_empty(), "Bob should have recorded audio");
    assert!(!alice_pcm.is_empty(), "Alice should have recorded audio");
    assert!(bob_wav_path.exists(), "Bob's WAV file should exist");
    assert!(alice_wav_path.exists(), "Alice's WAV file should exist");
}

// ─── UDP Voice Pipeline Test ───

#[tokio::test]
async fn test_voice_pipeline_udp() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    // Setup room
    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;
    let _ = alice.recv_signal().await; // PeerJoined

    // Request UDP for both clients
    alice.send_signal(&SignalMessage::RequestUdp).await;
    let alice_udp = match alice.recv_signal().await {
        SignalMessage::UdpReady { token, port } => (token, port),
        SignalMessage::UdpUnavailable => {
            println!("UDP unavailable on test server, skipping UDP test");
            return;
        }
        other => panic!("Expected UdpReady, got: {:?}", other),
    };

    bob.send_signal(&SignalMessage::RequestUdp).await;
    let bob_udp = match bob.recv_signal().await {
        SignalMessage::UdpReady { token, port } => (token, port),
        SignalMessage::UdpUnavailable => {
            println!("UDP unavailable on test server, skipping UDP test");
            return;
        }
        other => panic!("Expected UdpReady, got: {:?}", other),
    };

    println!("\n=== Voice Pipeline Test (UDP) ===");
    println!("Room: {room_code}");
    println!("Alice UDP token: {}, port: {}", alice_udp.0, alice_udp.1);
    println!("Bob UDP token: {}, port: {}", bob_udp.0, bob_udp.1);

    // Setup UDP sockets
    let alice_token = hex_decode_8(&alice_udp.0);
    let bob_token = hex_decode_8(&bob_udp.0);
    let udp_addr = format!("127.0.0.1:{}", alice_udp.1);

    let alice_udp_sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    alice_udp_sock.connect(&udp_addr).await.unwrap();
    // Send hello to register address
    alice_udp_sock.send(&alice_token).await.unwrap();

    let bob_udp_sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    bob_udp_sock.connect(&udp_addr).await.unwrap();
    bob_udp_sock.send(&bob_token).await.unwrap();

    // Small delay for server to register UDP addresses
    tokio::time::sleep(Duration::from_millis(100)).await;

    let alice_udp_sock = Arc::new(alice_udp_sock);
    let bob_udp_sock = Arc::new(bob_udp_sock);

    let alice_frames = generate_tone_frames(440.0, TEST_DURATION_FRAMES);
    let bob_frames = generate_tone_frames(660.0, TEST_DURATION_FRAMES);

    let start_time = Instant::now();

    // Alice UDP sender
    let alice_sock_tx = alice_udp_sock.clone();
    let alice_tok = alice_token;
    let alice_sender = tokio::spawn(async move {
        let mut encoder = OpusEncoder::new(
            OpusSampleRate::Hz48000,
            OpusChannels::Mono,
            Application::Voip,
        )
        .expect("Failed to create Opus encoder");
        encoder
            .set_bitrate(audiopus::Bitrate::BitsPerSecond(64000))
            .ok();

        let mut interval = tokio::time::interval(Duration::from_millis(FRAME_INTERVAL_MS));
        let mut opus_buf = [0u8; OPUS_MAX_PACKET];
        let mut sent = 0u64;

        for frame in &alice_frames {
            interval.tick().await;
            let len = encode_frame(&mut encoder, frame, &mut opus_buf);
            // UDP format: [token(8)][MEDIA_PACKET_AUDIO(1)][opus_data]
            let mut packet = Vec::with_capacity(8 + 1 + len);
            packet.extend_from_slice(&alice_tok);
            packet.push(shared_types::MEDIA_PACKET_AUDIO);
            packet.extend_from_slice(&opus_buf[..len]);
            alice_sock_tx.send(&packet).await.ok();
            sent += 1;
        }
        sent
    });

    // Bob UDP sender
    let bob_sock_tx = bob_udp_sock.clone();
    let bob_tok = bob_token;
    let bob_sender = tokio::spawn(async move {
        let mut encoder = OpusEncoder::new(
            OpusSampleRate::Hz48000,
            OpusChannels::Mono,
            Application::Voip,
        )
        .expect("Failed to create Opus encoder");
        encoder
            .set_bitrate(audiopus::Bitrate::BitsPerSecond(64000))
            .ok();

        let mut interval = tokio::time::interval(Duration::from_millis(FRAME_INTERVAL_MS));
        let mut opus_buf = [0u8; OPUS_MAX_PACKET];
        let mut sent = 0u64;

        for frame in &bob_frames {
            interval.tick().await;
            let len = encode_frame(&mut encoder, frame, &mut opus_buf);
            let mut packet = Vec::with_capacity(8 + 1 + len);
            packet.extend_from_slice(&bob_tok);
            packet.push(shared_types::MEDIA_PACKET_AUDIO);
            packet.extend_from_slice(&opus_buf[..len]);
            bob_sock_tx.send(&packet).await.ok();
            sent += 1;
        }
        sent
    });

    // Bob UDP receiver (records Alice's audio)
    let bob_sock_rx = bob_udp_sock.clone();
    let bob_receiver = tokio::spawn(async move {
        let mut decoder = OpusDecoder::new(OpusSampleRate::Hz48000, OpusChannels::Mono)
            .expect("Failed to create Opus decoder");
        let mut recorded_pcm: Vec<f32> = Vec::new();
        let mut received = 0u64;
        let mut buf = [0u8; 2048];

        loop {
            match timeout(Duration::from_secs(4), bob_sock_rx.recv(&mut buf)).await {
                Ok(Ok(len)) if len >= 3 => {
                    // Server relays as: [MEDIA_PACKET_AUDIO][id_len][sender_id][opus_data]
                    if buf[0] == shared_types::MEDIA_PACKET_AUDIO {
                        let id_len = buf[1] as usize;
                        if len > 2 + id_len {
                            let opus_data = &buf[2 + id_len..len];
                            let pcm = decode_frame(&mut decoder, opus_data);
                            recorded_pcm.extend_from_slice(&pcm);
                            received += 1;
                        }
                    }
                    // Also handle keepalive responses (0xFE) — just ignore
                }
                _ => break,
            }
        }

        (recorded_pcm, received)
    });

    // Alice UDP receiver (records Bob's audio)
    let alice_sock_rx = alice_udp_sock.clone();
    let alice_receiver = tokio::spawn(async move {
        let mut decoder = OpusDecoder::new(OpusSampleRate::Hz48000, OpusChannels::Mono)
            .expect("Failed to create Opus decoder");
        let mut recorded_pcm: Vec<f32> = Vec::new();
        let mut received = 0u64;
        let mut buf = [0u8; 2048];

        loop {
            match timeout(Duration::from_secs(4), alice_sock_rx.recv(&mut buf)).await {
                Ok(Ok(len)) if len >= 3 => {
                    if buf[0] == shared_types::MEDIA_PACKET_AUDIO {
                        let id_len = buf[1] as usize;
                        if len > 2 + id_len {
                            let opus_data = &buf[2 + id_len..len];
                            let pcm = decode_frame(&mut decoder, opus_data);
                            recorded_pcm.extend_from_slice(&pcm);
                            received += 1;
                        }
                    }
                }
                _ => break,
            }
        }

        (recorded_pcm, received)
    });

    // Wait for completion
    let alice_sent = alice_sender.await.unwrap();
    let bob_sent = bob_sender.await.unwrap();
    let (bob_pcm, bob_received) = bob_receiver.await.unwrap();
    let (alice_pcm, alice_received) = alice_receiver.await.unwrap();

    let elapsed = start_time.elapsed();

    // Write WAV files
    let output_dir = std::env::temp_dir().join("voxlink_voice_test");
    std::fs::create_dir_all(&output_dir).ok();

    let bob_wav_path = output_dir.join("bob_hears_alice_udp.wav");
    let alice_wav_path = output_dir.join("alice_hears_bob_udp.wav");

    if !bob_pcm.is_empty() {
        write_wav(&bob_wav_path, &bob_pcm);
    }
    if !alice_pcm.is_empty() {
        write_wav(&alice_wav_path, &alice_pcm);
    }

    // Report
    let alice_loss = if alice_sent > 0 {
        (1.0 - bob_received as f64 / alice_sent as f64) * 100.0
    } else {
        100.0
    };
    let bob_loss = if bob_sent > 0 {
        (1.0 - alice_received as f64 / bob_sent as f64) * 100.0
    } else {
        100.0
    };

    println!("\n--- Results ---");
    println!("Total time: {:.1}s", elapsed.as_secs_f64());
    println!(
        "Alice -> Bob (UDP): {} sent, {} received ({:.1}% loss)",
        alice_sent, bob_received, alice_loss
    );
    println!(
        "Bob -> Alice (UDP): {} sent, {} received ({:.1}% loss)",
        bob_sent, alice_received, bob_loss
    );
    println!(
        "Bob recorded: {} samples ({:.1}s)",
        bob_pcm.len(),
        bob_pcm.len() as f64 / SAMPLE_RATE as f64
    );
    println!(
        "Alice recorded: {} samples ({:.1}s)",
        alice_pcm.len(),
        alice_pcm.len() as f64 / SAMPLE_RATE as f64
    );
    if !bob_pcm.is_empty() {
        println!("WAV: {}", bob_wav_path.display());
    }
    if !alice_pcm.is_empty() {
        println!("WAV: {}", alice_wav_path.display());
    }
    println!("=== End ===\n");

    // Assertions (more lenient for UDP — localhost should still be ~100% though)
    assert!(
        bob_received as f64 >= alice_sent as f64 * 0.7,
        "Bob should receive >= 70% of Alice's UDP frames (got {bob_received}/{alice_sent})"
    );
    assert!(
        alice_received as f64 >= bob_sent as f64 * 0.7,
        "Alice should receive >= 70% of Bob's UDP frames (got {alice_received}/{bob_sent})"
    );
    assert!(!bob_pcm.is_empty(), "Bob should have recorded UDP audio");
    assert!(
        !alice_pcm.is_empty(),
        "Alice should have recorded UDP audio"
    );
}

#[tokio::test]
async fn test_screen_share_udp_relay() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;
    let _ = alice.recv_signal().await; // PeerJoined

    alice.send_signal(&SignalMessage::RequestUdp).await;
    let alice_udp = match alice.recv_signal().await {
        SignalMessage::UdpReady { token, port } => (token, port),
        SignalMessage::UdpUnavailable => panic!("UDP unavailable on test server"),
        other => panic!("Expected UdpReady, got: {:?}", other),
    };

    bob.send_signal(&SignalMessage::RequestUdp).await;
    let bob_udp = match bob.recv_signal().await {
        SignalMessage::UdpReady { token, port } => (token, port),
        SignalMessage::UdpUnavailable => panic!("UDP unavailable on test server"),
        other => panic!("Expected UdpReady, got: {:?}", other),
    };

    let alice_token = hex_decode_8(&alice_udp.0);
    let bob_token = hex_decode_8(&bob_udp.0);
    let udp_addr = format!("127.0.0.1:{}", alice_udp.1);

    let alice_udp_sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    alice_udp_sock.connect(&udp_addr).await.unwrap();
    let bob_udp_sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    bob_udp_sock.connect(&udp_addr).await.unwrap();

    // Register both UDP addresses with the relay before sending media.
    alice_udp_sock.send(&alice_token).await.unwrap();
    bob_udp_sock.send(&bob_token).await.unwrap();

    alice.send_signal(&SignalMessage::StartScreenShare).await;
    match alice.recv_signal().await {
        SignalMessage::ScreenShareStarted { .. } => {}
        other => panic!("Expected ScreenShareStarted for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStarted { .. } => {}
        other => panic!("Expected ScreenShareStarted for Bob, got: {:?}", other),
    }

    let payload = b"udp-screen-frame-001";
    let mut packet = Vec::with_capacity(8 + 1 + payload.len());
    packet.extend_from_slice(&alice_token);
    packet.push(shared_types::MEDIA_PACKET_SCREEN);
    packet.extend_from_slice(payload);
    alice_udp_sock.send(&packet).await.unwrap();

    let mut buf = vec![0u8; 2 + 256 + shared_types::MAX_UDP_MEDIA_PAYLOAD_SIZE];
    let len = timeout(Duration::from_secs(2), bob_udp_sock.recv(&mut buf))
        .await
        .expect("Timed out waiting for UDP screen frame")
        .expect("Failed to receive UDP screen frame");
    let (sender_id, frame_data) = parse_screen_frame(&buf[..len]);
    assert!(
        !sender_id.is_empty(),
        "UDP screen relay should include sender id"
    );
    assert_eq!(frame_data, payload);

    alice.send_signal(&SignalMessage::StopScreenShare).await;
    match alice.recv_signal().await {
        SignalMessage::ScreenShareStopped { .. } => {}
        other => panic!("Expected ScreenShareStopped for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStopped { .. } => {}
        other => panic!("Expected ScreenShareStopped for Bob, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_screen_share_udp_relay_chunked() {
    let server = TestServer::start().await;
    let mut alice = server.connect().await;
    let mut bob = server.connect().await;

    let room_code = create_room(&mut alice, "Alice").await;
    join_room(&mut bob, &room_code, "Bob").await;
    let _ = alice.recv_signal().await; // PeerJoined

    alice.send_signal(&SignalMessage::RequestUdp).await;
    let alice_udp = match alice.recv_signal().await {
        SignalMessage::UdpReady { token, port } => (token, port),
        SignalMessage::UdpUnavailable => panic!("UDP unavailable on test server"),
        other => panic!("Expected UdpReady, got: {:?}", other),
    };

    bob.send_signal(&SignalMessage::RequestUdp).await;
    let bob_udp = match bob.recv_signal().await {
        SignalMessage::UdpReady { token, port } => (token, port),
        SignalMessage::UdpUnavailable => panic!("UDP unavailable on test server"),
        other => panic!("Expected UdpReady, got: {:?}", other),
    };

    let alice_token = hex_decode_8(&alice_udp.0);
    let bob_token = hex_decode_8(&bob_udp.0);
    let udp_addr = format!("127.0.0.1:{}", alice_udp.1);

    let alice_udp_sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    alice_udp_sock.connect(&udp_addr).await.unwrap();
    let bob_udp_sock = tokio::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();
    bob_udp_sock.connect(&udp_addr).await.unwrap();

    alice_udp_sock.send(&alice_token).await.unwrap();
    bob_udp_sock.send(&bob_token).await.unwrap();

    alice.send_signal(&SignalMessage::StartScreenShare).await;
    match alice.recv_signal().await {
        SignalMessage::ScreenShareStarted { .. } => {}
        other => panic!("Expected ScreenShareStarted for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStarted { .. } => {}
        other => panic!("Expected ScreenShareStarted for Bob, got: {:?}", other),
    }

    let payload = vec![0x5A; shared_types::MAX_UDP_MEDIA_PAYLOAD_SIZE + 2048];
    let sequence = 1u32;
    let chunk_count = payload
        .len()
        .div_ceil(shared_types::MAX_UDP_SCREEN_CHUNK_SIZE);
    for (chunk_index, chunk) in payload
        .chunks(shared_types::MAX_UDP_SCREEN_CHUNK_SIZE)
        .enumerate()
    {
        let header = shared_types::encode_screen_chunk_metadata(
            sequence,
            chunk_index as u16,
            chunk_count as u16,
        );
        let mut packet = Vec::with_capacity(8 + 1 + header.len() + chunk.len());
        packet.extend_from_slice(&alice_token);
        packet.push(shared_types::MEDIA_PACKET_SCREEN_CHUNK);
        packet.extend_from_slice(&header);
        packet.extend_from_slice(chunk);
        alice_udp_sock.send(&packet).await.unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut received = vec![None; chunk_count];
    while received.iter().any(|chunk| chunk.is_none()) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let mut buf = vec![0u8; 2 + 256 + shared_types::MAX_UDP_MEDIA_PAYLOAD_SIZE];
        let len = timeout(remaining, bob_udp_sock.recv(&mut buf))
            .await
            .expect("Timed out waiting for UDP screen chunks")
            .expect("Failed to receive UDP screen chunk");
        let (sender_id, metadata, chunk_data) = parse_screen_chunk_frame(&buf[..len]);
        assert!(
            !sender_id.is_empty(),
            "Chunked UDP screen relay should include sender id"
        );
        assert_eq!(metadata.sequence, sequence);
        received[metadata.chunk_index as usize] = Some(chunk_data.to_vec());
    }

    let mut reassembled = Vec::with_capacity(payload.len());
    for chunk in received {
        reassembled.extend_from_slice(&chunk.expect("missing relayed screen chunk"));
    }
    assert_eq!(reassembled, payload);

    alice.send_signal(&SignalMessage::StopScreenShare).await;
    match alice.recv_signal().await {
        SignalMessage::ScreenShareStopped { .. } => {}
        other => panic!("Expected ScreenShareStopped for Alice, got: {:?}", other),
    }
    match bob.recv_signal().await {
        SignalMessage::ScreenShareStopped { .. } => {}
        other => panic!("Expected ScreenShareStopped for Bob, got: {:?}", other),
    }
}
