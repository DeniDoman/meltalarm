//! The contract every source implements, and the one `HidApi` they share.
//!
//! # Source authoring rules (docs/ARCHITECTURE.md §4.2) — mandatory
//! 1. A closed request enum; exactly one private function builds request bytes.
//! 2. Exactly one device-write call site, marked `#[allow(clippy::disallowed_methods)]`.
//! 3. No byte sent to a device derives from settings, UI, CLI or IPC.
//! 4. Every transaction is bounded in time.
//! 5. Honor the vendor's coexistence locks.
//! 6. Report anything unavailable as `None`, never as zero.
//! 7. Ship golden-frame tests.
//! 8. `discover` performs the vendor's complete connect sequence: `Found` means the device has
//!    answered; present but silent is `NotReady`.

pub use hidapi;
use meltalarm_model::{Report, SourceInfo};

/// Owns the process's single `HidApi` (hidapi allows one per process).
/// Lives on the acquisition thread and is lent to drivers and sources.
#[derive(Default)]
pub struct HidContext {
    api: Option<hidapi::HidApi>,
}

impl HidContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// The HID API with a freshly enumerated device list.
    pub fn refreshed(&mut self) -> Result<&hidapi::HidApi, String> {
        match &mut self.api {
            Some(api) => api.refresh_devices().map_err(|e| e.to_string())?,
            None => self.api = Some(hidapi::HidApi::new().map_err(|e| e.to_string())?),
        }
        Ok(self.api.as_ref().expect("initialized above"))
    }
}

pub enum Discovery {
    Found(Box<dyn Source>),
    NotPresent,
    /// A device of this family is present but not answering yet; the reason is kept for the
    /// "not found" message if it never becomes ready.
    NotReady(String),
    /// A device of this family is present but cannot be used: unsupported model,
    /// missing privileges, … The message is shown to the user.
    Unusable(String),
}

pub trait Driver: Send + Sync {
    /// Human name of the supported device family, used in "not found" messages.
    fn name(&self) -> &'static str;
    fn discover(&self, hid: &mut HidContext) -> Discovery;
}

pub trait Source: Send {
    fn info(&self) -> &SourceInfo;
    /// One tick. Blocking but bounded (≤ 3 s worst case). Never panics on device errors:
    /// failures are expressed as `None` fields in the report.
    fn poll(&mut self, hid: &mut HidContext) -> Report;
}
