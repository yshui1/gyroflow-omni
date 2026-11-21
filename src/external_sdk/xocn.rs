// SPDX-License-Identifier: GPL-3.0-or-later

use std::{os::windows::ffi::OsStrExt, sync::OnceLock};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LoadLibraryExW,
};
use windows::core::{PCWSTR, s};

type Probe = unsafe extern "C" fn(*const u16) -> u64;
type Configured = unsafe extern "C" fn() -> bool;

struct Reader {
    probe: Probe,
    configured: Configured,
}

static READER: OnceLock<Option<Reader>> = OnceLock::new();

fn load_reader() -> Option<Reader> {
    let dll = std::env::current_exe().ok()?.with_file_name("mdk-xocn.dll");
    if !dll.is_file() {
        return None;
    }
    let path: Vec<_> = dll.as_os_str().encode_wide().chain([0]).collect();
    // MDK and the probe share this module for the lifetime of the application.
    let module = unsafe {
        LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)
    }.ok()?;
    let probe = unsafe { GetProcAddress(module, s!("gyroflow_xocn_probe")) }?;
    let configured = unsafe { GetProcAddress(module, s!("gyroflow_xocn_configured")) }?;
    Some(Reader {
        probe: unsafe { std::mem::transmute::<_, Probe>(probe) },
        configured: unsafe { std::mem::transmute::<_, Configured>(configured) },
    })
}

pub fn enabled() -> bool {
    READER.get_or_init(load_reader).as_ref().is_some_and(|reader| unsafe { (reader.configured)() })
}

pub fn source_size(url: &str) -> Option<(usize, usize)> {
    if !enabled() {
        return None;
    }
    let path = url::Url::parse(url).ok()?.to_file_path().ok()?;
    let path: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    let reader = READER.get_or_init(load_reader).as_ref()?;
    let size = unsafe { (reader.probe)(path.as_ptr()) };
    (size != 0).then_some(((size >> 32) as usize, (size as u32) as usize))
}
