//! Program lifecycle, OS-free (docs/ARCHITECTURE.md §7.1, docs/FUNCTIONAL_SPEC.md §4).
//!
//! *What* a launch does and *how* a list of install steps is applied or rolled back is decided
//! here and tested on every OS. The steps themselves, autostart and instance control are
//! implemented by each frontend.
#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `major.minor.patch`; a pre-release suffix (`-dev`) is ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let core = s.trim().split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u16>());
        let major = parts.next()?.ok()?;
        let minor = parts.next().unwrap_or(Ok(0)).ok()?;
        let patch = parts.next().unwrap_or(Ok(0)).ok()?;
        Some(Version { major, minor, patch })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Spec L1: who installs, updates and uninstalls the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The executable does it itself (Windows).
    SelfManaged,
    /// A package manager does it; the app never offers it (Linux).
    PackageManaged,
}

/// Command-line intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    None,
    /// Run here, never install, never touch autostart (development, CI, deliberate portable use).
    Portable,
    /// "Install…" chosen in a running portable copy.
    Install,
    /// Uninstall chosen in the OS's installed-apps list.
    Uninstall,
}

impl Flag {
    pub fn from_args<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> Flag {
        let mut flag = Flag::None;
        for a in args {
            match a.as_ref() {
                "--uninstall" => return Flag::Uninstall,
                "--install" => flag = Flag::Install,
                "--portable" if flag == Flag::None => flag = Flag::Portable,
                _ => {}
            }
        }
        flag
    }
}

/// A MeltAlarm already running in this session, relative to the launched file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instance {
    None,
    SameFile,
    OtherFile,
}

/// What the frontend found out about this launch.
#[derive(Clone, Debug)]
pub struct Facts {
    pub policy: Policy,
    pub flag: Flag,
    pub this_version: Version,
    /// This file *is* the installed copy.
    pub this_is_installed: bool,
    /// The installed copy's version, if one is installed (unreadable → `Version::default()`).
    pub installed: Option<Version>,
    pub running: Instance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    /// Start monitoring. `portable`: no autostart, "Install…" offered (self-managed only).
    Monitor { portable: bool },
    /// Spec L6: bring the running instance forward, then exit.
    HandOff,
    OfferInstall,
    OfferUpdate { from: Version, to: Version },
    /// A downgrade: how a bad update is rolled back.
    OfferReplace { from: Version, to: Version },
    /// Same version as installed: hand off to the installed copy (start it if not running).
    StartInstalled,
    Uninstall,
    /// `--uninstall` with nothing installed.
    NotInstalled,
}

/// Spec §4.4.
pub fn decide(f: &Facts) -> Launch {
    let busy = f.running != Instance::None;
    if f.policy == Policy::PackageManaged {
        return match (busy, f.flag) {
            (true, _) => Launch::HandOff,
            (false, Flag::Portable) => Launch::Monitor { portable: true },
            (false, _) => Launch::Monitor { portable: false },
        };
    }
    match f.flag {
        Flag::Uninstall => {
            return if f.installed.is_some() || f.this_is_installed { Launch::Uninstall } else { Launch::NotInstalled };
        }
        Flag::Portable => return f.without_installing(),
        Flag::None if f.running == Instance::SameFile => return Launch::HandOff,
        Flag::None | Flag::Install => {}
    }
    if f.this_is_installed {
        return if busy { Launch::HandOff } else { Launch::Monitor { portable: false } };
    }
    match f.installed {
        None => Launch::OfferInstall,
        Some(v) => match f.this_version.cmp(&v) {
            Ordering::Greater => Launch::OfferUpdate { from: v, to: f.this_version },
            Ordering::Less => Launch::OfferReplace { from: v, to: f.this_version },
            Ordering::Equal => Launch::StartInstalled,
        },
    }
}

impl Facts {
    /// "Run without installing": monitor here, unless a monitor already runs (Spec L6).
    pub fn without_installing(&self) -> Launch {
        if self.running == Instance::None { Launch::Monitor { portable: true } } else { Launch::HandOff }
    }
}

/// One undoable change to the machine.
pub trait Step {
    /// Shown to the user if it fails, e.g. "copy the program to C:\Program Files\MeltAlarm".
    fn name(&self) -> String;
    fn apply(&mut self) -> Result<(), String>;
    /// Revert a successful `apply`. Best effort; never called for a failed step.
    fn undo(&mut self) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub step: String,
    pub error: String,
}

/// Install and update: all or nothing. On failure, the steps already applied are undone
/// in reverse order.
pub fn run_atomic(steps: &mut [Box<dyn Step>]) -> Result<(), Failed> {
    for i in 0..steps.len() {
        if let Err(error) = steps[i].apply() {
            let step = steps[i].name();
            for done in steps[..i].iter_mut().rev() {
                done.undo();
            }
            return Err(Failed { step, error });
        }
    }
    Ok(())
}

/// Uninstall: keep going, report what could not be removed.
pub fn run_best_effort(steps: &mut [Box<dyn Step>]) -> Vec<Failed> {
    steps.iter_mut().filter_map(|s| s.apply().err().map(|error| Failed { step: s.name(), error })).collect()
}

/// "Run at startup". The target is always the installed program (Spec L2).
pub trait Autostart {
    /// The program the autostart entry starts, if there is one.
    fn target(&self) -> Option<PathBuf>;
    /// `None` removes the entry.
    fn set(&self, target: Option<&Path>) -> Result<(), String>;
}

/// Finding the MeltAlarm already running in this session.
pub trait Instances {
    fn running(&self) -> Option<Box<dyn Running>>;
}

pub trait Running {
    /// Its program file, if it can be determined.
    fn path(&self) -> Option<PathBuf>;
    /// Bring it forward (Spec L6).
    fn show(&self);
    /// `None`: it can't say (an older version).
    fn alarm_active(&self) -> Option<bool>;
    /// Ask it to exit; terminate it after `grace`.
    fn stop(&self, grace: Duration) -> Result<(), String>;
}

#[cfg(test)]
mod tests;
