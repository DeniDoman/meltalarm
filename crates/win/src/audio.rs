//! Alarm sound + voice player: executes the core's `AudioScript` on its own thread, which
//! exists only while the alarm sounds (SAPI memory is released afterwards).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use meltalarm_core::{AudioScript, AudioStep};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_FILENAME, SND_NODEFAULT, SND_SYNC};
use windows::Win32::Media::Speech::{ISpVoice, SPF_ASYNC, SPF_PURGEBEFORESPEAK, SPRS_DONE, SPVOICESTATUS, SpVoice};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::core::{HSTRING, PCWSTR};

struct Playing {
    script: AudioScript,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
pub struct Audio {
    playing: Option<Playing>,
}

fn sound_file() -> Option<HSTRING> {
    let windir = std::env::var_os("WINDIR")?;
    let path = std::path::Path::new(&windir).join("Media").join("Windows Critical Stop.wav");
    path.exists().then(|| HSTRING::from(path.as_os_str()))
}

impl Audio {
    pub fn sync(&mut self, script: Option<&AudioScript>) {
        match (script, &self.playing) {
            (Some(s), Some(p)) if &p.script == s => {}
            (Some(s), _) => {
                self.stop();
                self.start(s.clone());
            }
            (None, Some(_)) => self.stop(),
            (None, None) => {}
        }
    }

    fn start(&mut self, script: AudioScript) {
        if cfg!(feature = "simulate") && std::env::var_os("MELTALARM_MUTE").is_some() {
            return; // dev: silent simulated alarms
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let s = script.clone();
        let thread = std::thread::Builder::new()
            .name("meltalarm-audio".into())
            .spawn(move || play(&s, &flag))
            .expect("spawn audio thread");
        self.playing = Some(Playing { script, stop, thread });
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.playing.take() {
            p.stop.store(true, Ordering::SeqCst);
            // SAFETY: a NULL sound stops any sound PlaySound is playing in this process.
            unsafe { let _ = PlaySoundW(PCWSTR::null(), None, SND_NODEFAULT); }
            let _ = p.thread.join();
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop();
    }
}

fn sleep_unless(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while !stop.load(Ordering::SeqCst) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn play(script: &AudioScript, stop: &AtomicBool) {
    // SAFETY: COM initialized for this thread only, uninitialized at the end; the voice is
    // released before that.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        {
            let file = sound_file();
            let mut voice: Option<ISpVoice> = None;
            'outer: loop {
                for step in &script.steps {
                    if stop.load(Ordering::SeqCst) {
                        break 'outer;
                    }
                    match step {
                        AudioStep::Sound => match &file {
                            Some(f) => {
                                let _ = PlaySoundW(f, None, SND_FILENAME | SND_SYNC | SND_NODEFAULT);
                            }
                            None => {
                                let _ = PlaySoundW(windows::core::w!("SystemHand"), None, SND_ALIAS | SND_SYNC);
                            }
                        },
                        AudioStep::Pause(d) => sleep_unless(stop, *d),
                        AudioStep::Speak(text) => {
                            if voice.is_none() {
                                voice = CoCreateInstance(&SpVoice, None, CLSCTX_ALL).ok();
                            }
                            if let Some(v) = &voice {
                                let t = HSTRING::from(text.as_str());
                                if v.Speak(&t, (SPF_ASYNC.0 | SPF_PURGEBEFORESPEAK.0) as u32, None).is_ok() {
                                    loop {
                                        if stop.load(Ordering::SeqCst) {
                                            let _ = v.Speak(PCWSTR::null(), SPF_PURGEBEFORESPEAK.0 as u32, None);
                                            break 'outer;
                                        }
                                        let mut st = SPVOICESTATUS::default();
                                        if v.GetStatus(&mut st, std::ptr::null_mut()).is_err() || st.dwRunningState == SPRS_DONE.0 as u32 {
                                            break;
                                        }
                                        std::thread::sleep(Duration::from_millis(50));
                                    }
                                }
                            }
                        }
                    }
                }
                if !script.repeat {
                    break;
                }
            }
            drop(voice);
        }
        CoUninitialize();
    }
}
