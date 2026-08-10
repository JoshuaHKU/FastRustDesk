//! LOCAL FORK: anonymous public-key encryption for the account API.
//!
//! The account endpoints carry passwords and bearer tokens, and this deployment
//! has no TLS. Rather than send those in the clear, the payload is encrypted to
//! the **ID server's public key** -- the same `key=` value every client is
//! already configured with. No certificates, nothing to renew, and the client
//! needs no new secret.
//!
//! libsodium's *sealed box* is built for exactly this shape: encrypt to a known
//! public key with no prior handshake. It generates a throw-away keypair per
//! message, so two encryptions of the same text differ and the sender is
//! anonymous. Only the holder of the matching secret key can open it -- which
//! also means a successful decrypt proves we are talking to the real server.
//!
//! The server key is ed25519 (a *signing* key). libsodium converts it to the
//! curve25519 form the box primitives need; that conversion is standard and
//! lossless.
//!
//! # What this does and does not give you
//!
//! * Passwords and tokens are unreadable on the wire.
//! * Only the real server can decrypt a request.
//! * Replay is bounded by a timestamp the caller embeds and the server checks --
//!   sealed boxes alone do not prevent it.
//! * It is **not** a general TLS substitute: it protects these payloads, not the
//!   rest of the traffic, and it all rests on the server's secret key staying
//!   secret.

use crate::{bail, base64, ResultType};
use sodiumoxide::crypto::{box_, sealedbox, sign};

/// Ephemeral keypair for the reply leg.
///
/// A sealed box hides the sender, so the server cannot reply into the same
/// channel. The client makes a one-shot keypair, ships the public half inside
/// the sealed request, and the server seals the answer back to it. The private
/// half never leaves the client, so a captured response is useless to anyone
/// else -- and useless to the same attacker next time, because the key is new.
pub fn gen_reply_keypair() -> (box_::PublicKey, box_::SecretKey) {
    box_::gen_keypair()
}

pub fn encode_pk(pk: &box_::PublicKey) -> String {
    base64::encode(pk.0)
}

pub fn decode_pk(s: &str) -> ResultType<box_::PublicKey> {
    let raw = base64::decode(s)?;
    match box_::PublicKey::from_slice(&raw) {
        Some(pk) => Ok(pk),
        None => bail!("bad reply public key"),
    }
}

/// Convert the server's base64 ed25519 **public** key (the client's `key=`
/// option) into the curve25519 form sealed boxes use.
pub fn server_box_pk(server_pk_b64: &str) -> ResultType<box_::PublicKey> {
    let raw = base64::decode(server_pk_b64)?;
    let Some(ed) = sign::PublicKey::from_slice(&raw) else {
        bail!("bad server public key");
    };
    match sign::to_curve25519_pk(&ed) {
        Ok(pk) => Ok(pk),
        Err(_) => bail!("cannot convert server public key"),
    }
}

/// Same conversion for the server's ed25519 **secret** key.
pub fn server_box_sk(sk: &sign::SecretKey) -> ResultType<(box_::PublicKey, box_::SecretKey)> {
    let Ok(bsk) = sign::to_curve25519_sk(sk) else {
        bail!("cannot convert server secret key");
    };
    // The public half is derived rather than passed in, so the caller cannot
    // accidentally pair a key with the wrong counterpart.
    let ed_pk = sign::PublicKey::from_slice(&sk.0[sign::SECRETKEYBYTES / 2..]);
    let Some(ed_pk) = ed_pk else {
        bail!("cannot derive server public key");
    };
    let Ok(bpk) = sign::to_curve25519_pk(&ed_pk) else {
        bail!("cannot convert derived public key");
    };
    Ok((bpk, bsk))
}

/// Encrypt to a curve25519 public key. Output is base64 for JSON transport.
pub fn seal_to(plaintext: &[u8], pk: &box_::PublicKey) -> String {
    base64::encode(sealedbox::seal(plaintext, pk))
}

/// Decrypt something sealed to `pk`/`sk`.
pub fn open_with(sealed_b64: &str, pk: &box_::PublicKey, sk: &box_::SecretKey) -> ResultType<Vec<u8>> {
    let raw = base64::decode(sealed_b64)?;
    match sealedbox::open(&raw, pk, sk) {
        Ok(v) => Ok(v),
        // Wrong key, corruption and tampering all land here; none of them should
        // be distinguishable to a caller.
        Err(_) => bail!("decryption failed"),
    }
}

/// Seconds since the epoch, for the anti-replay stamp callers embed.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How far out of date a request may be before the server rejects it. Wide
/// enough for ordinary clock skew, narrow enough that a captured blob stops
/// working quickly.
pub const MAX_SKEW_SECS: i64 = 120;

pub fn timestamp_acceptable(ts: i64) -> bool {
    (now_secs() - ts).abs() <= MAX_SKEW_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_keypair() -> (String, sign::SecretKey) {
        let (pk, sk) = sign::gen_keypair();
        (base64::encode(pk.0), sk)
    }

    #[test]
    fn round_trip_to_server() {
        sodiumoxide::init().ok();
        let (pk_b64, sk) = server_keypair();
        let sealed = seal_to(b"hunter2", &server_box_pk(&pk_b64).unwrap());
        let (bpk, bsk) = server_box_sk(&sk).unwrap();
        assert_eq!(open_with(&sealed, &bpk, &bsk).unwrap(), b"hunter2");
    }

    /// Same plaintext must not produce the same ciphertext, or a passive
    /// observer could recognise a repeated password without decrypting it.
    #[test]
    fn ciphertext_differs_each_time() {
        sodiumoxide::init().ok();
        let (pk_b64, _sk) = server_keypair();
        let pk = server_box_pk(&pk_b64).unwrap();
        assert_ne!(seal_to(b"same", &pk), seal_to(b"same", &pk));
    }

    /// A different server key must not open it -- this is what makes a successful
    /// decrypt evidence that we reached the real server.
    #[test]
    fn wrong_key_cannot_open() {
        sodiumoxide::init().ok();
        let (pk_b64, _sk) = server_keypair();
        let (_other_pk, other_sk) = server_keypair();
        let sealed = seal_to(b"secret", &server_box_pk(&pk_b64).unwrap());
        let (bpk, bsk) = server_box_sk(&other_sk).unwrap();
        assert!(open_with(&sealed, &bpk, &bsk).is_err());
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        sodiumoxide::init().ok();
        let (pk_b64, sk) = server_keypair();
        let sealed = seal_to(b"secret", &server_box_pk(&pk_b64).unwrap());
        let mut raw = base64::decode(&sealed).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        let (bpk, bsk) = server_box_sk(&sk).unwrap();
        assert!(open_with(&base64::encode(raw), &bpk, &bsk).is_err());
    }

    #[test]
    fn reply_leg_round_trips() {
        sodiumoxide::init().ok();
        let (rpk, rsk) = gen_reply_keypair();
        let wire = encode_pk(&rpk);
        let sealed = seal_to(b"token", &decode_pk(&wire).unwrap());
        assert_eq!(open_with(&sealed, &rpk, &rsk).unwrap(), b"token");
    }

    #[test]
    fn stale_timestamp_rejected() {
        assert!(timestamp_acceptable(now_secs()));
        assert!(!timestamp_acceptable(now_secs() - MAX_SKEW_SECS - 60));
        assert!(!timestamp_acceptable(now_secs() + MAX_SKEW_SECS + 60));
    }
}
