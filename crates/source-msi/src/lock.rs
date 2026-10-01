//! Coexistence with other PSU readers (MSI Center, Afterburner's PSU plugin, HWiNFO64):
//! every transaction runs under `Global\MSI_PSU_Mutex`, like theirs do.
//!
//! Verified behavior (docs/FUNCTIONAL_SPEC.md F3, F17): the mutex is usually created by a
//! SYSTEM service, so even an elevated admin gets only SYNCHRONIZE (enough to wait and
//! release). `CreateMutexW` would ask for full access and fail, so open first and create
//! only if absent. Other clients may hold it for ~250 ms.

use std::time::Duration;

use crate::transport::TxError;

pub(crate) enum PsuLock {
    #[cfg(windows)]
    Mutex(win::PsuMutex),
    /// No coexistence lock: non-Windows platforms (no MSI software) and tests.
    #[allow(dead_code)]
    None,
}

pub(crate) struct Guard<'a> {
    #[cfg(windows)]
    held: Option<&'a win::PsuMutex>,
    #[cfg(not(windows))]
    _p: std::marker::PhantomData<&'a ()>,
}

impl PsuLock {
    /// The platform's lock. Error text is meant for the user.
    pub(crate) fn platform() -> Result<PsuLock, String> {
        #[cfg(windows)]
        return win::PsuMutex::open().map(PsuLock::Mutex);
        #[cfg(not(windows))]
        return Ok(PsuLock::None);
    }

    pub(crate) fn acquire(&self, timeout: Duration) -> Result<Guard<'_>, TxError> {
        match self {
            #[cfg(windows)]
            PsuLock::Mutex(m) => m.wait(timeout).map(|()| Guard { held: Some(m) }),
            PsuLock::None => {
                let _ = timeout;
                Ok(Guard {
                    #[cfg(windows)]
                    held: None,
                    #[cfg(not(windows))]
                    _p: std::marker::PhantomData,
                })
            }
        }
    }
}

#[cfg(windows)]
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.held {
            m.release();
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, OpenMutexW, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject};

    pub(crate) struct PsuMutex(HANDLE);

    // SAFETY: a mutex handle is a process-wide kernel object reference usable from any
    // thread. Win32 mutex *ownership* is per thread; `transact` always acquires and
    // releases on the same thread within one call.
    unsafe impl Send for PsuMutex {}

    impl PsuMutex {
        pub(crate) fn open() -> Result<Self, String> {
            let name: Vec<u16> = "Global\\MSI_PSU_Mutex\0".encode_utf16().collect();
            // SAFETY: `name` is a valid NUL-terminated UTF-16 string that outlives the calls.
            unsafe {
                let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr());
                if !h.is_null() {
                    return Ok(PsuMutex(h));
                }
                if GetLastError() == ERROR_ACCESS_DENIED {
                    return Err(ACCESS_DENIED.into());
                }
                let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
                if h.is_null() {
                    let err = GetLastError();
                    return Err(if err == ERROR_ACCESS_DENIED {
                        ACCESS_DENIED.into()
                    } else {
                        format!("Cannot open the PSU coexistence lock (Windows error {err}).")
                    });
                }
                Ok(PsuMutex(h))
            }
        }

        pub(crate) fn wait(&self, timeout: Duration) -> Result<(), TxError> {
            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: the handle is valid for the lifetime of `self`.
            match unsafe { WaitForSingleObject(self.0, ms) } {
                // An abandoned mutex (previous owner died) is still acquired by us.
                WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(()),
                WAIT_TIMEOUT => Err(TxError::LockTimeout),
                other => Err(TxError::Io(format!("mutex wait failed ({other})"))),
            }
        }

        pub(crate) fn release(&self) {
            // SAFETY: called only by the guard created by a successful `wait` on this thread.
            unsafe { ReleaseMutex(self.0) };
        }
    }

    impl Drop for PsuMutex {
        fn drop(&mut self) {
            // SAFETY: we own this handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    const ACCESS_DENIED: &str = "MeltAlarm must run as administrator to share the PSU safely with MSI Center, \
        Afterburner or HWiNFO (their lock is only accessible to administrators).";
}
