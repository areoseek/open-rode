//! High-level Wireless GO II receiver: typed getters/setters over `RodeHid`.

use crate::hid::{enumerate, RodeHid, RodeHidInfo};
use crate::protocol::{self as p, rx, tx, ChannelMode, GainMode, RecordMode, RxButton, TxButton, TxLed, TxStatus};
use anyhow::{anyhow, bail, Result};
use hidapi::HidApi;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(1500);

/// The devices keep *local* wall-clock time as a unix-style number (that is
/// what RODE Central writes), so "now" for them is UTC shifted by the zone.
pub fn device_now() -> u32 {
    let now = chrono::Local::now();
    (now.timestamp() + now.offset().local_minus_utc() as i64) as u32
}

pub struct GoII {
    hid: RodeHid,
}

/// Everything the UI shows, refreshed by `GoII::snapshot`.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub serial: u32,
    pub name: String,
    pub rx_battery: u8,
    pub rx_clock: u32,
    pub gain_mode: Option<GainMode>,
    pub gain_value: u8,
    pub merged: bool,
    pub safety: bool,
    pub auto_dim: bool,
    pub rx_button: RxButton,
    pub tx: Option<TxStatus>,
}

impl GoII {
    pub fn find(api: &HidApi) -> Result<Vec<RodeHidInfo>> {
        Ok(enumerate(api)
            .into_iter()
            .filter(|d| matches!(p::Model::from_pid(d.pid), p::Model::WirelessGo2Rx))
            .collect())
    }

    pub fn open(api: &HidApi, info: &RodeHidInfo) -> Result<Self> {
        Ok(GoII { hid: RodeHid::open(api, info)? })
    }

    pub fn info(&self) -> &RodeHidInfo {
        &self.hid.info
    }

    // -- raw register access ------------------------------------------------

    /// GET a report-9 register; returns the data after the ACK byte.
    pub fn get(&self, op: u8) -> Result<Vec<u8>> {
        let r = self.hid.transact(&p::reg_get(op), p::LEN_REG, TIMEOUT)?;
        check(&r, p::REPORT_REG + 1, op)?;
        Ok(r[3..].to_vec())
    }

    /// SET a report-9 register.
    pub fn set(&self, op: u8, params: &[u8]) -> Result<()> {
        let r = self.hid.transact(&p::reg_set(op, params), p::LEN_REG, TIMEOUT)?;
        check(&r, p::REPORT_REG + 1, op)
    }

    /// Write one register on the paired transmitter.
    pub fn set_tx(&self, reg: u8, val: u8) -> Result<()> {
        let r = self.hid.transact(&p::tx_reg_set(reg, val), p::LEN_REG, TIMEOUT)?;
        check(&r, p::REPORT_REG + 1, rx::TX_REG)
    }

    // -- receiver ------------------------------------------------------------

    pub fn serial(&self) -> Result<u32> {
        let r = self.hid.transact(&p::frame(p::REPORT_EXT, p::LEN_EXT, &[0x00, 0x01]), p::LEN_EXT, TIMEOUT)?;
        if r.len() < 8 || r[0] != p::REPORT_EXT + 1 || r[2] != p::ACK {
            bail!("bad serial reply {}", hex::encode(&r));
        }
        Ok(u32::from_le_bytes([r[4], r[5], r[6], r[7]]))
    }

    pub fn rx_battery(&self) -> Result<u8> {
        Ok(*self.get(rx::BATTERY)?.first().unwrap_or(&0))
    }

