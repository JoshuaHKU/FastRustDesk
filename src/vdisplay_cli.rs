//! FastRustDesk: `FastRustDesk --vdisplay ...` (design 6.4).
//!
//! Everything except `probe` is sent over IPC to the login user's `--server`,
//! which owns the displays. Run it as the login user (an SSH login as that user
//! works); not with sudo, which would rewrite the user's config as root.

const USAGE: &str = "usage:
  FastRustDesk --vdisplay status [--json]
  FastRustDesk --vdisplay on|off <slot|all>
  FastRustDesk --vdisplay add [--res WxH] [--hidpi on|off] [--hz 30|60] [--persistent] [--follow] [--name N]
  FastRustDesk --vdisplay set <slot> [--res WxH] [--hidpi on|off] [--hz 30|60] [--persistent on|off] [--follow on|off] [--name N]
  FastRustDesk --vdisplay rm <slot>
  FastRustDesk --vdisplay opt <headless|allow-remote|hidpi|keep> <value>
  FastRustDesk --vdisplay probe";

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn onoff(v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "on" | "y" | "yes" | "true" | "1" => Ok(true),
        "off" | "n" | "no" | "false" | "0" => Ok(false),
        _ => Err(format!("expected on/off, got {}", v)),
    }
}

fn parse_res(v: &str) -> Result<(u32, u32), String> {
    let (w, h) = v
        .split_once(['x', 'X', '*'])
        .ok_or(format!("bad resolution {}", v))?;
    Ok((
        w.trim().parse().map_err(|_| format!("bad width {}", w))?,
        h.trim().parse().map_err(|_| format!("bad height {}", h))?,
    ))
}

/// Build the JSON command for everything but status/probe.
fn build(args: &[String]) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    let pos = |i: usize| args.get(i).map(|s| s.as_str());
    let mut v = match sub {
        "on" | "off" | "rm" => {
            let slot = pos(1).ok_or("slot required")?;
            if slot == "all" {
                json!({"cmd": sub, "slot": "all"})
            } else {
                json!({"cmd": sub, "slot": slot.parse::<u8>().map_err(|_| "bad slot")?})
            }
        }
        "add" => json!({"cmd": "add"}),
        "set" => {
            let slot = pos(1).ok_or("slot required")?;
            json!({"cmd": "set", "slot": slot.parse::<u8>().map_err(|_| "bad slot")?})
        }
        "opt" => {
            let key = pos(1).ok_or("option name required")?;
            let value = pos(2).ok_or("option value required")?;
            return Ok(json!({"cmd": "opt", "key": key, "value": value}));
        }
        _ => return Err(USAGE.to_owned()),
    };
    if sub == "add" || sub == "set" {
        if let Some(r) = flag(args, "--res") {
            let (w, h) = parse_res(r)?;
            v["w"] = json!(w);
            v["h"] = json!(h);
        }
        if let Some(x) = flag(args, "--hidpi") {
            v["hidpi"] = json!(onoff(x)?);
        }
        if let Some(x) = flag(args, "--hz") {
            v["refresh"] = json!(x.parse::<u32>().map_err(|_| "bad --hz")?);
        }
        if let Some(x) = flag(args, "--name") {
            v["name"] = json!(x);
        }
        for (f, k) in [("--persistent", "persistent"), ("--follow", "follow")] {
            if has(args, f) {
                // `add --persistent` is a switch; `set --persistent on|off` takes a value.
                let val = match flag(args, f) {
                    Some(x) if !x.starts_with("--") => onoff(x)?,
                    _ => true,
                };
                v[k] = json!(val);
            }
        }
    }
    Ok(v)
}

