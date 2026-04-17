use tokio::net::UdpSocket;

pub(crate) async fn run_discovery(server_addr: String) {
    let socket = match UdpSocket::bind("0.0.0.0:9092").await {
        Ok(s) => s,
        Err(e) => {
            log::warn!("LAN discovery unavailable: {e}");
            return;
        }
    };
    if socket.set_broadcast(true).is_err() {
        log::warn!("Could not enable UDP broadcast");
        return;
    }

    log::info!("LAN discovery listening on UDP 9092");
    let mut buf = [0u8; 64];
    loop {
        if let Ok((len, src)) = socket.recv_from(&mut buf).await {
            if len >= 16 && &buf[..16] == b"VOXLINK_DISCOVER" {
                let response = format!("VOXLINK_SERVER:{}", server_addr);
                let _ = socket.send_to(response.as_bytes(), src).await;
                log::info!("Discovery response sent to {src}");
            }
        }
    }
}
