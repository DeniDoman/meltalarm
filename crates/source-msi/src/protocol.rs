//! MSI MPG Ai1x00TS USB HID protocol: the closed set of read requests and reply decoding.
//!
//! Offsets follow MSI's own layout: `[0]` = report id, `[1]` = opcode, `[2]` = register,
//! `[3..]` = data. Verified on a real Ai1300TS (docs/FUNCTIONAL_SPEC.md §1).

use std::time::Duration;

use meltalarm_model::{DeviceStatus, Fault, Protection, WIRES};

pub const FRAME: usize = 65;
pub const VID: u16 = 0x0DB0;
pub const PID_AI1300TS: u16 = 0xAA6F;
pub const PID_AI1600TS: u16 = 0x808C;
/// Non-configurable firmware limit behind status 3 (`OCP_18A`).
pub const HARD_WIRE_LIMIT_A: f32 = 18.0;

const OP_READ: u8 = 0x51;

/// Every register this program can ever ask for. All are reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Reg {
    MfrId = 0x10,
    MfrModel = 0x11,
    MfrRevision = 0x12,
    MfrSerial = 0x13,
    SafeguardConfig = 0xC0,
    SafeguardStatus = 0xC1,
    Telemetry = 0xE0,
    ProtectionFlags = 0xE1,
}

impl Reg {
    pub const ALL: [Reg; 8] = [
        Reg::MfrId,
        Reg::MfrModel,
        Reg::MfrRevision,
        Reg::MfrSerial,
        Reg::SafeguardConfig,
        Reg::SafeguardStatus,
        Reg::Telemetry,
        Reg::ProtectionFlags,
    ];

    /// Reply bytes (from the opcode on) needed to decode this register without
    /// reading past the data — a short reply must never decode as zeros.
    fn min_reply(self) -> usize {
        match self {
            Reg::SafeguardConfig => 10,
            Reg::SafeguardStatus => 44,
            Reg::Telemetry => 32,
            Reg::ProtectionFlags => 20,
            _ => 3,
        }
    }
}

/// The only packet builder in this crate: `00 51 <reg> 00…`.
pub(crate) fn read_request(reg: Reg) -> [u8; FRAME] {
    let mut p = [0u8; FRAME];
    p[1] = OP_READ;
    p[2] = reg as u8;
    p
}

/// A validated reply in MSI layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame(pub(crate) [u8; FRAME]);

pub(crate) enum Reply {
    Ours(Frame),
    /// `FE` followed by zeros: the PSU could not answer.
    Busy,
    /// Echo matched but the reply is too short to decode.
    Short,
    /// Another request's reply (e.g. MSI Center's or HWiNFO's): ignore.
    Foreign,
}

/// Classify one input report read from the device for a pending request on `reg`.
pub(crate) fn classify(reg: Reg, raw: &[u8]) -> Reply {
    // hidapi strips the zero report id on Windows; tolerate it being present too.
    let off = usize::from(raw.len() > 1 && raw[0] == 0 && raw[1] == OP_READ);
    let data = &raw[off..];
    if data.len() < 2 || data[0] != OP_READ || data[1] != reg as u8 {
        return Reply::Foreign;
    }
    if data.len() < reg.min_reply() {
        return Reply::Short;
    }
    let mut f = [0u8; FRAME];
    let n = data.len().min(FRAME - 1);
    f[1..1 + n].copy_from_slice(&data[..n]);
    // A data word may legitimately start with FE; only FE + all zeros means "busy".
    if f[3] == 0xFE && f[4..].iter().all(|&b| b == 0) {
        return Reply::Busy;
    }
    Reply::Ours(Frame(f))
}

/// PMBus LINEAR11 as MSI decodes it: signed 5-bit exponent, *unsigned* 11-bit mantissa.
pub fn linear11(lo: u8, hi: u8) -> f32 {
    let w = u16::from_le_bytes([lo, hi]);
    let exp = (w as i16 >> 11) as i32;
    (w & 0x07FF) as f32 * 2f32.powi(exp)
}

