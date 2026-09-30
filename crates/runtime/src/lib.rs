//! Portable host for MeltAlarm (docs/ARCHITECTURE.md §6).
//!
//! The frontend owns a [`Runtime`] on its UI thread and calls it; the runtime never calls
//! UI code except the injected `waker`. A background acquisition thread discovers a
//! source and polls it once per second.
#![forbid(unsafe_code)]

mod acquisition;
mod store;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use meltalarm_core::{Core, Event, LogEvent, Output, UserAction, ViewModel};
use meltalarm_source_api::Driver;

pub use meltalarm_core as core;
pub use store::{LogSink, SettingsStore, StateStore};

pub struct Paths {
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
}

pub struct Host {
    pub drivers: Vec<Box<dyn Driver>>,
    pub paths: Paths,
    /// Local wall-clock time for log lines, e.g. `2026-09-27 18:02:11`.
    pub wall_clock: fn() -> String,
    /// Called from the acquisition thread whenever `pump` has work.
    pub waker: Box<dyn Fn() + Send>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lifecycle {
    /// Startup found no usable source: show the message and exit.
    DiscoveryFailed(String),
    /// Monitoring stopped unexpectedly: show the message.
    Fatal(String),
}

#[derive(Debug, Default)]
pub struct Update {
    pub next_wake: Option<Instant>,
    pub lifecycle: Option<Lifecycle>,
}

pub(crate) enum SourceEvent {
    Core(Event),
    DiscoveryFailed(String),
    Fatal(String),
}

pub(crate) enum Command {
    Pause,
    Resume,
    Shutdown,
}

pub struct Runtime {
    core: Core,
    log: LogSink,
    store: SettingsStore,
    state: StateStore,
    wall_clock: fn() -> String,
    rx: Receiver<SourceEvent>,
    cmd: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Runtime {
    pub fn start(host: Host) -> Runtime {
        let store = SettingsStore::new(host.paths.config_dir.join("settings.toml"));
        let log = LogSink::new(host.paths.log_dir.join("alarms.log"));
        let state = StateStore::new(host.paths.config_dir.join("state.toml"));
        let mut core = Core::new(store.load());
        core.set_wall_clock(host.wall_clock);
        core.restore(state.load());
        let (tx, rx) = mpsc::channel();
        let (cmd, cmd_rx) = mpsc::channel();
        let thread = acquisition::spawn(host.drivers, tx, cmd_rx, host.waker);
        Runtime { core, log, store, state, wall_clock: host.wall_clock, rx, cmd, thread: Some(thread) }
    }

    /// Drain pending source events into the core. Call after the waker fired.
    pub fn pump(&mut self, now: Instant) -> Update {
        let mut update = Update::default();
        let mut out = Output::default();
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                SourceEvent::Core(e) => merge(&mut out, self.core.handle(e, now)),
                SourceEvent::DiscoveryFailed(msg) => {
                    out.log.push(LogEvent::MonitoringStopped { reason: format!("not started: {}", msg.replace('\n', " ")) });
                    update.lifecycle = Some(Lifecycle::DiscoveryFailed(msg));
                }
                SourceEvent::Fatal(msg) => {
                    out.log.push(LogEvent::MonitoringStopped { reason: msg.clone() });
                    update.lifecycle = Some(Lifecycle::Fatal(msg));
                }
            }
        }
        merge(&mut out, self.core.handle(Event::Wake, now));
        update.next_wake = self.apply(out);
        update
    }

    pub fn user(&mut self, action: UserAction, now: Instant) -> Update {
        let out = self.core.handle(Event::User(action), now);
        Update { next_wake: self.apply(out), lifecycle: None }
    }

    /// Call at (or after) `Update::next_wake`.
    pub fn wake(&mut self, now: Instant) -> Update {
        let out = self.core.handle(Event::Wake, now);
        Update { next_wake: self.apply(out), lifecycle: None }
    }

    pub fn suspend(&mut self, now: Instant) {
        let _ = self.cmd.send(Command::Pause);
        let out = self.core.handle(Event::Suspended, now);
        self.apply(out);
    }

    pub fn resume(&mut self, now: Instant) -> Update {
        let _ = self.cmd.send(Command::Resume);
        let out = self.core.handle(Event::Resumed, now);
        Update { next_wake: self.apply(out), lifecycle: None }
    }

    pub fn view(&self, now: Instant) -> Arc<ViewModel> {
        Arc::new(self.core.view(now))
    }

    pub fn log_path(&self) -> &std::path::Path {
        self.log.path()
    }

    /// Write a line directly (e.g. from a panic hook or frontend-detected failure).
    pub fn log_event(&mut self, e: LogEvent) {
        self.log.write(&e.format(&(self.wall_clock)()));
    }

    fn apply(&mut self, out: Output) -> Option<Instant> {
        if !out.log.is_empty() {
            let wall = (self.wall_clock)();
            for e in &out.log {
                self.log.write(&e.format(&wall));
            }
        }
        if let Some(s) = &out.settings_changed {
            self.store.save(s);
        }
        if let Some(s) = &out.state_changed {
            self.state.save(s);
        }
        out.next_wake
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.cmd.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn merge(acc: &mut Output, o: Output) {
    acc.log.extend(o.log);
    if o.settings_changed.is_some() {
        acc.settings_changed = o.settings_changed;
    }
    if o.state_changed.is_some() {
        acc.state_changed = o.state_changed;
    }
    acc.next_wake = o.next_wake;
}