    pub fn rx_clock(&self) -> Result<u32> {
        let d = self.get(rx::CLOCK)?;
        Ok(u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
    }

    pub fn set_rx_clock(&self, unix: u32) -> Result<()> {
        self.set(rx::CLOCK, &unix.to_le_bytes())
    }

    /// Device name (slot 0 of the name table).
    pub fn name(&self) -> Result<String> {
        let d = self.get(rx::NAME)?; // [slot, len, chars...]
        let n = *d.get(1).unwrap_or(&0) as usize;
        let s = d.get(2..2 + n).unwrap_or(&[]);
        Ok(String::from_utf8_lossy(s).trim_end_matches('\0').to_string())
    }

    pub fn gain(&self) -> Result<(Option<GainMode>, u8)> {
        let d = self.get(rx::GAIN)?;
        Ok((GainMode::from_u8(d[0]), d[1]))
    }

    pub fn set_gain(&self, mode: GainMode, value: u8) -> Result<()> {
        let max = match mode {
            GainMode::Coarse => 2,
            GainMode::Fine => 10,
            GainMode::Camera => 0,
        };
        if value > max {
            bail!("gain value {value} out of range for {mode:?} (max {max})");
        }
        self.set(rx::GAIN, &[mode as u8, value])
    }

    /// Mirrors RODE Central exactly: MERGED, then the inverse SPLIT_FLAG,
    /// and SAFETY off when leaving safety mode.
    pub fn set_channel_mode(&self, mode: ChannelMode, safety: bool) -> Result<()> {
        let merged = (mode == ChannelMode::Merged) as u8;
        self.set(rx::MERGED, &[merged])?;
        self.set(rx::SPLIT_FLAG, &[1 - merged])?;
        self.set(rx::SAFETY, &[safety as u8])
    }

    pub fn set_safety(&self, on: bool) -> Result<()> {
        self.set(rx::SAFETY, &[on as u8])
    }

    pub fn set_auto_dim(&self, on: bool) -> Result<()> {
        self.set(rx::AUTO_DIM, &[on as u8])
    }

    pub fn set_rx_button(&self, b: RxButton) -> Result<()> {
        self.set(rx::BUTTON, &[b as u8])
    }

    pub fn refresh(&self) -> Result<()> {
        self.set(rx::REFRESH, &[])
    }

    // -- transmitter (via the RX) --------------------------------------------

    pub fn tx_status(&self) -> Result<Option<TxStatus>> {
        let d = self.get(rx::TX_STATUS)?;
        Ok(TxStatus::parse(&d).filter(|s| s.serial != 0))
    }

    pub fn set_tx_pad(&self, on: bool) -> Result<()> {
        self.set_tx(tx::PAD, on as u8)
    }

    pub fn set_tx_led(&self, led: TxLed) -> Result<()> {
        self.set_tx(tx::LED, led as u8)
    }

    pub fn set_tx_compress(&self, on: bool) -> Result<()> {
        self.set_tx(tx::REC_COMPRESS, on as u8)
    }

    pub fn set_tx_clock(&self, unix: u32) -> Result<()> {
        let mut params = vec![tx::CLOCK];
        params.extend_from_slice(&unix.to_le_bytes());
        let r = self.hid.transact(&p::frame(p::REPORT_REG, p::LEN_REG, &[&[rx::TX_REG][..], &params].concat()), p::LEN_REG, TIMEOUT)?;
        check(&r, p::REPORT_REG + 1, rx::TX_REG)
    }

    /// Record mode + Ø button function are one bundle on the TX.
    pub fn set_tx_record_and_button(&self, rec: RecordMode, btn: TxButton) -> Result<()> {
        for (reg, val) in p::tx_button_bundle(rec, btn) {
            self.set_tx(reg, val)?;
        }
        Ok(())
    }

    // -- aggregate -----------------------------------------------------------

    fn get_bool(&self, op: u8) -> Result<bool> {
        Ok(self.get(op)?.first().copied().unwrap_or(0) != 0)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let (gain_mode, gain_value) = self.gain()?;
        Ok(Snapshot {
            serial: self.serial()?,
            name: self.name()?,
            rx_battery: self.rx_battery()?,
            rx_clock: self.rx_clock()?,
            gain_mode,
            gain_value,
            merged: self.get_bool(rx::MERGED)?,
            safety: self.get_bool(rx::SAFETY)?,
            auto_dim: self.get_bool(rx::AUTO_DIM)?,
            rx_button: if self.get_bool(rx::BUTTON)? { RxButton::Marker } else { RxButton::AutoDim },
            tx: self.tx_status()?,
        })
    }
}

fn check(r: &[u8], want_id: u8, op: u8) -> Result<()> {
    if r.len() < 3 {
        return Err(anyhow!("short reply {}", hex::encode(r)));
    }
    if r[0] != want_id || r[1] != op {
        return Err(anyhow!("unexpected reply {} to op {op:#04x}", hex::encode(r)));
    }
    match r[2] {
        p::ACK => Ok(()),
        p::NAK => Err(anyhow!("device NAKed op {op:#04x}")),
        s => Err(anyhow!("unknown status {s:#04x} for op {op:#04x}")),
    }
}
