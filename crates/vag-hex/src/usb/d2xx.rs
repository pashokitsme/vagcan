//! Native FTDI D2XX via runtime `dlopen` of the vendored `libftd2xx.dylib`.
//!
//! We do NOT link the driver at build time. The vendored dylib
//! (`driver/darwin-arm64/build/libftd2xx.dylib`, install name
//! `@rpath/libftd2xx.dylib`) is loaded with [`libloading`] the first time a
//! cable is enumerated or opened. Consequences:
//!   * `cargo build`/`cargo test` never touch the native lib — no link-time or
//!     dyld dependency, so they are green on any host.
//!   * A missing/uninstallable driver surfaces as a clean [`HexError`] at
//!     `open`/`list_cables` time, not a build or launch failure.
//!
//! Only the handful of `FT_*` entry points the byte pipe needs are bound. All
//! FFI is confined to this module; the rest of the crate sees safe Rust.

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::Arc;

use crate::error::HexError;
use crate::usb::{CableInfo, RawDevice};

// FTDI D2XX typedefs (see the vendored `ftd2xx.h`).
type FtStatus = u32; // FT_OK == 0
type FtHandle = *mut c_void; // PVOID
type Dword = u32;

const FT_OK: FtStatus = 0;
const FT_OPEN_BY_SERIAL_NUMBER: Dword = 1;
const FT_PURGE_RX: Dword = 1;
const FT_PURGE_TX: Dword = 2;

// Serial/description buffer sizes mandated by FT_GetDeviceInfoDetail.
const SERIAL_LEN: usize = 16;
const DESC_LEN: usize = 64;

type FnCreateInfoList = unsafe extern "C" fn(*mut Dword) -> FtStatus;
type FnGetInfoDetail = unsafe extern "C" fn(
    Dword,        // index
    *mut Dword,   // flags
    *mut Dword,   // type
    *mut Dword,   // id ((vid<<16)|pid)
    *mut Dword,   // locid
    *mut c_char,  // serial
    *mut c_char,  // description
    *mut FtHandle,
) -> FtStatus;
type FnOpen = unsafe extern "C" fn(c_int, *mut FtHandle) -> FtStatus;
type FnOpenEx = unsafe extern "C" fn(*mut c_void, Dword, *mut FtHandle) -> FtStatus;
type FnClose = unsafe extern "C" fn(FtHandle) -> FtStatus;
type FnRead = unsafe extern "C" fn(FtHandle, *mut c_void, Dword, *mut Dword) -> FtStatus;
type FnWrite = unsafe extern "C" fn(FtHandle, *const c_void, Dword, *mut Dword) -> FtStatus;
type FnSetBaud = unsafe extern "C" fn(FtHandle, Dword) -> FtStatus;
type FnSetLatency = unsafe extern "C" fn(FtHandle, u8) -> FtStatus;
type FnPurge = unsafe extern "C" fn(FtHandle, Dword) -> FtStatus;
type FnSetTimeouts = unsafe extern "C" fn(FtHandle, Dword, Dword) -> FtStatus;
type FnReset = unsafe extern "C" fn(FtHandle) -> FtStatus;

/// The loaded driver: the `Library` handle plus every entry point we bind.
/// `_lib` is kept alive so the copied function pointers stay valid.
struct Ftd2xx {
    _lib: libloading::Library,
    create_info_list: FnCreateInfoList,
    get_info_detail: FnGetInfoDetail,
    open: FnOpen,
    open_ex: FnOpenEx,
    close: FnClose,
    read: FnRead,
    write: FnWrite,
    set_baud: FnSetBaud,
    set_latency: FnSetLatency,
    purge: FnPurge,
    set_timeouts: FnSetTimeouts,
    reset: FnReset,
}

// SAFETY: the bound function pointers are position-independent code in the
// loaded image; `_lib` keeps that image mapped for the struct's whole life.
unsafe impl Send for Ftd2xx {}
unsafe impl Sync for Ftd2xx {}

/// Candidate dylib locations, most specific first: the vendored driver next to
/// this source tree, then whatever the dynamic loader can find.
fn candidate_paths() -> Vec<String> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    vec![
        format!("{manifest}/../../driver/darwin-arm64/build/libftd2xx.dylib"),
        "libftd2xx.dylib".to_string(),
        "/usr/local/lib/libftd2xx.dylib".to_string(),
    ]
}

impl Ftd2xx {
    fn load() -> Result<Self, HexError> {
        let mut last_err = String::from("no candidate paths");
        for path in candidate_paths() {
            match unsafe { libloading::Library::new(&path) } {
                Ok(lib) => return Self::bind(lib),
                Err(e) => last_err = format!("{path}: {e}"),
            }
        }
        Err(HexError::D2xx(format!(
            "could not load libftd2xx.dylib ({last_err})"
        )))
    }

