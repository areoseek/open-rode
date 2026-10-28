# Open Rode

**Open Rode is an open source Rust rewrite of the RODE Central client for Linux.**
It configures the RODE Wireless GO II (receiver on USB, transmitters through it)
natively: no Wine, no kernel module, no RODE account.

RODE ships RODE Central for Windows and macOS only. Under the hood that app is a
Flutter shell over a C++ library that talks to the microphones with
vendor-specific USB HID reports. Linux already exposes those as `/dev/hidrawN`,
so the whole feature set could be rebuilt as a small native program. The
protocol was recovered by tracing RODE Central's own traffic (run once under
Wine with HID logging while every control was clicked) and then replaying each
command against real hardware. The capture, the parser and the full register
map are in this repository so anyone can check the work or extend it.

Tested on CachyOS (Arch) with a Wireless GO II RX on firmware 2.5.0
(USB id `19f7:002a`) and one paired TX.

Open Rode is a community project and is not affiliated with or endorsed by
RODE Microphones.

## Features

| Receiver | Transmitter (through the RX radio link) |
|---|---|
| Battery, serial, name, clock with one-click sync | Battery, firmware version, serial |
| Channel routing: Split or Merged | On-board recording: Off, Always, Manual, Backup |
| Safety channel | Compressed recordings on or off |
| Output gain: Coarse (3 levels), Fine (-30 to 0 dB in 3 dB steps), Camera | Button function: Disabled, Mute, Marker, Record |
| Button function: toggle auto-dim or drop a marker | LED brightness: Bright or Dim |
| Auto-dim | Input PAD |

Everything above is read back from the device, so the window always shows the
real state, including changes made on the hardware itself.

Not implemented: firmware updates (a separate bulk-transfer path that is left
alone on purpose), factory reset, renaming, diagnostics, and a recordings
browser. Recordings are plain WAV files on the transmitter, which shows up as a
read-only USB drive when plugged in with a cable, so any file manager works.

## Installing

### Arch Linux and derivatives (CachyOS, EndeavourOS, Manjaro)

