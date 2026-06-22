//! Integration tests for the `network` module — exercise the public
//! `Sender` / `Receiver` interface through real loopback UDP sockets.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use teehee::network::{Receiver, Sender};

fn loopback_any_port() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
}

#[test]
fn sender_to_receiver_round_trips_a_small_packet() {
    let rx = Receiver::bind(loopback_any_port()).expect("bind receiver");
    let rx_addr = rx.local_addr().expect("receiver local_addr");
    let tx = Sender::connect(rx_addr).expect("connect sender");

    let payload = b"teehee v1 roundtrip";
    let n = tx.send(payload).expect("send");
    assert_eq!(n, payload.len());

    let mut buf = [0u8; 2048];
    let m = rx.recv(&mut buf).expect("recv");
    assert_eq!(m, payload.len());
    assert_eq!(&buf[..m], payload);
}

#[test]
fn sender_reports_local_addr_used_for_sending() {
    let rx = Receiver::bind(loopback_any_port()).unwrap();
    let rx_addr = rx.local_addr().unwrap();
    let tx = Sender::connect(rx_addr).unwrap();
    // connect() bound the sender to some local port.
    let tx_local = tx.local_addr().unwrap();
    assert!(
        tx_local.ip().is_loopback() || tx_local.ip().is_unspecified(),
        "sender local IP should be loopback, got {}",
        tx_local.ip()
    );
    assert_ne!(tx_local.port(), 0, "sender port should be assigned");
}

#[test]
fn multiple_packets_arrive_in_send_order_on_loopback() {
    let rx = Receiver::bind(loopback_any_port()).unwrap();
    let rx_addr = rx.local_addr().unwrap();
    let tx = Sender::connect(rx_addr).unwrap();

    for i in 0u32..5 {
        let payload = i.to_le_bytes();
        let n = tx.send(&payload).unwrap();
        assert_eq!(n, payload.len());
    }

    let mut buf = [0u8; 4];
    for i in 0u32..5 {
        let m = rx.recv(&mut buf).expect("recv");
        assert_eq!(m, 4);
        assert_eq!(u32::from_le_bytes(buf), i);
    }
}

#[test]
fn recv_timeout_returns_none_when_no_data_arrives() {
    // No sender is connected: the receiver should time out cleanly
    // with `Ok(None)` rather than hanging. This pins the shutdown
    // path used by the localhost smoke test.
    let rx = Receiver::bind(loopback_any_port()).unwrap();
    let mut buf = [0u8; 64];
    let result = rx
        .recv_timeout(&mut buf, std::time::Duration::from_millis(50))
        .expect("recv_timeout must not error on timeout");
    assert!(
        result.is_none(),
        "expected timeout (None), got {:?}",
        result
    );
}

#[test]
fn recv_timeout_returns_some_after_send_within_window() {
    let rx = Receiver::bind(loopback_any_port()).unwrap();
    let rx_addr = rx.local_addr().unwrap();
    let tx = Sender::connect(rx_addr).unwrap();

    // Send from a separate thread so the recv_timeout has a window.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        tx.send(b"teehee").unwrap();
    });

    let mut buf = [0u8; 64];
    let result = rx
        .recv_timeout(&mut buf, std::time::Duration::from_millis(500))
        .expect("recv_timeout ok");
    assert_eq!(result, Some(6));
    assert_eq!(&buf[..6], b"teehee");
}

#[test]
fn sender_send_rejects_zero_length_datagrams() {
    // A zero-byte UDP datagram is permitted by the protocol but we
    // contract that no caller should ever produce one — the protocol
    // always ships at least one PCM sample. Sending zero bytes must
    // be a no-op (or surfaced as a no-op for v1).
    let rx = Receiver::bind(loopback_any_port()).unwrap();
    let rx_addr = rx.local_addr().unwrap();
    let tx = Sender::connect(rx_addr).unwrap();
    let result = tx.send(&[]);
    assert!(result.is_ok(), "zero-length send should not error");
    assert_eq!(result.unwrap(), 0);
}
