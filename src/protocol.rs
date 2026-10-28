//! Wire protocol for the RØDE Wireless GO II receiver (VID 19F7, PID 002A,
//! firmware 2.5.0). Everything here was captured from RODE Central's own HID
//! traffic (`tools/parse_wine_hid.py`) and replayed against the hardware.
//!
//! Transport: vendor HID (usage page FF00). Every OUT report `n` is answered on
//! IN report `n+1`, with the opcode echoed and an ACK (0x41) / NAK (0x4E) byte.
//!
//! Report 0x09 (37 bytes) is the settings/status register file:
//!
//! ```text
//!   OUT 09 <op> 01                -> IN 0a <op> 41 <data...>     GET
//!   OUT 09 <op> 00 <params...>    -> IN 0a <op> 41 <echo...>     SET
//!   OUT 09 0d <reg> <val>         -> IN 0a 0d 41 <reg>           SET on the paired TX
//! ```
//!
//! Report 0x01 (17 bytes) carries one-shot commands (`01 <op> <param>`), report
//! 0x0f (64 bytes) extended queries. Reports 0x05/0x07 (1073/1537 bytes) are the
//! firmware upload path and are deliberately not modelled.

pub const ACK: u8 = 0x41;
pub const NAK: u8 = 0x4E;

pub const REPORT_CMD: u8 = 0x01; // 17 B
pub const REPORT_REG: u8 = 0x09; // 37 B
pub const REPORT_EXT: u8 = 0x0F; // 64 B
pub const LEN_CMD: usize = 17;
pub const LEN_REG: usize = 37;
pub const LEN_EXT: usize = 64;

/// Receiver registers (report 0x09 opcodes).
pub mod rx {
    /// GET -> [percent]. RX battery.
    pub const BATTERY: u8 = 0x00;
    /// GET -> [u32 LE unix time]; SET [u32 LE]. RX clock.
    pub const CLOCK: u8 = 0x01;
    /// GET -> [u32 LE]. Another timestamp (observed ≈ clock − 45 s; purpose TBD).
    pub const CLOCK2: u8 = 0x02;
    /// SET [0|1]. Auto-dim the RX screen after 10 s.
    pub const AUTO_DIM: u8 = 0x04;
    /// GET -> [mode, value]; SET [mode, value]. Output gain, see `GainMode`.
    pub const GAIN: u8 = 0x05;
    /// SET [0|1]. 1 = Merged (mono mix), 0 = Split (TX1 L / TX2 R).
    pub const MERGED: u8 = 0x06;
    /// SET [0|1]. Safety channel (−20 dB copy on the right channel).
    pub const SAFETY: u8 = 0x07;
    /// GET [slot] -> [slot, len, ascii...]; slot 0 is the device name ("WIGO2").
    pub const NAME: u8 = 0x08;
    /// SET [0|1]. Ø button on the RX: 0 = toggles auto-dim, 1 = drops a marker.
    pub const BUTTON: u8 = 0x0A;
    /// GET -> 28-byte status blob for the paired TX, see `TxStatus`.
    pub const TX_STATUS: u8 = 0x0B;
    /// SET [reg, val]. Write a register on the paired TX, see `tx`.
    pub const TX_REG: u8 = 0x0D;
    /// Polled by RODE Central every cycle; always empty ACK. Purpose TBD.
    pub const POLL_0F: u8 = 0x0F;
    /// SET [0|1]. Sent alongside MERGED with the inverse value (1 when Split).
    pub const SPLIT_FLAG: u8 = 0x14;
    /// SET (no params). Sent periodically and after mode changes; refresh/commit.
    pub const REFRESH: u8 = 0x1A;
}