impl Frame {
    fn l11(&self, at: usize) -> f32 {
        linear11(self.0[at], self.0[at + 1])
    }

    /// The reply from the opcode on, as uppercase hex without trailing zero bytes.
    pub fn hex(&self) -> String {
        let end = self.0.iter().rposition(|&b| b != 0).map_or(3, |i| i + 1).max(3);
        self.0[1..end].iter().map(|b| format!("{b:02X}")).collect()
    }

    /// `0xE0`: wire currents of both connectors, in amps.
    pub fn wire_currents(&self) -> [[f32; WIRES]; 2] {
        let word = |k: usize| self.l11(3 + 2 * k);
        [std::array::from_fn(|i| word(3 + i)), std::array::from_fn(|i| word(9 + i))]
    }

    /// `0xC1`: raw status code of each connector.
    pub fn status_codes(&self) -> [u8; 2] {
        [self.0[3], self.0[24]]
    }

    /// `0xE1`: protection flags.
    pub fn flags(&self) -> Flags {
        let b = |i: usize| self.0[i] != 0;
        Flags {
            rails: [b(3), b(4), b(5)],
            wires: [std::array::from_fn(|i| b(6 + i)), std::array::from_fn(|i| b(12 + i))],
            over_power: b(18),
            over_temperature: b(19),
            fan: b(20),
        }
    }

    /// `0xC0`: Safeguard+ configuration.
    pub fn protection(&self) -> Protection {
        let secs = |i: usize| Some(Duration::from_secs(u64::from(self.0[i])));
        Protection {
            enabled: self.0[3] == 1,
            wire_limit: Some(self.l11(4)),
            spread_limit: Some(self.l11(6)),
            wire_trigger: secs(8),
            spread_trigger: secs(9),
            cutoff_after: secs(10),
            hard_wire_limit: Some(HARD_WIRE_LIMIT_A),
        }
    }

    /// `0x10`–`0x13`: ASCII text, optionally length-prefixed (the serial is).
    pub fn text(&self) -> String {
        let data = &self.0[3..];
        let bytes = match data[0] {
            n @ 1..0x20 => &data[1..(1 + n as usize).min(data.len())],
            _ => data,
        };
        let s: String = bytes.iter().take_while(|&&b| b != 0).filter(|b| b.is_ascii_graphic() || **b == b' ').map(|&b| b as char).collect();
        s.trim().to_owned()
    }
}

