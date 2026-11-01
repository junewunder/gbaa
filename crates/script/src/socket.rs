//! mGBA's native `_socket` (`src/script/socket.c` at 0.10.5) over
//! `std::net`: non-blocking TCP, with mGBA's `C.SOCKERR` codes. The
//! `socket` API scripts use is the prelude's, over this, as in mGBA.
//!
//! `std` binds and listens in one step, so `open` (the prelude's `bind`)
//! listens and `listen` only reports whether it is. A listener's `select`
//! accepts a waiting client and keeps it for the next `accept`.
//!
//! A sandboxed script's sockets ([`crate::Access`]) reach loopback only:
//! binding to any interface (mGBA's `nil` address) binds 127.0.0.1, and any
//! other address that isn't loopback is refused with `DENIED`.

use mlua::{UserData, UserDataFields, UserDataMethods};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// `enum mSocketErrorCode` (`include/mgba/internal/script/socket.h`), by
/// name, for `C.SOCKERR`.
pub const SOCKERR: &[(&str, i32)] = &[
    ("UNKNOWN_ERROR", UNKNOWN),
    ("OK", OK),
    ("AGAIN", AGAIN),
    ("ADDRESS_IN_USE", 2),
    ("CONNECTION_REFUSED", 3),
    ("DENIED", DENIED),
    ("FAILED", 5),
    ("NETWORK_UNREACHABLE", 6),
    ("NOT_FOUND", NOT_FOUND),
    ("NO_DATA", 8),
    ("OUT_OF_MEMORY", 9),
    ("TIMEOUT", 10),
    ("UNSUPPORTED", 11),
];
const UNKNOWN: i32 = -1;
const OK: i32 = 0;
const AGAIN: i32 = 1;
const DENIED: i32 = 4;
const NOT_FOUND: i32 = 7;

/// How long `connect` may block (mGBA's blocks too, as long as the OS lets it).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// mGBA's code for an OS error (`_mScriptSocketErrorMappings`).
fn code(e: &std::io::Error) -> i32 {
    match e.kind() {
        ErrorKind::WouldBlock => AGAIN,
        ErrorKind::AddrInUse => 2,
        ErrorKind::ConnectionRefused => 3,
        ErrorKind::PermissionDenied => DENIED,
        ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable => 6,
        ErrorKind::TimedOut => 10,
        ErrorKind::InvalidInput | ErrorKind::Unsupported => 11,
        ErrorKind::OutOfMemory => 9,
        _ => UNKNOWN,
    }
}

enum Sock {
    None,
    /// With a client `select` accepted, for the next `accept`.
    Listener(TcpListener, Option<TcpStream>),
    Stream(TcpStream),
}

/// `_socket.create()`'s socket.
pub struct Socket {
    sock: Sock,
    error: i32,
    /// Any address; else loopback only (sandboxed).
    full: bool,
}

impl Socket {
    pub fn new(full: bool) -> Socket {
        Socket {
            sock: Sock::None,
            error: OK,
            full,
        }
    }

    /// Set the error from `r`'s, and return the status `open`, `connect`
    /// and `listen` give.
    fn status<T>(&mut self, r: Result<T, i32>) -> Result<T, i32> {
        self.error = match &r {
            Ok(_) => OK,
            Err(e) => *e,
        };
        r
    }

    /// `address:port` resolved, keeping only what this socket may reach.
    fn resolve(&self, address: &str, port: u16) -> Result<Vec<SocketAddr>, i32> {
        let all: Vec<SocketAddr> = (address, port)
            .to_socket_addrs()
            .map_err(|_| NOT_FOUND)?
            .collect();
        if all.is_empty() {
            return Err(NOT_FOUND);
        }
        let ok: Vec<SocketAddr> = all
            .into_iter()
            .filter(|a| self.full || a.ip().is_loopback())
            .collect();
        if ok.is_empty() {
            return Err(DENIED);
        }
        Ok(ok)
    }

    fn open(&mut self, address: &str, port: u16) -> i32 {
        let r = if address.is_empty() {
            let any = if self.full { "0.0.0.0" } else { "127.0.0.1" };
            Ok(vec![SocketAddr::new(any.parse().unwrap(), port)])
        } else {
            self.resolve(address, port)
        };
        let r = r.and_then(|addrs| {
            let l = TcpListener::bind(&addrs[..]).map_err(|e| code(&e))?;
            l.set_nonblocking(true).map_err(|e| code(&e))?;
            Ok(l)
        });
        match self.status(r) {
            Ok(l) => {
                self.sock = Sock::Listener(l, None);
                OK
            }
            Err(e) => e,
        }
    }

