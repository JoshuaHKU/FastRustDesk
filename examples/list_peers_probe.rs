//! Probe for the fork's device-list feature against the self-hosted ID server.
//!
//! Goes through `main_get_common("server-peers")` -- the exact entry point the
//! Flutter panel uses -- so this exercises the real path end to end: bridge
//! function, `ListPeersRequest` over the rendezvous connection, server-side key
//! check, JSON shaping. `client` is a private module, and reaching past the
//! bridge would test less than the UI actually does.
//!
//! Both outcomes are checked:
//!
//!   * with the configured key       -> the server returns the roster
//!   * with a deliberately wrong key -> the server refuses
//!
//! The negative case is the point. Enumeration is only safe to expose because
//! the server rejects callers that cannot present the key; a regression there
//! would silently turn this into an open directory of every device and its IP.
//!
//! Run: cargo run --release --features hwcodec,flutter --example list_peers_probe
use hbb_common::config::Config;

fn main() {
    let server = Config::get_option("custom-rendezvous-server");
    let key = Config::get_option("key");
    println!(
        "id server : {}",
        if server.is_empty() {
            "(unset)"
        } else {
            &server
        }
    );
    println!(
        "key       : {}",
        if key.is_empty() { "(unset)" } else { "(set)" }
    );
    println!("my id     : {}\n", Config::get_id());

    println!("--- with the configured key ---");
    let raw = librustdesk::flutter_ffi::main_get_common("server-peers".to_owned());
    let v: serde_json::Value = serde_json::from_str(&raw).expect("response is not JSON");
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        println!("FAILED: {err}");
        std::process::exit(1);
    }
    let peers = v["peers"].as_array().cloned().unwrap_or_default();
    println!("{} peer(s):", peers.len());
    for p in &peers {
        println!(
            "  {:<12} {:<8} {:<10} {:<24} {}",
            p["id"].as_str().unwrap_or(""),
            if p["online"].as_bool().unwrap_or(false) {
                "online"
            } else {
                "offline"
            },
            p["platform"].as_str().unwrap_or(""),
            p["hostname"].as_str().unwrap_or(""),
            p["ip"].as_str().unwrap_or("")
        );
    }
    assert!(!peers.is_empty(), "expected at least one other device");

    println!("\n--- with a wrong key (must be refused) ---");
    Config::set_option("key".to_owned(), "definitely-not-the-key".to_owned());
    let raw2 = librustdesk::flutter_ffi::main_get_common("server-peers".to_owned());
    Config::set_option("key".to_owned(), key);
    let v2: serde_json::Value = serde_json::from_str(&raw2).expect("response is not JSON");
    match v2.get("error").and_then(|e| e.as_str()) {
        Some(err) => println!("refused as expected: {err}"),
        None => {
            let n = v2["peers"].as_array().map(|a| a.len()).unwrap_or(0);
            println!("NOT REFUSED -- got {n} peer(s). This is a security hole.");
            std::process::exit(1);
        }
    }

    println!("\nOK: device list works and the key is enforced");
}