/// Transmitter registers, written through `rx::TX_REG`.
pub mod tx {
    /// Clock, u32 LE unix time.
    pub const CLOCK: u8 = 0x06;
    /// 0|1. Input pad.
    pub const PAD: u8 = 0x08;
    /// Ø button starts/stops recording (Manual record mode).
    pub const BTN_RECORD: u8 = 0x21;
    /// Ø button drops a marker.
    pub const BTN_MARKER: u8 = 0x1D;
    /// 0 only while the Ø button mutes; 1 otherwise.
    pub const BTN_NOT_MUTE: u8 = 0x17;
    /// Always 1 in every captured bundle.
    pub const BTN_FLAG_1E: u8 = 0x1E;
    /// On-board recording enabled (Always / Backup).
    pub const REC_ENABLED: u8 = 0x04;
    /// Record continuously (Always); 0 for Backup.
    pub const REC_ALWAYS: u8 = 0x1B;
    /// 0|1. Compressed recordings.
    pub const REC_COMPRESS: u8 = 0x18;
    /// LED brightness: 0xDF bright, 0xFE dim.
    pub const LED: u8 = 0x1A;
}

/// One-shot commands on report 0x01.
pub mod cmd {
    /// Sent twice at connect by RODE Central; bare ACK.
    pub const HELLO_36: u8 = 0x36;
    pub const HELLO_77: u8 = 0x77;
    /// Sent once at connect; replies `42 41 01`.
    pub const QUERY_42: u8 = 0x42;
    /// Interview PRO / Wireless PRO: erase all onboard recordings. Not sent by
    /// this app; kept for reference. DESTRUCTIVE.
    pub const ERASE_RECORDINGS: u8 = 0x4A;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainMode {
    /// value 0..=2 (3 pad levels)
    Coarse = 0,
    /// value 0..=10, 3 dB steps: 0 = −30 dB … 10 = 0 dB
    Fine = 1,
    /// camera preset
    Camera = 2,
}

impl GainMode {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => GainMode::Coarse,
            1 => GainMode::Fine,
            2 => GainMode::Camera,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            GainMode::Coarse => "Coarse",
            GainMode::Fine => "Fine",
            GainMode::Camera => "Camera",
        }
    }
}