pub fn device_status(code: u8) -> DeviceStatus {
    match code {
        0 => DeviceStatus::Normal,
        1 => DeviceStatus::OverCurrent,
        2 => DeviceStatus::Imbalance,
        3 => DeviceStatus::CriticalOverCurrent,
        n => DeviceStatus::Unknown(n),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flags {
    pub rails: [bool; 3],
    pub wires: [[bool; WIRES]; 2],
    pub over_power: bool,
    pub over_temperature: bool,
    pub fan: bool,
}

impl Flags {
    pub fn faults(&self) -> Vec<Fault> {
        let mut v = Vec::new();
        for (on, rail) in self.rails.iter().zip(["12V", "5V", "3.3V"]) {
            if *on {
                v.push(Fault::RailOverCurrent(rail));
            }
        }
        if self.over_power {
            v.push(Fault::OverPower);
        }
        if self.over_temperature {
            v.push(Fault::OverTemperature);
        }
        if self.fan {
            v.push(Fault::FanFailure);
        }
        v
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Replies captured from the reference Ai1300TS (docs/FUNCTIONAL_SPEC.md §1), as hidapi
    /// returns them (no report id), trailing zeros omitted.
    pub const E0_IDLE: &str = "51E016F034E014E002E002E002E002E002E002E000E000E000E000E000E000E02B086CEA1E00001006D385CAAAC9";
    pub const C0_REF: &str = "51C001C0E058E01414B4";
    pub const C1_NORMAL: &str = "51C100000000000000000000E000E000E000E000E000E000000000000000000000E000E000E000E000E000E0";
    pub const E1_CLEAR: &str = "51E1";
    pub const MODEL: &str = "51114D50472041693133303054530000";
    pub const SERIAL_SYNTH: &str = "51130E32303236303130312D3030303030";

    pub fn raw(hex: &str) -> Vec<u8> {
        let mut v: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        v.resize(FRAME - 1, 0);
        v
    }

    fn ours(reg: Reg, hex: &str) -> Frame {
        match classify(reg, &raw(hex)) {
            Reply::Ours(f) => f,
            _ => panic!("not accepted"),
        }
    }

    #[test]
    fn only_allowlisted_read_packets_exist() {
        for reg in Reg::ALL {
            let p = read_request(reg);
            assert_eq!(&p[..2], &[0x00, 0x51]);
            assert!([0x10, 0x11, 0x12, 0x13, 0xC0, 0xC1, 0xE0, 0xE1].contains(&p[2]));
            assert!(p[3..].iter().all(|&b| b == 0));
        }
    }

    #[test]
    fn linear11_matches_msi_examples() {
        assert_eq!(linear11(0xC0, 0xE0), 12.0);
        assert_eq!(linear11(0x58, 0xE0), 5.5);
        assert_eq!(linear11(0x0A, 0x08), 20.0);
        assert_eq!(linear11(0x05, 0xF0), 1.25);
    }

    #[test]
    fn decodes_reference_frames() {
        let e0 = ours(Reg::Telemetry, E0_IDLE);
        assert_eq!(e0.wire_currents(), [[0.125; 6], [0.0; 6]]);

        let p = ours(Reg::SafeguardConfig, C0_REF).protection();
        assert!(p.enabled);
        assert_eq!((p.wire_limit, p.spread_limit), (Some(12.0), Some(5.5)));
        assert_eq!(p.wire_trigger, Some(Duration::from_secs(20)));
        assert_eq!(p.cutoff_after, Some(Duration::from_secs(180)));

        assert_eq!(ours(Reg::SafeguardStatus, C1_NORMAL).status_codes(), [0, 0]);
        assert!(ours(Reg::ProtectionFlags, E1_CLEAR).flags().faults().is_empty());
        assert_eq!(ours(Reg::MfrModel, MODEL).text(), "MPG Ai1300TS");
        assert_eq!(ours(Reg::MfrSerial, SERIAL_SYNTH).text(), "20260101-00000");
    }

    #[test]
    fn classifies_foreign_busy_short_and_report_id() {
        assert!(matches!(classify(Reg::Telemetry, &raw(C1_NORMAL)), Reply::Foreign));
        assert!(matches!(classify(Reg::Telemetry, &raw("51E0FE")), Reply::Busy));
        // FE as the low byte of real data is not "busy".
        assert!(matches!(classify(Reg::Telemetry, &raw("51E0FE01")), Reply::Ours(_)));
        assert!(matches!(classify(Reg::Telemetry, &[0x51, 0xE0, 0x16]), Reply::Short));
        let mut with_id = vec![0u8];
        with_id.extend(raw(E0_IDLE));
        with_id.truncate(FRAME);
        assert!(matches!(classify(Reg::Telemetry, &with_id), Reply::Ours(_)));
    }

    #[test]
    fn status_codes_map_to_model() {
        assert_eq!(device_status(0), DeviceStatus::Normal);
        assert_eq!(device_status(2), DeviceStatus::Imbalance);
        assert_eq!(device_status(3), DeviceStatus::CriticalOverCurrent);
        assert_eq!(device_status(9), DeviceStatus::Unknown(9));
    }

    #[test]
    fn hex_trims_trailing_zeros() {
        assert_eq!(ours(Reg::SafeguardConfig, C0_REF).hex(), C0_REF);
    }
}