fn print_status(json: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        println!("{}", json);
        return;
    };
    let supported = v["supported"].as_bool().unwrap_or(false);
    println!(
        "supported: {}{}",
        if supported { "yes" } else { "no" },
        v["reason"]
            .as_str()
            .filter(|r| !r.is_empty())
            .map(|r| format!(" ({})", r))
            .unwrap_or_default()
    );
    println!(
        "headless: {}   sessions: {}   reclaim in: {}",
        v["headless"],
        v["sessions"],
        v["reclaim_in"]
            .as_u64()
            .map(|s| format!("{} s", s))
            .unwrap_or("-".into())
    );
    let o = &v["options"];
    println!(
        "options: headless={} allow-remote={} hidpi={} keep={}",
        o["headless"], o["allow_remote"], o["hidpi"], o["keep"]
    );
    let slots = v["slots"].as_array().cloned().unwrap_or_default();
    if slots.is_empty() {
        println!("no virtual displays configured");
    }
    for s in slots {
        println!(
            "  slot {}  {:<36} {:<18} {}{}{}  {}",
            s["slot"],
            s["name"].as_str().unwrap_or(""),
            s["label"].as_str().unwrap_or(""),
            if s["persistent"].as_bool() == Some(true) {
                "persistent "
            } else {
                ""
            },
            if s["follow"].as_bool() == Some(true) {
                "follow "
            } else {
                ""
            },
            if s["active"].as_bool() == Some(true) {
                format!("ACTIVE id={}", s["display_id"])
            } else {
                "off".to_owned()
            },
            s["sources"]
        );
    }
}

/// Entry point; returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "probe" => {
            let (fails, report) = crate::vdisplay::probe_report();
            print!("{}", report);
            println!("failures: {}", fails);
            fails
        }
        "status" | "" => match crate::ipc::vdisplay_request("vdisplay-status", 3_000) {
            Ok(Some(j)) => {
                if has(args, "--json") {
                    println!("{}", j);
                } else {
                    print_status(&j);
                }
                0
            }
            Ok(None) => {
                eprintln!("the running server does not support --vdisplay");
                2
            }
            Err(e) => {
                eprintln!("server not running ({})", e);
                2
            }
        },
        "help" | "-h" | "--help" => {
            println!("{}", USAGE);
            0
        }
        _ => {
            let cmd = match build(args) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{}", e);
                    return 64;
                }
            };
            let name = format!("vdisplay-cmd:{}", cmd);
            match crate::ipc::vdisplay_request(&name, 20_000) {
                Ok(Some(r)) if r.starts_with("OK:") => {
                    println!("{}", &r[3..]);
                    0
                }
                Ok(Some(r)) => {
                    eprintln!("{}", r.strip_prefix("ERR:").unwrap_or(&r));
                    1
                }
                Ok(None) => {
                    eprintln!("the running server does not support --vdisplay");
                    2
                }
                Err(e) => {
                    eprintln!("server not running ({})", e);
                    2
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn builds_commands() {
        let c = build(&a(&["on", "2"])).unwrap();
        assert_eq!(c["cmd"], "on");
        assert_eq!(c["slot"], 2);
        let c = build(&a(&["off", "all"])).unwrap();
        assert_eq!(c["slot"], "all");
        let c = build(&a(&[
            "add",
            "--res",
            "2560x1440",
            "--hidpi",
            "off",
            "--persistent",
        ]))
        .unwrap();
        assert_eq!((c["w"].as_u64(), c["h"].as_u64()), (Some(2560), Some(1440)));
        assert_eq!(c["hidpi"], false);
        assert_eq!(c["persistent"], true);
        let c = build(&a(&["set", "1", "--persistent", "off", "--name", "TV"])).unwrap();
        assert_eq!(c["persistent"], false);
        assert_eq!(c["name"], "TV");
        let c = build(&a(&["opt", "keep", "-1"])).unwrap();
        assert_eq!(c["value"], "-1");
        assert!(build(&a(&["on"])).is_err());
        assert!(build(&a(&["add", "--res", "abc"])).is_err());
        assert!(build(&a(&["frob"])).is_err());
    }
}
