//! Alarm sound + voice player: executes the core's `AudioScript` on its own thread, which
//! exists only while the alarm sounds (SAPI memory is released afterwards). Also the caution
//! chime (DESIGN.md "Caution chime"), synthesized once in memory.

use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use meltalarm_core::{AudioScript, AudioStep};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_FILENAME, SND_MEMORY, SND_NODEFAULT, SND_SYNC};
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

    /// The caution chime, once. Never over the alarm sound (the alarm wins).
    pub fn chime(&self) {
        if self.playing.is_some() || (cfg!(feature = "simulate") && std::env::var_os("MELTALARM_MUTE").is_some()) {
            return;
        }
        static WAV: OnceLock<Vec<u8>> = OnceLock::new();
        let wav = WAV.get_or_init(chime_wav);
        // SAFETY: with SND_MEMORY the "name" is a pointer to a complete WAV image; it is static,
        // so it outlives the asynchronous playback.
        unsafe {
            let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
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

/// One soft bell-like note: C6 with a quiet ×2.76 partial, 5 ms attack, exponential decay
/// (τ 160 ms), 0.7 s, peak about −12 dBFS. 16-bit mono PCM at 22.05 kHz.
fn chime_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let n = (RATE as f32 * 0.7) as usize;
    let (f, peak) = (1046.5_f32, 10f32.powf(-12.0 / 20.0));
    let samples: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let env = (t / 0.005).min(1.0) * (-t / 0.16).exp();
            let tone = (std::f32::consts::TAU * f * t).sin() + 0.25 * (std::f32::consts::TAU * f * 2.76 * t).sin();
            (tone / 1.25 * env * peak * i16::MAX as f32) as i16
        })
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
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
