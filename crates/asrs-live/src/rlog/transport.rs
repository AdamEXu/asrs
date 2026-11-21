//! What `rlogMain` needs from Node's `net`: `createConnection`, `write`, `destroy` and
//! `setTimeout`, with the socket's own rules for them: writes while connecting are held and
//! go out on connect, a refused connection reports `Error` then `Close`, `Close` reports
//! once, and the `setTimeout` idle timer reports `Timeout` once per `set_timeout` call while
//! connecting, reads and writes count as activity.

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
    /// `'timeout'`: the `set_timeout` idle timer fired once after `ms` without activity.
    Timeout(TcpId),
}

/// Node `net.Socket`, reduced to what `main.ts` uses.
pub trait RlogTransport {
    /// `net.createConnection({host, port})`: the socket starts connecting.
    fn tcp_connect(&mut self, host: &str, port: f64) -> TcpId;

    /// `socket.write(data)`: buffered until connect, then written.
    fn tcp_write(&mut self, id: TcpId, data: &[u8]);

    /// `socket.destroy()`: closes immediately and reports `Close` once.
    fn tcp_destroy(&mut self, id: TcpId);

    /// `socket.setTimeout(ms, callback)`: after `ms` without connecting, reads or writes the
    /// socket reports `Timeout`; any activity re-arms it. The callback is a `once` listener,
    /// so only the first fire after this call is delivered.
    fn tcp_set_timeout(&mut self, id: TcpId, ms: f64);
}
