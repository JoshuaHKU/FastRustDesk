//! Prove the account login is encrypted end to end without TLS.
//!
//! Runs the exact flow a client will: seal `{username, password, id, ts,
//! reply_pk}` to the ID server's public key, POST it, then open the reply with
//! the one-shot secret key. Nothing readable crosses the wire.
//!
//! Checks the failure modes too, because those are what make it worth doing:
//! a wrong server key must not decrypt, a stale timestamp must be refused, and
//! the plaintext password must not appear anywhere in the bytes actually sent.
//!
//! Run: cargo run --release --features hwcodec,flutter --example sealed_login_probe \
//!        -- <api-url> <server-pk-base64> <user> <password>
use hbb_common::sealed;
use serde_json::json;

fn main() {
    hbb_common::sodiumoxide::init().ok();
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        println!("usage: sealed_login_probe <api-url> <server-pk-b64> <user> <password>");
        std::process::exit(2);
    }
    let (url, pk_b64, user, password) = (&a[1], &a[2], &a[3], &a[4]);

    let server_pk = match sealed::server_box_pk(pk_b64) {
        Ok(pk) => pk,
        Err(e) => {
            println!("bad server public key: {e}");
            std::process::exit(1);
        }
    };
    let (reply_pk, reply_sk) = sealed::gen_reply_keypair();

    let plain = json!({
        "username": user,
        "password": password,
        "id": "probe-device",
        "ts": sealed::now_secs(),
        "reply_pk": sealed::encode_pk(&reply_pk),
    })
    .to_string();

    let sealed_b64 = sealed::seal_to(plain.as_bytes(), &server_pk);
    let wire = json!({ "sealed": sealed_b64 }).to_string();

    println!("--- what actually goes on the wire ---");
    println!("  {} bytes", wire.len());
    println!("  {}...", &wire[..wire.len().min(96)]);
    // The whole point: an observer must not be able to read the password.
    if wire.contains(password.as_str()) || wire.contains(user.as_str()) {
        println!("\nFAIL: credentials appear in the request body in the clear");
        std::process::exit(1);
    }
    println!("  password/username not present in the payload: OK");

    println!("\n--- POST {url}/api/login ---");
    let resp = match ureq_post(&format!("{url}/api/login"), &wire) {
        Ok(r) => r,
        Err(e) => {
            println!("request failed: {e}");
            std::process::exit(1);
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&resp) {
        Ok(v) => v,
        Err(e) => {
            println!("response is not JSON: {e}\n{resp}");
            std::process::exit(1);
        }
    };
    if let Some(err) = v["error"].as_str() {
        println!("server said: {err}");
        std::process::exit(1);
    }
    let Some(sealed_reply) = v["sealed"].as_str() else {
        println!("FAIL: reply was not sealed -- token would be readable on the wire");
        std::process::exit(1);
    };
    println!("  reply is sealed ({} bytes)", sealed_reply.len());
    if sealed_reply.contains("access_token") {
        println!("FAIL: reply looks like plaintext");
        std::process::exit(1);
    }

    let opened = match sealed::open_with(sealed_reply, &reply_pk, &reply_sk) {
        Ok(b) => b,
        Err(e) => {
            println!("FAIL: cannot open reply: {e}");
            std::process::exit(1);
        }
    };
    let out: serde_json::Value = serde_json::from_slice(&opened).unwrap_or_default();
    let token = out["access_token"].as_str().unwrap_or("");
    println!(
        "  opened: user={} admin={} token={} chars",
        out["user"]["name"].as_str().unwrap_or("?"),
        out["user"]["is_admin"],
        token.len()
    );
    if token.len() != 64 {
        println!("FAIL: unexpected token");
        std::process::exit(1);
    }

    println!("\n--- negative: stale timestamp must be refused ---");
    let stale = json!({
        "username": user, "password": password, "id": "probe",
        "ts": sealed::now_secs() - 3600,
        "reply_pk": sealed::encode_pk(&reply_pk),
    })
    .to_string();
    let wire2 = json!({ "sealed": sealed::seal_to(stale.as_bytes(), &server_pk) }).to_string();
    let r2 = ureq_post(&format!("{url}/api/login"), &wire2).unwrap_or_default();
    if r2.contains("Stale request") {
        println!("  refused as expected");
    } else {
        println!("FAIL: replay window not enforced: {r2}");
        std::process::exit(1);
    }

    println!("\n--- negative: sealed to the wrong server key must not decrypt ---");
    let (other_pk, _other_sk) = sealed::gen_reply_keypair();
    let wire3 = json!({ "sealed": sealed::seal_to(plain.as_bytes(), &other_pk) }).to_string();
    let r3 = ureq_post(&format!("{url}/api/login"), &wire3).unwrap_or_default();
    if r3.contains("Decryption failed") {
        println!("  refused as expected");
    } else {
        println!("FAIL: server accepted a payload it should not decrypt: {r3}");
        std::process::exit(1);
    }

    println!("\nOK: credentials and token are encrypted end to end, replay is bounded");
}

/// Minimal POST so the probe does not drag in an HTTP client crate.
fn ureq_post(url: &str, body: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").ok_or("only http:// supported")?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut stream =
        std::net::TcpStream::connect(host_port).map_err(|e| format!("connect: {e}"))?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    match buf.split_once("\r\n\r\n") {
        Some((_, body)) => Ok(body.to_owned()),
        None => Err("malformed response".into()),
    }
}
