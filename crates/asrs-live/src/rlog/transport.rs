//! What `rlogMain` needs from Node's `net`: `createConnection`, `write` and `destroy`, with the
//! socket's own rules for them: writes while connecting are held and go out on connect, a refused
//! connection reports `Error` then `Close`, and `Close` reports once. The `setTimeout` idle timer
//! is [`super::RlogMain`]'s: Node runs it on the main process's event loop with every other timer.

use asrs_js::number;

use crate::thrown::Thrown;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TcpId(pub u32);

/// A socket event for the main-process emulation, in dispatch order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RlogNetEvent {
    /// `'connect'`: the peer accepted the connection.
    Connect(TcpId),
    /// `'data'`: a received chunk.
    Data(TcpId, Vec<u8>),
    /// `'error'`: a refused connection; `Close` follows as a later task.
    Error(TcpId),
    /// `'close'`: dropped by the peer, destroyed locally, or ended after an error.
    Close(TcpId),
}

/// Node's `validatePort` on a number: anything else than an integer from 0 to 65535 throws before
/// a socket exists.
pub fn socket_port(port: f64) -> Result<u16, Thrown> {
    if (0.0..=65535.0).contains(&port) && port.fract() == 0.0 {
        return Ok(port as u16);
    }
    Err(Thrown::error(
        "RangeError [ERR_SOCKET_BAD_PORT]",
        format!(
            "Port should be >= 0 and < 65536. Received type number ({}).",
            number::to_string(port)
        ),
    ))
}

/// Node `net.Socket`, reduced to what `main.ts` uses.
pub trait RlogTransport {
    /// `net.createConnection({host, port})`: the socket starts connecting, or Node throws on the
    /// port.
    fn tcp_connect(&mut self, host: &str, port: f64) -> Result<TcpId, Thrown>;

    /// `socket.write(data)`: buffered until connect, then written.
    fn tcp_write(&mut self, id: TcpId, data: &[u8]);

    /// `socket.destroy()`: closes immediately and reports `Close` once.
    fn tcp_destroy(&mut self, id: TcpId);
}
