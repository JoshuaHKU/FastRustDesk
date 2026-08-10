//! Configure this machine as a RustDesk host without needing root or the service:
//! set the permanent password and enable direct IP access.
//!
//! The password is salted and hashed on the way in, so it cannot simply be written
//! into RustDesk.toml by hand — this goes through the same Config API the GUI uses.
extern crate librustdesk as _;

use hbb_common::config::{self, Config};

fn main() {
    let pw = std::env::args().nth(1).expect("usage: setup_host <password>");

    if Config::is_disable_change_permanent_password() {
        eprintln!("permanent password changes are disabled by policy");
        std::process::exit(1);
    }
    let ok = Config::set_permanent_password(&pw);
    println!("set_permanent_password: {ok}");

    // Direct IP access. option2bool treats this key as opt-in ("Y" enables).
    Config::set_option(
        config::keys::OPTION_DIRECT_SERVER.to_owned(),
        "Y".to_owned(),
    );
    let got = Config::get_option(config::keys::OPTION_DIRECT_SERVER);
    println!("{} = {:?}", config::keys::OPTION_DIRECT_SERVER, got);

    println!("id = {}", Config::get_id());
    let (stored, salt) = Config::get_local_permanent_password_storage_and_salt();
    println!("password stored: {} chars, salt: {} chars", stored.len(), salt.len());
}