    fn bind(lib: libloading::Library) -> Result<Self, HexError> {
        // Copy each entry point out of the library into a plain fn pointer.
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                let s: libloading::Symbol<$ty> = unsafe { lib.get($name) }
                    .map_err(|e| HexError::D2xx(format!("missing {}: {e}", stringify!($name))))?;
                *s
            }};
        }
        Ok(Self {
            create_info_list: sym!(b"FT_CreateDeviceInfoList\0", FnCreateInfoList),
            get_info_detail: sym!(b"FT_GetDeviceInfoDetail\0", FnGetInfoDetail),
            open: sym!(b"FT_Open\0", FnOpen),
            open_ex: sym!(b"FT_OpenEx\0", FnOpenEx),
            close: sym!(b"FT_Close\0", FnClose),
            read: sym!(b"FT_Read\0", FnRead),
            write: sym!(b"FT_Write\0", FnWrite),
            set_baud: sym!(b"FT_SetBaudRate\0", FnSetBaud),
            set_latency: sym!(b"FT_SetLatencyTimer\0", FnSetLatency),
            purge: sym!(b"FT_Purge\0", FnPurge),
            set_timeouts: sym!(b"FT_SetTimeouts\0", FnSetTimeouts),
            reset: sym!(b"FT_ResetDevice\0", FnReset),
            _lib: lib,
        })
    }
}

/// Map a non-`FT_OK` status to a [`HexError`], tagging which call failed.
fn check(op: &str, status: FtStatus) -> Result<(), HexError> {
    if status == FT_OK {
        Ok(())
    } else {
        Err(HexError::D2xx(format!("{op} failed: FT_STATUS {status}")))
    }
}

/// Enumerate FTDI devices via `FT_CreateDeviceInfoList` + `FT_GetDeviceInfoDetail`.
pub(crate) fn list_cables() -> Result<Vec<CableInfo>, HexError> {
    let drv = Ftd2xx::load()?;
    let mut num: Dword = 0;
    check("FT_CreateDeviceInfoList", unsafe {
        (drv.create_info_list)(&mut num)
    })?;

    let mut out = Vec::with_capacity(num as usize);
    for index in 0..num {
        let mut flags: Dword = 0;
        let mut dtype: Dword = 0;
        let mut id: Dword = 0;
        let mut locid: Dword = 0;
        let mut serial = [0i8; SERIAL_LEN];
        let mut desc = [0i8; DESC_LEN];
        let mut handle: FtHandle = std::ptr::null_mut();
        let st = unsafe {
            (drv.get_info_detail)(
                index,
                &mut flags,
                &mut dtype,
                &mut id,
                &mut locid,
                serial.as_mut_ptr(),
                desc.as_mut_ptr(),
                &mut handle,
            )
        };
        check("FT_GetDeviceInfoDetail", st)?;
        out.push(CableInfo {
            serial: cstr_to_string(&serial),
            description: cstr_to_string(&desc),
            vid: ((id >> 16) & 0xFFFF) as u16,
            pid: (id & 0xFFFF) as u16,
        });
    }
    Ok(out)
}

fn cstr_to_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Open a cable and program the FTDI params, returning the blocking device the
/// worker thread will own.
pub(crate) fn open_device(serial: Option<&str>) -> Result<D2xxDevice, HexError> {
    use crate::usb::D2xxBackend;

    let drv = Arc::new(Ftd2xx::load()?);
    let mut handle: FtHandle = std::ptr::null_mut();

    match serial {
        Some(s) => {
            let cser = CString::new(s)
                .map_err(|_| HexError::D2xx("serial contains NUL".into()))?;
            check("FT_OpenEx", unsafe {
                (drv.open_ex)(
                    cser.as_ptr() as *mut c_void,
                    FT_OPEN_BY_SERIAL_NUMBER,
                    &mut handle,
                )
            })?;
        }
        None => check("FT_Open", unsafe { (drv.open)(0, &mut handle) })?,
    }

    // Program the params VCDS-style: reset, fixed baud, low latency, timeouts,
    // then flush both directions before any framing runs.
    check("FT_ResetDevice", unsafe { (drv.reset)(handle) })?;
    check("FT_SetBaudRate", unsafe {
        (drv.set_baud)(handle, D2xxBackend::BAUD_RATE)
    })?;
    check("FT_SetLatencyTimer", unsafe {
        (drv.set_latency)(handle, D2xxBackend::LATENCY_TIMER_MS)
    })?;
    check("FT_SetTimeouts", unsafe {
        (drv.set_timeouts)(handle, D2xxBackend::TIMEOUT_MS, D2xxBackend::TIMEOUT_MS)
    })?;
    check("FT_Purge", unsafe {
        (drv.purge)(handle, FT_PURGE_RX | FT_PURGE_TX)
    })?;

    Ok(D2xxDevice { drv, handle })
}

/// A blocking, open FTDI handle. Owned solely by the worker thread.
pub(crate) struct D2xxDevice {
    drv: Arc<Ftd2xx>,
    handle: FtHandle,
}

// SAFETY: the handle is used only from the single worker thread that owns this
// value; `Ftd2xx` is itself `Send`.
unsafe impl Send for D2xxDevice {}

impl RawDevice for D2xxDevice {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, HexError> {
        let mut written: Dword = 0;
        let st = unsafe {
            (self.drv.write)(
                self.handle,
                bytes.as_ptr() as *const c_void,
                bytes.len() as Dword,
                &mut written,
            )
        };
        check("FT_Write", st)?;
        Ok(written as usize)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError> {
        let mut got: Dword = 0;
        let st = unsafe {
            (self.drv.read)(
                self.handle,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as Dword,
                &mut got,
            )
        };
        check("FT_Read", st)?;
        Ok(got as usize)
    }
}

impl Drop for D2xxDevice {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.drv.close)(self.handle) };
        }
    }
}
