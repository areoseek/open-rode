use anyhow::{anyhow, Context, Result};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::time::Duration;

pub const RODE_VID: u16 = 0x19F7;

#[derive(Debug, Clone)]
pub struct RodeHidInfo {
    pub path: std::ffi::CString,
    pub pid: u16,
    pub serial: String,
    pub product: String,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
}

impl From<&DeviceInfo> for RodeHidInfo {
    fn from(d: &DeviceInfo) -> Self {
        RodeHidInfo {
            path: d.path().to_owned(),
            pid: d.product_id(),
            serial: d.serial_number().unwrap_or("").to_string(),
            product: d.product_string().unwrap_or("").to_string(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            interface: d.interface_number(),
        }
    }
}

/// Every RØDE HID interface currently attached.
pub fn enumerate(api: &HidApi) -> Vec<RodeHidInfo> {
    api.device_list()
        .filter(|d| d.vendor_id() == RODE_VID)
        .map(RodeHidInfo::from)
        .collect()
}

pub struct RodeHid {
    dev: HidDevice,
    pub info: RodeHidInfo,
}

impl RodeHid {
    pub fn open(api: &HidApi, info: &RodeHidInfo) -> Result<Self> {
        let dev = api
            .open_path(&info.path)
            .with_context(|| format!("open {:?} (udev rule installed?)", info.path))?;
        dev.set_blocking_mode(true)?;
        Ok(RodeHid { dev, info: info.clone() })
    }

    /// The raw HID report descriptor, read through hidraw.
    pub fn report_descriptor(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; 4096];
        let n = self.dev.get_report_descriptor(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Send one output report (first byte = report id) and read one input report.
    pub fn transact(&self, out: &[u8], in_len: usize, timeout: Duration) -> Result<Vec<u8>> {
        let n = self.dev.write(out)?;
        if n != out.len() {
            return Err(anyhow!("short write {n}/{}", out.len()));
        }
        let mut buf = vec![0u8; in_len.max(64)];
        let n = self
            .dev
            .read_timeout(&mut buf, timeout.as_millis() as i32)?;
        if n == 0 {
            return Err(anyhow!("no response within {:?}", timeout));
        }
        buf.truncate(n);
        Ok(buf)
    }

    /// Read one unsolicited / pending input report.
    pub fn read(&self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; 1600];
        let n = self
            .dev
            .read_timeout(&mut buf, timeout.as_millis() as i32)?;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }
}