A prebuilt package is attached to each release on the
[releases page](https://github.com/areoseek/open-rode/releases):

```sh
sudo pacman -U open-rode-0.1.0-1-x86_64.pkg.tar.zst
```

Then unplug and replug the receiver so the bundled udev rule takes effect. Start
"Open Rode" from the application menu, or run `open-rode` / `open-rode-cli`.

To build the package yourself: `cd packaging/arch && makepkg -si`.

### Other distributions (build from source)

You need a Rust toolchain (1.80 or newer, via [rustup](https://rustup.rs)), a C
compiler, pkg-config, and the development headers for libudev, Wayland or X11,
libxkbcommon and OpenGL. On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config libudev-dev libwayland-dev \
    libxkbcommon-dev libgl1-mesa-dev libxcb1-dev libxcb-render0-dev \
    libxcb-shape0-dev libxcb-xfixes0-dev libfontconfig1-dev
```

On Fedora:

```sh
sudo dnf install gcc pkgconf-pkg-config systemd-devel wayland-devel \
    libxkbcommon-devel mesa-libGL-devel fontconfig-devel
```

Then:

```sh
git clone https://github.com/areoseek/open-rode.git
cd open-rode
cargo build --release

sudo install -Dm755 target/release/open-rode     /usr/local/bin/open-rode
sudo install -Dm755 target/release/open-rode-cli /usr/local/bin/open-rode-cli
sudo install -Dm644 open-rode.desktop /usr/local/share/applications/open-rode.desktop
sudo install -Dm644 udev/70-open-rode.rules /etc/udev/rules.d/70-open-rode.rules
sudo udevadm control --reload-rules
```

Unplug and replug the receiver after installing the udev rule. Without the rule
the HID node is root-only and the app will say no device was found.

## Command line

```
open-rode-cli status
open-rode-cli gain fine 8                  # -6 dB
open-rode-cli gain coarse 2                # high
open-rode-cli channel merged --safety
open-rode-cli autodim off
open-rode-cli sync-clock
open-rode-cli tx-record backup --button marker
open-rode-cli tx-led dim
open-rode-cli tx-pad on
open-rode-cli get 0b                       # raw register read
open-rode-cli send "09 00 01" --len 37     # raw frame
open-rode-cli desc                         # HID report descriptor
```

## Protocol reference (Wireless GO II RX, firmware 2.5.0)

Vendor HID, usage page `FF00`, one interface with a single interrupt IN
endpoint. Host writes go out as `SET_REPORT` on the control pipe (hidraw does
that for you). Every OUT report `n` is answered on IN report `n+1` with the
opcode echoed, then `41` for ACK or `4E` for NAK.

Report sizes from the descriptor: `01/02` 17 bytes, `03/04` 17/13 bytes, `05`
1073 bytes and `07` 1537 bytes (firmware upload), `09/0A` 37 bytes, `0B` 61
bytes (unsolicited), `0D/0E` and `0F/10` 64 bytes.

### Report 0x09: register file (37 bytes)

```
GET   09 <op> 01                 ->  0a <op> 41 <data...>
SET   09 <op> 00 <params...>     ->  0a <op> 41 <echo>
TX    09 0d <reg> <val...>       ->  0a 0d 41 <reg>        (forwarded to the paired TX)
```

| op | GET returns | SET params | meaning |
|---|---|---|---|
| `00` | `[pct]` | none | RX battery percent |
| `01` | `[u32 LE]` | `[u32 LE]` | RX clock as local wall time in unix format. RODE Central writes it on connect |
| `02` | `[u32 LE]` | none | second timestamp, about 45 s behind the clock, purpose unknown |
| `04` | `[0 or 1]` | `[0 or 1]` | auto-dim |
| `05` | `[mode, val]` | `[mode, val]` | output gain. mode 0 Coarse (val 0 to 2), 1 Fine (val 0 to 10 = -30 to 0 dB), 2 Camera |
| `06` | `[0 or 1]` | `[0 or 1]` | 1 = Merged, 0 = Split |
| `07` | `[0 or 1]` | `[0 or 1]` | safety channel |
| `08` | `[slot, len, ascii...]` | none | name table. Slot 0 is the device name (`WIGO2`), slots 1 to 9 return `ff` |
| `0a` | `[0 or 1]` | `[0 or 1]` | RX button: 0 toggles auto-dim, 1 drops a marker |
| `0b` | 28 bytes | none | paired TX status blob, see below |
| `0c` | NAK | | not present on the RX |
| `0d` | none | `[reg, val]` | write a TX register |
| `0f` | empty | none | polled every cycle by RODE Central, purpose unknown |
| `14` | `[0 or 1]` | `[0 or 1]` | written together with `06`, inverse value (1 when Split) |
| `16` | `00 16` | | unknown |
| `18`, `19` | `02` | | unknown |
| `1a` | none | none | refresh. Sent periodically and after changes |
| `22` | `42` | | unknown |

RODE Central's idle loop GETs `00 01 04 05 06 0b 0c 0f` round robin at about
4 Hz, plus `1a` every two minutes.

### TX registers (written through `09 0d`)

| reg | value | meaning |
|---|---|---|
| `06` | u32 LE | TX clock |
| `08` | 0 or 1 | input PAD |
| `18` | 0 or 1 | compressed recordings |
| `1a` | `DF` or `FE` | LEDs bright or dim |
| `1b 04 21 1d 17 1e` | bundle | recording mode plus button function, always written together in this order |

| state | 1B | 04 | 21 | 1D | 17 | 1E |
|---|---|---|---|---|---|---|
| recording off, button Mute | 0 | 0 | 0 | 0 | 0 | 1 |
| recording off, button Marker | 0 | 0 | 0 | 1 | 1 | 1 |
| recording off, button disabled | 0 | 0 | 0 | 0 | 1 | 1 |
| Manual (button starts and stops recording) | 0 | 0 | 1 | 0 | 1 | 1 |
| Always | 1 | 1 | 0 | 0 | 1 | 1 |
| Backup | 0 | 1 | 0 | 0 | 1 | 1 |

### TX status blob (`09 0b 01`, 28 bytes after the ACK byte)

```
offset  meaning
0-3     TX serial, u32 LE
4-7     a unix timestamp (2024-12-22 on the test unit): build date or last sync
8-9     firmware, 50 02 = 2.5.0
15      recording enabled (Always or Backup)
16      1 = uncompressed
17      1 = Always
18      LED byte (DF bright, FE dim)
20      button = marker
21      button is not mute
22      TX battery percent
23      0x29 constant, unknown
26      pulses to 1 briefly after entering Manual mode, possibly "recording"
27      0x16 constant, unknown
```

### Reports 0x01 (17 bytes) and 0x0F (64 bytes)

`01 36` and `01 77` are sent at connect and get a bare ACK. `01 42` returns
`02 42 41 01`. `0f 00 01` returns `10 00 41 00` followed by the serial as u32 LE.

Other RODE models share the framing (Wireless PRO, Interview PRO; see
[lgnap/rode-raconteur](https://github.com/lgnap/rode-raconteur) and
[LinuxRenaissance/krode](https://github.com/LinuxRenaissance/krode)), but their
register maps differ. On those models `01 4A 01` erases all onboard recordings.
Open Rode never sends it.

## Repository layout

```
src/protocol.rs   opcodes, enums, frame builders, TX status decoding
src/hid.rs        hidraw transport (hidapi)
src/device.rs     typed getters and setters for the GO II
src/main.rs       egui GUI
src/bin/probe.rs  open-rode-cli
tools/            capture of RODE Central's HID traffic and the scripts used to decode it
packaging/arch/   PKGBUILD
udev/             udev rule
```

## Contributing

Issues and pull requests are welcome, especially captures from other RODE
devices (Wireless PRO, Wireless ME, Wireless Micro, GO gen 3). The tools in
`tools/` show how to record RODE Central under Wine and diff what each click
sends. Please verify any new opcode with `open-rode-cli get` or `send` before
wiring it into the GUI, and never replay opcodes from the firmware-update
reports.

## Credits

Written by areoseek with Claude (Anthropic). Prior work that made this quicker:
[christianmeurer/rode-wireless-go-ctl](https://github.com/christianmeurer/rode-wireless-go-ctl),
[lgnap/rode-raconteur](https://github.com/lgnap/rode-raconteur) and
[LinuxRenaissance/krode](https://github.com/LinuxRenaissance/krode).

## License

MIT. See `LICENSE`.
