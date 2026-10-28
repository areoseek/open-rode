//! Shared protocol + transport layer for RØDE wireless devices.
//!
//! Transport: vendor HID (usage page 0xFF00) on /dev/hidrawN.
//! Framing (from firmware report descriptors + prior reverse engineering):
//!   OUT report `n` is answered on IN report `n+1`.
//!   id 0x01 / 17 B : [01][opcode][param...]  -> id 0x02 : [02][opcode][0x41 ACK | 0x4E NAK][data...]
//!   id 0x09 / 37 B : status polling family  -> id 0x0A
//!   id 0x0F / 64 B : extended queries       -> id 0x10
//! Opcodes are filled in per device family from captured RODE Central traffic.

pub mod audio;
pub mod device;
pub mod hid;
pub mod protocol;
