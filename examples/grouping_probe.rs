//! Prove that devices group by account and that the device list respects it.
//!
//! Registers several devices against a test server, each carrying a different
//! account token, then asks for the roster three ways:
//!
//!   * with account A's token  -> only A's devices
//!   * with account B's token  -> only B's devices
//!   * with no token           -> everything (what a signed-out client sees, and
//!                                what every client saw before accounts existed)
//!
//! The no-token case is the one worth guarding: narrowing it would silently
//! empty the panel for anyone who has not signed in.
//!
//! Run: cargo run --release --features hwcodec,flutter --example grouping_probe \
//!        -- <host:port> <key> <tokenA> <tokenB>
use hbb_common::{
    protobuf::Message as _, rendezvous_proto::*, tcp::FramedStream, tokio, udp::FramedSocket,
};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        println!("usage: grouping_probe <host:port> <key> <tokenA> <tokenB>");
        std::process::exit(2);
    }
    let (addr, key, ta, tb) = (a[1].clone(), a[2].clone(), a[3].clone(), a[4].clone());

    // Two devices for A, one for B, one signed out.
    for (id, token, who) in [
        ("aaa111111", ta.as_str(), "A"),
        ("aaa222222", ta.as_str(), "A"),
        ("bbb333333", tb.as_str(), "B"),
        ("ccc444444", "", "nobody"),
    ] {
        register(&addr, id, token).await;
        println!("registered {id} as {who}");
    }
    // Registration binds the account asynchronously; give it a moment to land.
    tokio::time::sleep(Duration::from_millis(800)).await;

    println!("\n--- roster with A's token ---");
    let list_a = list(&addr, "zzz000000", &key, &ta).await;
    show(&list_a);
    println!("--- roster with B's token ---");
    let list_b = list(&addr, "zzz000000", &key, &tb).await;
    show(&list_b);
    println!("--- roster with no token ---");
    let list_all = list(&addr, "zzz000000", &key, "").await;
    show(&list_all);

    let ids = |v: &Vec<PeerBrief>| -> Vec<String> { v.iter().map(|p| p.id.clone()).collect() };
    let a_ids = ids(&list_a);
    let b_ids = ids(&list_b);
    let all_ids = ids(&list_all);

    let mut bad = false;
    if !(a_ids.contains(&"aaa111111".into()) && a_ids.contains(&"aaa222222".into())) {
        println!("FAIL: A's own devices missing from A's roster");
        bad = true;
    }
    if a_ids.iter().any(|i| i.starts_with("bbb") || i.starts_with("ccc")) {
        println!("FAIL: A can see devices that are not in A's account");
        bad = true;
    }
    if !b_ids.contains(&"bbb333333".into()) || b_ids.len() != 1 {
        println!("FAIL: B's roster is wrong: {b_ids:?}");
        bad = true;
    }
    if !(all_ids.contains(&"aaa111111".into())
        && all_ids.contains(&"bbb333333".into())
        && all_ids.contains(&"ccc444444".into()))
    {
        println!("FAIL: signed-out roster should still show everything");
        bad = true;
    }
    if bad {
        std::process::exit(1);
    }
    println!("\nOK: devices group by account, and a signed-out client still sees all");
}

fn show(v: &Vec<PeerBrief>) {
    if v.is_empty() {
        println!("  (empty)");
    }
    for p in v {
        println!("  {}  online={}", p.id, p.online);
    }
}

/// RegisterPk travels over **UDP** -- the server answers it from the udp socket,
/// not the TCP listener. Sending it over TCP silently goes nowhere.
async fn register(addr: &str, id: &str, token: &str) {
    let bind: std::net::SocketAddr = "0.0.0.0:0".parse().unwrap();
    let Ok(mut s) = FramedSocket::new(bind).await else {
        println!("cannot open udp socket");
        std::process::exit(1);
    };
    let mut msg = RendezvousMessage::new();
    msg.set_register_pk(RegisterPk {
        id: id.to_owned(),
        // Deterministic but distinct per id, so each device is its own row.
        uuid: id.as_bytes().to_vec().into(),
        pk: id.as_bytes().to_vec().into(),
        hostname: format!("host-{id}"),
        platform: "Linux".to_owned(),
        account_token: token.to_owned(),
        ..Default::default()
    });
    let target: std::net::SocketAddr = addr.parse().expect("host:port");
    let _ = s.send(&msg, target).await;
    let _ = hbb_common::timeout(1000, s.next()).await;
}

async fn list(addr: &str, me: &str, key: &str, token: &str) -> Vec<PeerBrief> {
    // The list endpoint lives on port - 1, same as the online-state query.
    //
    // Note the host must NOT be loopback: handle_listener2 routes connections
    // from 127.0.0.1 to the console command handler, so a protocol message from
    // there is simply never parsed. Pass the machine's LAN address instead.
    let (host, port) = addr.rsplit_once(':').unwrap();
    let nat: u16 = port.parse::<u16>().unwrap() - 1;
    let Ok(mut s) = FramedStream::new(format!("{host}:{nat}"), None, 1000).await else {
        println!("cannot reach list port");
        std::process::exit(1);
    };
    let mut msg = RendezvousMessage::new();
    msg.set_list_peers_request(ListPeersRequest {
        id: me.to_owned(),
        licence_key: key.to_owned(),
        account_token: token.to_owned(),
        ..Default::default()
    });
    let _ = s.send(&msg).await;
    for _ in 0..3 {
        if let Some(Ok(bytes)) = hbb_common::timeout(2000, s.next()).await.unwrap_or(None) {
            if let Ok(m) = RendezvousMessage::parse_from_bytes(&bytes) {
                if let Some(rendezvous_message::Union::ListPeersResponse(r)) = m.union {
                    if !r.error.is_empty() {
                        println!("server refused: {}", r.error);
                        std::process::exit(1);
                    }
                    return r.peers;
                }
            }
        }
    }
    println!("no response from list endpoint");
    std::process::exit(1);
}
