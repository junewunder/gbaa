//! `gbaa-server [ADDR] [--min-version N]`: the netplay server, plain-UDP
//! rooms for rollback netplay ([`relay::rooms`]) on one port (default
//! 0.0.0.0:7374). ADDR may be HOST:PORT (deploy/fly/ runs it on fly.io as
//! `fly-global-services:7374`). Players need only its address.
//! `--min-version N` refuses clients older than netplay version N
//! (`netplay::VERSION`), telling them to get the new build; without it,
//! every version is let in.
//!
//! `gbaa-server --rooms ADDR` asks a running server how many rooms are
//! open and how many players are in them ([`relay::rooms::query`]).

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

const USAGE: &str = "usage: gbaa-server [ADDR] [--min-version N]  (default 0.0.0.0:7374)\n       gbaa-server --rooms ADDR";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [flag, addr] if flag == "--rooms" => rooms(addr),
        _ => server_args(&args).and_then(|(addr, min)| serve(&addr, min)),
    };
    if let Err(e) = result {
        eprintln!("gbaa-server: {e}");
        std::process::exit(1);
    }
}

/// Print how many rooms the server at `addr` (port 7374 if none given)
/// has open.
fn rooms(addr: &str) -> Result<(), String> {
    let a = resolve(&match addr.contains(':') {
        true => addr.to_string(),
        false => format!("{addr}:{}", relay::PORT),
    })?;
    let (rooms, players) = relay::rooms::query(a).map_err(|e| format!("{addr}: {e}"))?;
    println!("{rooms} rooms open, {players} players in them");
    Ok(())
}

/// ADDR and the minimum version from `gbaa-server [ADDR] [--min-version N]`.
fn server_args(args: &[String]) -> Result<(String, u32), String> {
    let mut addr = None;
    let mut min = 0;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--min-version" => {
                let v = it.next().ok_or(USAGE)?;
                min = v
                    .parse()
                    .map_err(|_| format!("--min-version {v}?\n{USAGE}"))?;
            }
            a if !a.starts_with('-') && addr.is_none() => addr = Some(a.to_string()),
            _ => return Err(USAGE.into()),
        }
    }
    let addr = addr.unwrap_or_else(|| format!("0.0.0.0:{}", relay::PORT));
    Ok((addr, min))
}

fn resolve(addr: &str) -> Result<SocketAddr, String> {
    match addr.parse() {
        Ok(a) => Ok(a),
        Err(_) => addr
            .to_socket_addrs()
            .map_err(|e| format!("{addr}: {e}"))?
            .next()
            .ok_or_else(|| format!("{addr}: no address")),
    }
}

/// `addr` may name a host (`fly-global-services:7374`, the address fly.io
/// routes a public UDP port to): its first address is bound.
fn serve(addr: &str, min_version: u32) -> Result<(), String> {
    let addr = resolve(addr)?;
    let sock = UdpSocket::bind(addr).map_err(|e| format!("{addr}: {e}"))?;
    match min_version {
        0 => eprintln!("gbaa-server: rooms on {addr} (UDP), every version let in"),
        v => eprintln!("gbaa-server: rooms on {addr} (UDP), netplay version {v} and newer"),
    }
    relay::rooms::serve(sock, min_version).map_err(|e| format!("rooms stopped: {e}"))
}