/// Fine-gain step (0..=10) to dB.
pub fn fine_step_db(step: u8) -> i32 {
    -30 + 3 * step as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    Split,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxButton {
    AutoDim = 0,
    Marker = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxLed {
    Bright = 0xDF,
    Dim = 0xFE,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxButton {
    Disabled,
    Mute,
    Marker,
    Record,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordMode {
    Off,
    Always,
    /// Ø button starts/stops recording
    Manual,
    Backup,
}

/// The six-register bundle RODE Central writes (in this order) whenever the
/// TX button function or recording mode changes. Captured combinations:
///
/// | state              | 1B | 04 | 21 | 1D | 17 | 1E |
/// |--------------------|----|----|----|----|----|----|
/// | rec off, Mute      | 0  | 0  | 0  | 0  | 0  | 1  |
/// | rec off, Marker    | 0  | 0  | 0  | 1  | 1  | 1  |
/// | rec off, disabled  | 0  | 0  | 0  | 0  | 1  | 1  |
/// | Manual (Record)    | 0  | 0  | 1  | 0  | 1  | 1  |
/// | Always             | 1  | 1  | 0  | 0  | 1  | 1  |
/// | Backup             | 0  | 1  | 0  | 0  | 1  | 1  |
pub fn tx_button_bundle(rec: RecordMode, btn: TxButton) -> [(u8, u8); 6] {
    let (always, enabled) = match rec {
        RecordMode::Off | RecordMode::Manual => (0, 0),
        RecordMode::Always => (1, 1),
        RecordMode::Backup => (0, 1),
    };
    let record = (rec == RecordMode::Manual) as u8;
    let (marker, not_mute) = match btn {
        TxButton::Mute => (0, 0),
        TxButton::Marker => (1, 1),
        TxButton::Disabled | TxButton::Record => (0, 1),
    };
    [
        (tx::REC_ALWAYS, always),
        (tx::REC_ENABLED, enabled),
        (tx::BTN_RECORD, record),
        (tx::BTN_MARKER, marker),
        (tx::BTN_NOT_MUTE, not_mute),
        (tx::BTN_FLAG_1E, 1),
    ]
}

/// Decoded `rx::TX_STATUS` blob (28 bytes after the ACK byte).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxStatus {
    pub serial: u32,
    /// Bytes 4..8. A unix timestamp (2024-12-22 on this unit); build date or
    /// last-sync time, not yet confirmed.
    pub timestamp: u32,
    pub fw_major: u8,
    pub fw_minor: u8,
    pub rec_enabled: bool,
    pub uncompressed: bool,
    pub rec_always: bool,
    pub led: u8,
    pub btn_marker: bool,
    pub btn_not_mute: bool,
    pub battery: u8,
    /// Byte 23, constant 0x29 on this unit; meaning TBD.
    pub byte23: u8,
    /// Byte 26, pulses to 1 briefly after switching to Manual/Record. Possibly
    /// "recording in progress".
    pub flag26: bool,
    pub raw: [u8; 28],
}

impl TxStatus {
    pub fn parse(d: &[u8]) -> Option<Self> {
        if d.len() < 28 {
            return None;
        }
        let mut raw = [0u8; 28];
        raw.copy_from_slice(&d[..28]);
        Some(TxStatus {
            serial: u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
            timestamp: u32::from_le_bytes([d[4], d[5], d[6], d[7]]),
            fw_major: d[9],
            fw_minor: d[8] >> 4,
            rec_enabled: d[15] != 0,
            uncompressed: d[16] != 0,
            rec_always: d[17] != 0,
            led: d[18],
            btn_marker: d[20] != 0,
            btn_not_mute: d[21] != 0,
            battery: d[22],
            byte23: d[23],
            flag26: d[26] != 0,
            raw,
        })
    }

    pub fn firmware(&self) -> String {
        format!("{}.{}.{}", self.fw_major, self.fw_minor, self.raw[8] & 0x0F)
    }

    pub fn record_mode(&self) -> RecordMode {
        match (self.rec_enabled, self.rec_always) {
            (true, true) => RecordMode::Always,
            (true, false) => RecordMode::Backup,
            // Manual is only visible through the button registers.
            (false, _) => RecordMode::Off,
        }
    }

    pub fn button(&self) -> TxButton {
        match (self.btn_marker, self.btn_not_mute) {
            (true, _) => TxButton::Marker,
            (false, false) => TxButton::Mute,
            (false, true) => TxButton::Disabled, // or Record (Manual mode)
        }
    }
}

/// Build a fixed-size frame: [report_id][payload...][zero pad].
pub fn frame(report_id: u8, total_len: usize, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; total_len];
    f[0] = report_id;
    f[1..1 + payload.len()].copy_from_slice(payload);
    f
}

pub fn reg_get(op: u8) -> Vec<u8> {
    frame(REPORT_REG, LEN_REG, &[op, 0x01])
}

pub fn reg_set(op: u8, params: &[u8]) -> Vec<u8> {
    let mut p = vec![op, 0x00];
    p.extend_from_slice(params);
    frame(REPORT_REG, LEN_REG, &p)
}

pub fn tx_reg_set(reg: u8, val: u8) -> Vec<u8> {
    frame(REPORT_REG, LEN_REG, &[rx::TX_REG, reg, val])
}

/// Known product IDs under VID 0x19F7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    WirelessGo2Rx,
    WirelessProTx,
    WirelessProRx,
    WirelessProChargeCase,
    InterviewProTx,
    Unknown(u16),
}

impl Model {
    pub fn from_pid(pid: u16) -> Self {
        match pid {
            0x001F | 0x002A => Model::WirelessGo2Rx,
            0x0056 => Model::WirelessProTx,
            0x0058 => Model::WirelessProRx,
            0x007A => Model::WirelessProChargeCase,
            0x0063 | 0x0068 => Model::InterviewProTx,
            p => Model::Unknown(p),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Model::WirelessGo2Rx => "Wireless GO II RX".into(),
            Model::WirelessProTx => "Wireless PRO TX".into(),
            Model::WirelessProRx => "Wireless PRO RX".into(),
            Model::WirelessProChargeCase => "Wireless PRO Charge Case+".into(),
            Model::InterviewProTx => "Interview PRO TX".into(),
            Model::Unknown(p) => format!("Unknown RØDE device (PID 0x{p:04X})"),
        }
    }
}