    fn connect(&mut self, address: &str, port: u16) -> i32 {
        let r = self.resolve(address, port).and_then(|addrs| {
            let mut last = UNKNOWN;
            for a in addrs {
                match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
                    Ok(s) => {
                        s.set_nonblocking(true).map_err(|e| code(&e))?;
                        return Ok(s);
                    }
                    Err(e) => last = code(&e),
                }
            }
            Err(last)
        });
        match self.status(r) {
            Ok(s) => {
                self.sock = Sock::Stream(s);
                OK
            }
            Err(e) => e,
        }
    }

    fn listen(&mut self) -> i32 {
        let r = match self.sock {
            Sock::Listener(..) => Ok(()),
            _ => Err(UNKNOWN),
        };
        match self.status(r) {
            Ok(()) => OK,
            Err(e) => e,
        }
    }

    /// A waiting client, if there is one.
    fn take_client(&mut self) -> Result<TcpStream, i32> {
        let Sock::Listener(l, pending) = &mut self.sock else {
            return Err(UNKNOWN);
        };
        if let Some(s) = pending.take() {
            return Ok(s);
        }
        let (s, _) = l.accept().map_err(|e| code(&e))?;
        s.set_nonblocking(true).map_err(|e| code(&e))?;
        Ok(s)
    }

    fn accept(&mut self) -> Socket {
        let mut client = Socket::new(self.full);
        match self.take_client() {
            Ok(s) => client.sock = Sock::Stream(s),
            Err(e) => client.error = e,
        }
        client
    }

    fn send(&mut self, data: &[u8]) -> i64 {
        let r = match &mut self.sock {
            Sock::Stream(s) => s.write(data).map_err(|e| code(&e)),
            _ => Err(UNKNOWN),
        };
        match self.status(r) {
            Ok(n) => n as i64,
            Err(e) => -(e as i64),
        }
    }

    /// Up to `max` bytes; none, with the error set, if none came (none,
    /// with no error, when the other end closed).
    fn recv(&mut self, max: usize) -> Vec<u8> {
        let mut buf = vec![0; max];
        let r = match &mut self.sock {
            Sock::Stream(s) => s.read(&mut buf).map_err(|e| code(&e)),
            _ => Err(UNKNOWN),
        };
        buf.truncate(self.status(r).unwrap_or(0));
        buf
    }

    /// 1 if there is something to read (or a client to accept), 0 if not,
    /// -1 on an error. Never waits: mGBA's prelude only polls.
    fn select(&mut self) -> i32 {
        let r = match &mut self.sock {
            Sock::Listener(_, Some(_)) => Ok(true),
            Sock::Listener(..) => match self.take_client() {
                Ok(s) => {
                    if let Sock::Listener(_, pending) = &mut self.sock {
                        *pending = Some(s);
                    }
                    Ok(true)
                }
                Err(AGAIN) => Ok(false),
                Err(e) => Err(e),
            },
            // A closed connection reads as ready: `recv` then says so.
            Sock::Stream(s) => match s.peek(&mut [0]) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(false),
                Err(e) => Err(code(&e)),
            },
            Sock::None => Err(UNKNOWN),
        };
        match r {
            Ok(ready) => ready as i32,
            Err(e) => {
                self.error = e;
                -1
            }
        }
    }
}

impl UserData for Socket {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("error", |_, s| Ok(s.error));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method_mut("close", |_, s, ()| {
            s.sock = Sock::None;
            Ok(())
        });
        m.add_method_mut("accept", |_, s, ()| Ok(s.accept()));
        m.add_method_mut("open", |_, s, (a, p): (Option<String>, u16)| {
            Ok(s.open(a.as_deref().unwrap_or(""), p))
        });
        m.add_method_mut("connect", |_, s, (a, p): (Option<String>, u16)| {
            Ok(s.connect(a.as_deref().unwrap_or(""), p))
        });
        m.add_method_mut("listen", |_, s, _: Option<u32>| Ok(s.listen()));
        m.add_method_mut("send", |_, s, d: mlua::LuaString| Ok(s.send(&d.as_bytes())));
        m.add_method_mut("recv", |lua, s, max: u32| {
            lua.create_string(s.recv(max as usize))
        });
        m.add_method_mut("select", |_, s, _: Option<i64>| Ok(s.select()));
    }
}
