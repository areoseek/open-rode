# rode-central-linux

Native Linux control app for the **RØDE Wireless GO II** (receiver on USB,
transmitters through it). Rust, `hidapi` + `egui`. No Wine, no kernel driver.

RØDE's own RODE Central is Windows/macOS only. It is a Flutter shell over
`rode_bridge.dll`, which talks to the devices with vendor-specific **USB HID
reports** — so the whole thing maps onto `/dev/hidrawN`. The protocol below was
captured from RODE Central's real traffic (run once under Wine with
`WINEDEBUG=+hid`, every control clicked) and then replayed against the
hardware from this code. See `tools/` for the capture and the parser.

Tested: Wireless GO II RX firmware 2.5.0 (USB `19f7:002a`), one TX paired.

## Install

```sh
# user access to the HID node (the node is root:root 0600 by default)
sudo install -m 0644 udev/70-rode.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules   # then re-plug the receiver

cargo build --release
./target/release/rode-central          # GUI
./target/release/rode-probe status     # CLI
```

## What works

| Receiver | Transmitter (via RX) |
|---|---|
| battery, serial, name, clock (+ sync) | battery, firmware, serial |
| channel routing: Split / Merged | on-board recording: Off / Always / Manual / Backup |
| safety channel | compress recordings |
| output gain: Coarse (3 levels) / Fine (−30…0 dB, 3 dB steps) / Camera | Ø button: Disabled / Mute / Marker / Record |
| Ø button: toggle auto-dim / drop marker | LEDs: Bright / Dim |
| auto-dim | input PAD |

Not done: firmware update (reports 0x05/0x07 — deliberately untouched), factory
reset, rename, diagnostics, recordings browser (needs the TX on a cable as mass
storage; the files are plain WAV).

## CLI

```
rode-probe status
rode-probe gain fine 8            # -6 dB
rode-probe channel merged --safety
rode-probe tx-record backup --button marker
rode-probe tx-led dim
rode-probe get 0b                 # raw register read
rode-probe send "09 00 01" --len 37
```

## Protocol (Wireless GO II RX, fw 2.5.0)

Vendor HID, usage page `FF00`, one interface, interrupt IN only; writes go out
as `SET_REPORT` on the control pipe (hidraw does this for you). Every OUT
report `n` is answered on IN report `n+1`, opcode echoed, then `41` ACK or
`4E` NAK.

Report sizes (from the descriptor): `01/02` 17 B, `03/04` 17/13 B,
`05` 1073 B and `07` 1537 B (firmware upload), `09/0A` 37 B, `0B` 61 B
(unsolicited), `0D/0E` and `0F/10` 64 B.

### Report 0x09 — register file (37 bytes)

```
GET   09 <op> 01                 ->  0a <op> 41 <data…>
SET   09 <op> 00 <params…>       ->  0a <op> 41 <echo>
TX    09 0d <reg> <val…>         ->  0a 0d 41 <reg>        (forwarded to the paired TX)
```

| op | GET returns | SET params | meaning |
|---|---|---|---|
| `00` | `[pct]` | – | RX battery % |
| `01` | `[u32 LE]` | `[u32 LE]` | RX clock (unix). RODE Central sets it on connect |
| `02` | `[u32 LE]` | – | second timestamp (≈ clock − 45 s), purpose TBD |
| `04` | `[0\|1]` | `[0\|1]` | auto-dim |
| `05` | `[mode, val]` | `[mode, val]` | output gain: mode 0 Coarse (val 0–2), 1 Fine (val 0–10 = −30…0 dB), 2 Camera |
| `06` | `[0\|1]` | `[0\|1]` | 1 = Merged, 0 = Split |
| `07` | `[0\|1]` | `[0\|1]` | safety channel |
| `08` | `[slot, len, ascii…]` | – | name table; slot 0 = device name (`WIGO2`), 1–9 empty (`ff`) |
| `0a` | `[0\|1]` | `[0\|1]` | RX Ø button: 0 toggles auto-dim, 1 drops a marker |
| `0b` | 28 B | – | paired-TX status blob (below) |
| `0c` | NAK | | not on RX |
| `0d` | – | `[reg, val]` | write TX register |
| `0f` | empty | – | polled every cycle by RODE Central |
| `14` | `[0\|1]` | `[0\|1]` | sent with `06`, inverse value (1 when Split) |
| `16` | `00 16` | | ? |
| `18`,`19` | `02` | | ? |
| `1a` | – | – | "refresh", sent periodically and after changes |
| `22` | `42` | | ? |

RODE Central's idle loop: GET `00 01 04 05 06 0b 0c 0f` round-robin, ~4 Hz,
plus `1a` every ~2 min.

### TX registers (through `09 0d`)

| reg | value | meaning |
|---|---|---|
| `06` | u32 LE | TX clock |
| `08` | 0/1 | input PAD |
| `18` | 0/1 | compressed recordings |
| `1a` | `DF` / `FE` | LEDs bright / dim |
| `1b 04 21 1d 17 1e` | bundle | recording mode + Ø button, always written together in this order: |

| state | 1B | 04 | 21 | 1D | 17 | 1E |
|---|---|---|---|---|---|---|
| rec off, button Mute | 0 | 0 | 0 | 0 | 0 | 1 |
| rec off, button Marker | 0 | 0 | 0 | 1 | 1 | 1 |
| rec off, button disabled | 0 | 0 | 0 | 0 | 1 | 1 |
| Manual (button = record) | 0 | 0 | 1 | 0 | 1 | 1 |
| Always | 1 | 1 | 0 | 0 | 1 | 1 |
| Backup | 0 | 1 | 0 | 0 | 1 | 1 |

### TX status blob (`09 0b 01`, 28 bytes after the ACK)

```
off  0-3   TX serial, u32 LE
     4-7   a unix timestamp (2024-12-22 on this unit) — build date or last sync
     8-9   firmware, BCD-ish: 50 02 = 2.5.0
     15    recording enabled (Always/Backup)
     16    1 = uncompressed
     17    1 = Always
     18    LED byte (DF bright / FE dim)
     20    button = marker
     21    button not mute
     22    TX battery %
     23    0x29 constant — ?
     26    pulses to 1 briefly after entering Manual/Record — "recording"?
     27    0x16 constant — ?
```

### Report 0x01 (17 B) and 0x0F (64 B)

`01 36`, `01 77`: sent at connect, bare ACK. `01 42` → `02 42 41 01`.
`0f 00 01` → `10 00 41 00 <serial u32 LE>`.

Other RØDE models share the framing (Wireless PRO, Interview PRO — see
`lgnap/rode-raconteur` and `LinuxRenaissance/krode`), but their register maps
differ. `01 4A 01` on those erases all onboard recordings; this app never sends it.

## tools/

* `parse_wine_hid.py` — pairs OUT/IN reports from a `WINEDEBUG=+hid` log
  (thread-aware), `--all --nopoll --since N` to isolate a click.
* `click_and_diff.py` — drives RODE Central under Wine with a uinput tablet and
  prints what each click sent.
* `sweep_get.py` — read-only GET sweep of report 9.
* `capture-wgo2-rx-2.5.0.log.txt.gz` — the capture this was derived from.
