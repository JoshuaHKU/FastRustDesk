[中文](README.md) | **English**

# FastRustDesk

**Self-hosted remote desktop for Windows ↔ macOS**, an **unofficial fork** of
[RustDesk](https://github.com/rustdesk/rustdesk) v1.4.9.

Companion server: **FastRDServer** (same author, separate repository) — the two
together are the complete set.

---

> ### ⚠️ Unofficial fork
>
> This is an **unofficial personal fork** of RustDesk. It is **not affiliated
> with, endorsed by, or supported by** RustDesk or Purslane Ltd. "RustDesk" is a
> trademark of Purslane Ltd; it appears in this project's name only to describe
> what this work derives from.
>
> This is a **personal build**: **no commercial support**, **no warranty of any
> kind**, no guarantee of fitness for any purpose. Evaluate it yourself before
> relying on it.
>
> Please do **not** report problems with this fork to RustDesk upstream — it is
> not their code.

---

## What this fork is for

One Mac and a few Windows machines that need to control each other **without
going through anyone else's server**, with a **fluid picture and a controlled
machine that does not run hot**.

Upstream has three concrete obstacles in that scenario. This fork addresses each:

| Obstacle | What this fork does |
|---|---|
| High CPU on a macOS controlled machine (45%+ on a 5K display under load) | Capture and encode paths reworked — **down to 9%** |
| macOS capture uses `CGDisplayStream`, deprecated by Apple | Replaced with **ScreenCaptureKit** |
| With a self-hosted server, the "Accessible devices" tab can only show a login button (it is a commercial feature) | **Queries your own ID server directly**; click a device to connect |

On top of that: account groups, passwordless access within a group, and
automatic address-book population — the things that actually matter once you
run your own server.

## Main improvements

### 1. macOS controlled-side CPU cut to one fifth

Measured on the same 5120×2880 Mac, same workload, same session:
**45.6% → 9.0%**.

| Change | What it saves |
|---|---|
| **Capture NV12 directly** instead of capturing BGRA and converting on the CPU | 14.7M pixels of colour-space maths per frame; buffer 59 MB → 22 MB |
| **Zero-copy VideoToolbox encoding**: the IOSurface goes straight to the encoder | Pixels never touch the CPU |
| Hand the capture buffer to the encoder when the layouts match | 22 MB read + 22 MB write per frame |
| Drop the whole-frame comparison that duplicates what ScreenCaptureKit already reports | 59 MB memcmp + 59 MB copy per frame |
| Cap capture at 30 fps instead of 60 | About half the callbacks were wasted work |

The zero-copy path **needs no Metal** —
`VTCompressionSessionEncodeFrame` accepts a CVPixelBuffer / IOSurface as it is.
Verifying that before starting saved building an entire Metal pipeline.

> A still screen costs almost nothing: ScreenCaptureKit reports `Idle` for an
> unchanged desktop and those frames are dropped in the callback. **Make the
> screen move when comparing CPU figures**, or both versions read 0.

### 2. macOS capture moved to ScreenCaptureKit

Apple deprecated `CGDisplayStream` in macOS 14. The new backend lives in
`libs/scrap/src/sck/`; the minimum supported macOS therefore rises from 10.14 to
**12.3**.

Porting exposed two robustness problems, both fixed: ScreenCaptureKit briefly
returns an empty shareable list around display sleep/wake, and taking that first
answer at face value turns a momentary flap into a **permanent black screen**;
and a stopped stream was invisible to the layer above, freezing the session on
its last frame.

### 3. "Accessible devices" that works on a self-hosted server

Upstream's version of this tab is a **commercial feature** — it fetches from
Server Pro's HTTP API and requires an account login. The open-source server has
no such endpoint, so on a self-hosted setup that tab can only ever be a login
button.

This fork asks the ID server directly over the rendezvous protocol: no account
needed, devices listed, click to connect. Display name resolves as
**your alias → hostname → ID**.

> The enumeration request **requires the server key**. The adjacent online-state
> query upstream does not check it — that is fine, because it can only confirm
> the state of an ID the caller already knows. Enumeration is different: without
> the check, anyone who can reach the signalling port could harvest every device
> ID and IP on the server.

### 4. Accounts, groups and passwordless access (needs FastRDServer)

- A user table in the server's SQLite, with create / change-password / login
- Devices under one account form a group; the device roster is filtered by it
- **Devices in the same account connect to each other without a password**
- Group devices are added to the address book automatically at startup

> The same-account decision is made **on the server** and delivered to the
> controlled side over the signalling channel. That exemption sits **after** the
> "ask me first" gate — a machine set to require manual approval still asks.
> **Grouping relaxes the password, not consent.**
>
> The cost, stated plainly: the account token is a **bearer credential**. Whoever
> holds it reaches every device in the group without a password, and it crosses
> the network in the clear (only the login payload is sealed). Acceptable for a
> private deployment; not for an exposed one.

### 5. P2P status in the quality monitor

With "Show quality monitor" enabled, the overlay gains a `P2P: Yes/No` row:
**green Yes = direct**, **orange No = relayed through the server**,
grey `-` = the connection is not established yet.

A relayed session is not an error, but it costs a hop of latency and server
bandwidth. Previously you had to infer it from the tooltip on a toolbar icon.

### 6. Windows installer can carry the server configuration

The installer encodes the server address and public key **in its own file name**:
double-click to install, and it points at your server with nothing to type.

> A colon is illegal in a Windows file name, so this fork accepts **`+` in place
> of `:`** to carry a port: `host=rd.example.com+41116` →
> `rd.example.com:41116`. Upstream can only name a bare host, which silently
> dials the wrong port against a server on non-default ports.

### 7. Drag and drop between the two file-transfer panes

Drag a file or folder from one pane to the other to transfer it, and **drop it
onto a folder row to write into that folder**. Before this you had to select and
press your own pane's button, and the destination was always whatever directory
the opposite pane had open — there was no way to aim.

> Upstream's two buttons look like a pair but are actually **one per
> direction**: "Send" on your pane, "Receive" on the other one, the same code
> behind both, with the direction taken from which pane's controller ran it. To
> pull a file back from the remote machine you select it in the **remote** pane
> and press that pane's own button — which is not obvious.

This also fixes an upstream defect: dropping a file from Explorer/Finder onto
the remote pane used to ask the *remote* machine to send a local path, so the
job failed. It now uploads, which is what the gesture means.

### 8. The update check is turned off

**This fork never checks for updates and never asks anyone what the latest
version is.** That is deliberate, not an oversight.

Upstream's check compares the latest release against `crate::VERSION`, which
this fork keeps at its upstream base. Every later upstream release therefore
reads as an available update, permanently. And it is not one: what it offers is
stock RustDesk, which on macOS gets installed **as root**, replacing this build
and every change in it. The only thing standing in the way is
`allow-auto-update`, off by default.

It also POSTs to `api.rustdesk.com` on every start, which rather defeats the
point of running your own server.

> It is disabled in `do_check_software_update()` — the one funnel the startup
> banner, the auto-update scheduler and the macOS root service all pass
> through. The latter two call it directly, **bypassing** the "Check for
> software update on startup" setting, which is why unticking that box is not
> enough on its own and why this is done in code instead.
>
> To restore it, delete the two blocks marked `LOCAL FORK` in `src/common.rs`
> and `src/updater.rs`. The `enable-check-update` option itself is untouched.

### 9. Slow-link tuning (fast.5 / fast.6)

For relayed links with high RTT and real packet loss:

- **The client acknowledges every frame**, so the host stops queueing seconds of
  video in TCP and relay buffers. This works against stock RustDesk hosts too.
- **The host caps its bitrate to the measured link capacity**, and on macOS
  **encodes at a lower resolution** when the cap is low (GPU scaling, the
  zero-copy path is untouched).
- The VideoToolbox encoder **changes bitrate without forcing a keyframe**.
- An optional **MSS clamp on the relay connection**, for paths that lose large
  packets more than small ones and whose MTU you cannot change.

Everything has a default. Options and how to measure them are in
[FastRustDesk.md, section 7](FastRustDesk.md#七慢链路调优开关fast5--fast6) (Chinese).

### 10. macOS virtual displays (1.5.0)

A headless Mac mini / Mac Studio, or a closed MacBook, normally shows a remote
session nothing but macOS's low-resolution placeholder, or no picture at all.
The 1.5.0 host **creates a virtual display** instead, with no HDMI dummy plug
and no third-party tool:

- sized to the **controlling side's screen** (logical size, HiDPI by default)
  and kept as a **persistent** display that later sessions start on;
- the remote toolbar can add up to 4 displays and resize them; displays added
  that way are removed 30 s after the last session ends;
- on the Mac itself they are managed from the settings page, the menu bar tray
  or the `--vdisplay` command line, which suits SSH upkeep of headless machines;
- built on macOS 14+'s private `CGVirtualDisplay` API; if it is missing the
  feature hides itself.

Usage, options and implementation notes (several measured behaviours differ
from what other projects document) are in
[FastRustDesk.md, section 8](FastRustDesk.md#八macos-虚拟显示屏150) (Chinese).
The controlling side needs 1.5.0 too to report its screen size and show the menu.

## What FastRDServer adds

This client with the stock open-source server gives you "RustDesk that performs
better". Pairing it with **FastRDServer** gives you the whole thing:

| Capability | Client only | Client + FastRDServer |
|---|---|---|
| Self-hosted, no third party involved | ✅ | ✅ |
| Low macOS CPU | ✅ | ✅ |
| "Accessible devices" lists what is on your server | ✅ | ✅ |
| Account login (accounts created from the server CLI) | ❌ | ✅ |
| Devices grouped by account; you only see your own | ❌ | ✅ |
| **Passwordless access within a group** | ❌ | ✅ |
| Address book synced across machines | ❌ | ✅ |

## Platform support

| Platform | Status |
|---|---|
| **macOS** (Apple Silicon, 12.3+) | Controller and controlled; all performance work applies |
| **Windows** 10/11 x64 | Controller and controlled; feature-complete |
| **iOS** 13.0+ | **Controller only**; builds and runs, see below |
| Linux / Android | Upstream code retained, **untested in this fork** |

### iOS

It builds and runs on a device, but it can only ever be the **controlling**
side: upstream excludes `mod server` and `mod rendezvous_mediator` from iOS, so
an iPhone cannot be controlled. ffmpeg's iOS configuration agrees — it enables
decoders only (`--disable-encoders --disable-videotoolbox`).

Three things to know before building it yourself:

- **Set your own bundle identifier and team first.**
  `PRODUCT_BUNDLE_IDENTIFIER` here is a placeholder
  (`com.example.fastrustdesk`) and `DEVELOPMENT_TEAM` is empty so Xcode fills
  in whichever account is signed in.
- **Export `IPHONEOS_DEPLOYMENT_TARGET=13.0` before `cargo build`.** rustc's
  `aarch64-apple-ios` links against iOS 10.0 by default while vcpkg builds aom
  against the current SDK; the 10.0 libSystem has no `___chkstk_darwin` (it
  arrived in iOS 12), which produces a wall of undefined symbols.
- **Installing the `arm64-ios` dependencies deletes `ffmpeg:arm64-osx`.**
  ffmpeg's `host: true` platform expression in vcpkg.json is
  `(... | osx) & static`, which is false for an iOS target, so manifest mode
  decides it is no longer needed. The macOS build wants it — reinstall it
  afterwards.

Signed with a free Apple ID, the certificate **expires after 7 days** and the
app has to be re-signed and reinstalled.

The performance work is in the macOS capture and encode path and **does not
carry over** to other platforms. Accounts, grouping, the device roster and the
P2P indicator are cross-platform.

## Building

macOS:

```bash
export VCPKG_ROOT=~/vcpkg
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
export PATH="$HOME/flutter/bin:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
python3 build.py --flutter --hwcodec
```

Windows (Flutter desktop **cannot be cross-compiled from macOS**; build on
Windows):

```powershell
$env:VCPKG_ROOT = "D:\vcpkg"
$env:VCPKG_DEFAULT_HOST_TRIPLET = "x64-windows-static"
python build.py --portable --flutter --hwcodec
```

Full prerequisites and the versions that must be pinned are in
**[FastRustDesk.md](FastRustDesk.md)** (Chinese).

## Documentation

The detailed documents are in Chinese.

| File | Contents |
|---|---|
| [FastRustDesk.md](FastRustDesk.md) | Build and deploy: how the changes work, full build steps, deployment notes |
| [FastRustDesk-SELFHOST.md](FastRustDesk-SELFHOST.md) | Running your own server |
| [INSTALL-FastRustDesk-macos.md](INSTALL-FastRustDesk-macos.md) | macOS client install and configuration |
| [INSTALL-FastRustDesk-windows.md](INSTALL-FastRustDesk-windows.md) | Windows client install and configuration |
| [INSTALL-FastRDServer-ubuntu.md](INSTALL-FastRDServer-ubuntu.md) | Server install and configuration |
| [README-upstream.md](README-upstream.md) | RustDesk's original README |

Every address, key and device ID in the documents is a placeholder
(`rd.example.com`, `<YOUR_SERVER_PUBLIC_KEY>`, …). Substitute your own.

## Relationship to upstream

Baseline: `rustdesk/rustdesk` **v1.4.9** (commit `11190fa`).

**These changes are not suitable for upstream** and are not offered to it: the
macOS deployment target was raised, the CGDisplayStream backend was deleted, the
meaning of a commercial-edition tab was changed, and fields were added to the
signalling protocol. Upstream has to support every platform and leave room for
the commercial edition; these trade-offs are not available to them.

For the official version, go to
[rustdesk/rustdesk](https://github.com/rustdesk/rustdesk).

## License

**AGPL-3.0**, unchanged from upstream — see [LICENCE](LICENCE).

As a derivative of an AGPL work, this project **must** be, and can only be,
released under AGPL-3.0. The statements above about personal use, no commercial
support and no warranty are the author's **statements**, **not** additional
licence terms — AGPL-3.0 does not permit imposing further restrictions.

⚠️ **The AGPL network clause**: if you modify this and **offer it as a service to
others**, you owe those users the corresponding source. Running it for yourself
carries no such obligation.
