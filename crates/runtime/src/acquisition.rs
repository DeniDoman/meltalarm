//! The acquisition thread: discovery (≤ 15 s at startup), 1 Hz polling, generic reconnect.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use meltalarm_core::Event;
use meltalarm_source_api::{Discovery, Driver, HidContext, Source};

use crate::{Command, SourceEvent};

const STARTUP_DISCOVERY: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_secs(1);
const REDISCOVER_EVERY: Duration = Duration::from_secs(2);
const DROP_AFTER_UNHEALTHY: u32 = 3;

struct Ctx {
    drivers: Vec<Box<dyn Driver>>,
    tx: Sender<SourceEvent>,
    cmd: Receiver<Command>,
    waker: Box<dyn Fn() + Send>,
    hid: HidContext,
    paused: bool,
}

/// Why a wait ended early.
enum Stop {
    Shutdown,
}

impl Ctx {
    fn send(&self, e: SourceEvent) {
        let _ = self.tx.send(e);
        (self.waker)();
    }

    /// Sleep until `deadline`, handling commands. Returns Err on shutdown.
    fn wait_until(&mut self, deadline: Instant) -> Result<(), Stop> {
        loop {
            // While paused (system suspend) idle in 1 s steps until resumed.
            let left = if self.paused { TICK } else { deadline.saturating_duration_since(Instant::now()) };
            match self.cmd.recv_timeout(left) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return Err(Stop::Shutdown),
                Ok(Command::Pause) => self.paused = true,
                Ok(Command::Resume) => self.paused = false,
                Err(RecvTimeoutError::Timeout) => {}
            }
            if !self.paused && Instant::now() >= deadline {
                return Ok(());
            }
        }
    }

    fn discover(&mut self) -> Result<Box<dyn Source>, Option<String>> {
        let mut unusable = None;
        for d in &self.drivers {
            match d.discover(&mut self.hid) {
                Discovery::Found(s) => return Ok(s),
                Discovery::Unusable(msg) => unusable = Some(msg),
                Discovery::NotPresent => {}
            }
        }
        Err(unusable)
    }

    fn not_found_message(&self) -> String {
        let names: Vec<&str> = self.drivers.iter().map(|d| d.name()).collect();
        format!(
            "MeltAlarm supports only {} power supplies connected by USB. No supported PSU was found.",
            names.join(", ")
        )
    }

    fn run(&mut self) -> Result<(), Stop> {
        // Startup discovery: give USB enumeration time after logon.
        let give_up = Instant::now() + STARTUP_DISCOVERY;
        let mut source = loop {
            match self.discover() {
                Ok(s) => break s,
                Err(Some(msg)) => {
                    self.send(SourceEvent::DiscoveryFailed(msg));
                    return Ok(());
                }
                Err(None) if Instant::now() >= give_up => {
                    let msg = self.not_found_message();
                    self.send(SourceEvent::DiscoveryFailed(msg));
                    return Ok(());
                }
                Err(None) => self.wait_until(Instant::now() + TICK)?,
            }
        };
        self.send(SourceEvent::Core(Event::SourceConnected(source.info().clone())));

        let mut next = Instant::now();
        let mut unhealthy = 0;
        loop {
            self.wait_until(next)?;
            next = (next + TICK).max(Instant::now());
            let report = source.poll(&mut self.hid);
            let needs_verdict = source.info().caps.device_verdict;
            let healthy = report.readings.is_some() && (!needs_verdict || report.verdicts.is_some());
            unhealthy = if healthy { 0 } else { unhealthy + 1 };
            self.send(SourceEvent::Core(Event::Report(report)));

            if unhealthy >= DROP_AFTER_UNHEALTHY {
                // Close the device and rediscover until it answers again (USB replug, …).
                drop(source);
                self.send(SourceEvent::Core(Event::SourceLost));
                source = loop {
                    self.wait_until(Instant::now() + REDISCOVER_EVERY)?;
                    if let Ok(s) = self.discover() {
                        break s;
                    }
                };
                self.send(SourceEvent::Core(Event::SourceConnected(source.info().clone())));
                unhealthy = 0;
                next = Instant::now();
            }
        }
    }
}

pub(crate) fn spawn(
    drivers: Vec<Box<dyn Driver>>,
    tx: Sender<SourceEvent>,
    cmd: Receiver<Command>,
    waker: Box<dyn Fn() + Send>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("meltalarm-acquisition".into())
        .spawn(move || {
            let mut ctx = Ctx { drivers, tx, cmd, waker, hid: HidContext::new(), paused: false };
            let result = catch_unwind(AssertUnwindSafe(|| ctx.run()));
            if let Err(panic) = result {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "internal error".into());
                ctx.send(SourceEvent::Fatal(format!("internal error in the monitoring thread: {msg}")));
            }
        })
        .expect("spawn acquisition thread")
}
