//! MeltAlarm source for MSI MPG Ai1300TS / Ai1600TS power supplies (GPU Safeguard+).
//!
//! Read-only by construction: the only bytes this crate can send come from `protocol::packet`
//! over the closed `Request` enum (8 reads + MSI's connect handshake), and they reach the
//! device through a single call site in `transport` (docs/ARCHITECTURE.md §9).

mod lock;
pub mod protocol;
mod transport;

use std::time::{Duration, Instant};

use meltalarm_model::{Capabilities, ConnectorInfo, ConnectorReading, Protection, Report, SourceId, SourceInfo, Verdict};
use meltalarm_source_api::hidapi::HidDevice;
use meltalarm_source_api::{Discovery, Driver, HidContext, Source};

use lock::PsuLock;
use protocol::{PID_AI1300TS, PID_AI1600TS, Reg, Request, VID, device_status};
pub use transport::TxError;

const CONFIG_REFRESH: Duration = Duration::from_secs(60);

pub struct MsiDriver;

impl Driver for MsiDriver {
    fn name(&self) -> &'static str {
        "MSI MPG Ai1300TS / Ai1600TS"
    }

    fn discover(&self, hid: &mut HidContext) -> Discovery {
        let api = match hid.refreshed() {
            Ok(api) => api,
            Err(e) => return Discovery::NotReady(format!("USB HID is not available ({e})")),
        };
        // The product id identifies a Safeguard+ (TS) model: MSI Center and the Afterburner
        // plugin select the TS protocol by these PIDs alone.
        let Some(dev_info) = api.device_list().find(|d| d.vendor_id() == VID && [PID_AI1300TS, PID_AI1600TS].contains(&d.product_id()))
        else {
            return Discovery::NotPresent;
        };
        let pid = dev_info.product_id();
        let device = match dev_info.open_device(api) {
            Ok(d) => d,
            Err(e) => return Discovery::NotReady(format!("PSU found but could not be opened ({e})")),
        };
        let lock = match PsuLock::platform() {
            Ok(l) => l,
            Err(msg) => return Discovery::Unusable(msg),
        };
        match MsiSource::identify(device, lock, pid) {
            Ok(src) => Discovery::Found(Box::new(src)),
            Err(e) => Discovery::NotReady(format!("PSU found but not answering ({e:?})")),
        }
    }
}

pub struct MsiSource {
    device: HidDevice,
    lock: PsuLock,
    info: SourceInfo,
    last_config: Option<Instant>,
}

impl MsiSource {
    /// MSI's connect sequence (`CONNECT_PSU`): handshake, then identity. The PSU must answer to
    /// count as found (source rule 8). Its model string is only for display.
    fn identify(device: HidDevice, lock: PsuLock, pid: u16) -> Result<Self, TxError> {
        transport::transact(&device, &lock, Request::Handshake)?;
        let text = |reg| transport::transact(&device, &lock, Request::Read(reg)).map(|f| f.text());
        let (id, default_model) = if pid == PID_AI1600TS { ("msi:ai1600ts", "MPG Ai1600TS") } else { ("msi:ai1300ts", "MPG Ai1300TS") };
        let reported = text(Reg::MfrModel)?;
        let model = if reported.starts_with("MPG") { reported } else { default_model.to_owned() };
        let info = SourceInfo {
            id: SourceId(id.into()),
            vendor: text(Reg::MfrId).unwrap_or_else(|_| "MSI".into()),
            model,
            firmware: text(Reg::MfrRevision).ok().filter(|s| !s.is_empty()),
            serial: text(Reg::MfrSerial).ok().filter(|s| !s.is_empty()),
            connectors: (0..2).map(|i| ConnectorInfo { index: i }).collect(),
            caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
            protection_name: Some("Safeguard+".into()),
        };
        Ok(MsiSource { device, lock, info, last_config: None })
    }

    fn read(&self, reg: Reg) -> Result<protocol::Frame, TxError> {
        transport::transact(&self.device, &self.lock, Request::Read(reg))
    }

    fn read_protection(&mut self, now: Instant) -> Option<Protection> {
        let due = self.last_config.is_none_or(|t| now.duration_since(t) >= CONFIG_REFRESH);
        if !due {
            return None;
        }
        let p = self.read(Reg::SafeguardConfig).ok()?.protection();
        self.last_config = Some(now);
        Some(p)
    }
}

impl Source for MsiSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn poll(&mut self, _hid: &mut HidContext) -> Report {
        let at = Instant::now();
        let telemetry = self.read(Reg::Telemetry);
        let status = self.read(Reg::SafeguardStatus);
        let flags = self.read(Reg::ProtectionFlags).map(|f| f.flags());
        let protection = self.read_protection(at);

        let readings = telemetry.ok().map(|f| {
            let w = f.wire_currents();
            (0..2).map(|i| ConnectorReading { index: i as u8, wires: w[i].map(Some) }).collect()
        });
        let verdicts = status.as_ref().ok().map(|f| {
            let codes = f.status_codes();
            (0..2)
                .map(|i| Verdict {
                    index: i as u8,
                    status: device_status(codes[i]),
                    // Wire flags are secondary detail: if E1 failed this tick, show none.
                    flagged: flags.as_ref().map(|fl| fl.wires[i]).unwrap_or([false; 6]),
                })
                .collect()
        });
        let diagnostic = status.as_ref().ok().filter(|f| f.status_codes() != [0, 0]).map(|f| format!("C1 {}", f.hex()));

        Report { at, readings, verdicts, faults: flags.ok().map(|f| f.faults()), protection, diagnostic }
    }
}
