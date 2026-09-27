//! MeltAlarm source for MSI MPG Ai1300TS / Ai1600TS power supplies (GPU Safeguard+).
//!
//! Read-only by construction: the only request bytes this crate can build come from
//! `protocol::read_request` over the closed `Reg` enum, and they reach the device through
//! a single call site in `transport` (docs/ARCHITECTURE.md §9).

pub mod protocol;
mod lock;
mod transport;

use std::time::{Duration, Instant};

use meltalarm_model::{Capabilities, ConnectorInfo, ConnectorReading, Protection, Report, SourceId, SourceInfo, Verdict};
use meltalarm_source_api::hidapi::HidDevice;
use meltalarm_source_api::{Discovery, Driver, HidContext, Source};

use lock::PsuLock;
use protocol::{PID_AI1300TS, PID_AI1600TS, Reg, VID, device_status};
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
            Err(_) => return Discovery::NotPresent,
        };
        let Some(dev_info) =
            api.device_list().find(|d| d.vendor_id() == VID && [PID_AI1300TS, PID_AI1600TS].contains(&d.product_id()))
        else {
            return Discovery::NotPresent;
        };
        let device = match dev_info.open_device(api) {
            Ok(d) => d,
            Err(_) => return Discovery::NotPresent,
        };
        let lock = match PsuLock::platform() {
            Ok(l) => l,
            Err(msg) => return Discovery::Unusable(msg),
        };
        match MsiSource::identify(device, lock) {
            Ok(Some(src)) => Discovery::Found(Box::new(src)),
            Ok(None) => Discovery::Unusable(
                "The connected MSI power supply is not an MPG Ai1300TS / Ai1600TS. \
                 MeltAlarm needs a PSU with GPU Safeguard+."
                    .into(),
            ),
            // Present but not answering yet (e.g. still starting): try again later.
            Err(_) => Discovery::NotPresent,
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
    /// Reads identity; `Ok(None)` if the model string is not a supported TS model.
    fn identify(device: HidDevice, lock: PsuLock) -> Result<Option<Self>, TxError> {
        let text = |reg| transport::transact(&device, &lock, reg).map(|f| f.text());
        let model = text(Reg::MfrModel)?;
        let id = if model.starts_with("MPG Ai1300TS") {
            "msi:ai1300ts"
        } else if model.starts_with("MPG Ai1600TS") {
            "msi:ai1600ts"
        } else {
            return Ok(None);
        };
        let info = SourceInfo {
            id: SourceId(id.into()),
            vendor: text(Reg::MfrId).unwrap_or_else(|_| "MSI".into()),
            model,
            firmware: text(Reg::MfrRevision).ok().filter(|s| !s.is_empty()),
            serial: text(Reg::MfrSerial).ok().filter(|s| !s.is_empty()),
            connectors: (0..2).map(|i| ConnectorInfo { index: i, label: format!("12V-2x6 #{}", i + 1) }).collect(),
            caps: Capabilities { device_verdict: true, device_limits: true, cutoff_timer: true, wire_flags: true },
        };
        Ok(Some(MsiSource { device, lock, info, last_config: None }))
    }

    fn read(&self, reg: Reg) -> Result<protocol::Frame, TxError> {
        transport::transact(&self.device, &self.lock, reg)
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
