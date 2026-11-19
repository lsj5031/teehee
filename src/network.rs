//! `network` — thin UDP socket adapters for teehee audio packets.
//!
//! [`Sender`] speaks to a specific target (created via
//! [`Sender::connect`]); [`Receiver`] listens on a local address (via
//! [`Receiver::bind`]). Both wrap a single [`std::net::UdpSocket`] and
//! expose a narrow, blocking API. The `protocol` module owns framing
//! and serialization; the `network` module owns transport.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};

/// Send UDP datagrams to a single target.
///
/// Constructed via [`Sender::connect`], which both binds a local
/// ephemeral port and connects to the target. `send` is a thin wrapper
/// around [`UdpSocket::send`] and is therefore non-blocking in
/// practice on Linux for typical datagram sizes.
pub struct Sender {
    socket: UdpSocket,
}

impl Sender {
    /// Bind an ephemeral local socket and connect it to `target`.
    ///
    /// `target` is resolved via [`std::net::ToSocketAddrs`], so a
    /// `SocketAddr`, `"host:port"`, or `"[::1]:port"` all work.
    /// IPv4 addresses are tried first to preserve dual-stack hostname behavior.
    pub fn connect(target: impl ToSocketAddrs) -> io::Result<Self> {
        let mut addresses: Vec<_> = target.to_socket_addrs()?.collect();
        addresses.sort_by_key(SocketAddr::is_ipv6);

        let mut last_error = None;
        for addr in addresses {
            let local_ip = match addr {
                SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            };
            match UdpSocket::bind((local_ip, 0)).and_then(|socket| {
                socket.connect(addr)?;
                Ok(Self { socket })
            }) {
                Ok(sender) => return Ok(sender),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any addresses",
            )
        }))
    }

    /// Send `payload` as a single UDP datagram. Returns the number of
    /// bytes written. A zero-length `payload` is a no-op (sends an
    /// empty datagram) — teehee callers should never send zero bytes,
    /// but the protocol contract keeps it well-defined.
    pub fn send(&self, payload: &[u8]) -> io::Result<usize> {
        self.socket.send(payload)
    }

    /// Underlying local address (assigned by the OS at bind time).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

/// Receive UDP datagrams on a bound local socket.
///
/// Constructed via [`Receiver::bind`]. `recv` blocks until a datagram
/// arrives or the socket is closed (returning `Ok(None)` in the
/// closed case so the application can shut down cleanly).
pub struct Receiver {
    socket: UdpSocket,
}

impl Receiver {
    /// Bind to `addr` (e.g. `127.0.0.1:5000` or `[::]:5000`).
    pub fn bind(addr: impl ToSocketAddrs) -> io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        Ok(Self { socket })
    }

    /// Block waiting for the next datagram. Returns the number of bytes
    /// written to `buf`. I/O errors (including `ConnectionReset` from a
    /// prior send hitting an unreachable host) are surfaced verbatim
    /// so the application sees real diagnostic information.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.socket.recv(buf)
    }

    /// Receive with a timeout. Returns:
    ///
    /// * `Ok(Some(n))` — datagram received.
    /// * `Ok(None)` — timeout elapsed with no data.
    /// * `Err(_)` — I/O error.
    ///
    /// Use this in a poll loop driven by a stop flag for clean
    /// shutdown of receiver threads.
    pub fn recv_timeout(
        &self,
        buf: &mut [u8],
        timeout: std::time::Duration,
    ) -> io::Result<Option<usize>> {
        self.socket.set_read_timeout(Some(timeout))?;
        match self.socket.recv(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Underlying local address. Useful for sharing with a connected
    /// `Sender`.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Drop the socket, unblocking any in-flight `recv` calls.
    pub fn close(self) {
        // Dropping closes the socket; explicit method for symmetry
        // with `Sender` and to make shutdown paths obvious at call sites.
        drop(self);
    }
}
