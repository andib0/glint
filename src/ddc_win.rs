//! Real DDC/CI backend via the Windows Monitor Configuration API (dxva2).

use crate::backend::{Backend, MonitorInfo};
use crate::caps;
use std::collections::{BTreeMap, HashMap};
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::Duration;
use windows_sys::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitor, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, SetVCPFeature,
};
use windows_sys::Win32::Foundation::{BOOL, HANDLE, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW,
};

const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
const MONITORINFOF_PRIMARY: u32 = 1;

struct Phys(HANDLE);
// Physical monitor handles are plain kernel handles; we only use them from one thread at a time.
unsafe impl Send for Phys {}
impl Drop for Phys {
    fn drop(&mut self) {
        unsafe { DestroyPhysicalMonitor(self.0) };
    }
}

pub struct WinBackend {
    handles: HashMap<String, Phys>,
    caps_cache: BTreeMap<String, String>,
    cache_path: Option<PathBuf>,
}

impl WinBackend {
    /// `cache_path`: where capability strings are cached (reading them takes ~1 s per monitor).
    pub fn new(cache_path: Option<PathBuf>) -> Self {
        let caps_cache = cache_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        WinBackend {
            handles: HashMap::new(),
            caps_cache,
            cache_path,
        }
    }

    fn handle(&mut self, id: &str) -> Result<HANDLE, String> {
        if !self.handles.contains_key(id) {
            self.enumerate()?;
        }
        self.handles
            .get(id)
            .map(|p| p.0)
            .ok_or_else(|| format!("monitor not connected: {id}"))
    }

    fn save_cache(&self) {
        if let (Some(p), Ok(s)) = (&self.cache_path, toml::to_string(&self.caps_cache)) {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, s);
        }
    }
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    unsafe { (*(data as *mut Vec<HMONITOR>)).push(hmon) };
    TRUE
}

fn read_caps(h: HANDLE) -> Option<String> {
    for attempt in 0..3 {
        unsafe {
            let mut len = 0u32;
            if GetCapabilitiesStringLength(h, &mut len) != 0 && len > 0 {
                let mut buf = vec![0u8; len as usize];
                if CapabilitiesRequestAndCapabilitiesReply(h, buf.as_mut_ptr(), len) != 0 {
                    let s = String::from_utf8_lossy(&buf)
                        .trim_end_matches('\0')
                        .to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100 * (attempt + 1)));
    }
    None
}

impl Backend for WinBackend {
    fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String> {
        self.handles.clear();
        let mut hmons: Vec<HMONITOR> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(collect),
                &mut hmons as *mut _ as LPARAM,
            )
        };

        let mut out = Vec::new();
        let mut cache_dirty = false;
        for hm in hmons {
            unsafe {
                let mut mi: MONITORINFOEXW = zeroed();
                mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
                if GetMonitorInfoW(hm, &mut mi as *mut _ as *mut MONITORINFO) == 0 {
                    continue;
                }
                let device = from_wide(&mi.szDevice);
                let r = mi.monitorInfo.rcMonitor;
                let mut n = 0u32;
                if GetNumberOfPhysicalMonitorsFromHMONITOR(hm, &mut n) == 0 || n == 0 {
                    continue;
                }
                let mut phys: Vec<PHYSICAL_MONITOR> = vec![zeroed(); n as usize];
                if GetPhysicalMonitorsFromHMONITOR(hm, n, phys.as_mut_ptr()) == 0 {
                    continue;
                }
                let dev_w: Vec<u16> = device.encode_utf16().chain(Some(0)).collect();
                for (j, p) in phys.iter().enumerate() {
                    let handle = Phys(p.hPhysicalMonitor);
                    let desc_buf = p.szPhysicalMonitorDescription;
                    let mut dd: DISPLAY_DEVICEW = zeroed();
                    dd.cb = size_of::<DISPLAY_DEVICEW>() as u32;
                    let mut id = String::new();
                    if EnumDisplayDevicesW(
                        dev_w.as_ptr(),
                        j as u32,
                        &mut dd,
                        EDD_GET_DEVICE_INTERFACE_NAME,
                    ) != 0
                    {
                        id = from_wide(&dd.DeviceID);
                    }
                    if id.is_empty() {
                        id = format!("{device}#{j}");
                    }
                    let caps_raw = match self.caps_cache.get(&id) {
                        Some(c) => c.clone(),
                        None => match read_caps(handle.0) {
                            Some(c) => {
                                self.caps_cache.insert(id.clone(), c.clone());
                                cache_dirty = true;
                                c
                            }
                            None => String::new(),
                        },
                    };
                    out.push(MonitorInfo {
                        id: id.clone(),
                        device: device.clone(),
                        description: from_wide(&desc_buf),
                        left: r.left,
                        top: r.top,
                        width: r.right - r.left,
                        height: r.bottom - r.top,
                        primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                        caps: caps::parse(&caps_raw),
                        caps_raw,
                    });
                    self.handles.insert(id, handle);
                }
            }
        }
        if cache_dirty {
            self.save_cache();
        }
        out.sort_by_key(|m| (m.left, m.top));
        Ok(out)
    }

    fn get(&mut self, id: &str, code: u8) -> Result<(u32, u32), String> {
        let h = self.handle(id)?;
        for attempt in 0..3 {
            let (mut cur, mut max) = (0u32, 0u32);
            if unsafe { GetVCPFeatureAndVCPFeatureReply(h, code, null_mut(), &mut cur, &mut max) }
                != 0
            {
                return Ok((cur, max));
            }
            std::thread::sleep(Duration::from_millis(30 * (attempt + 1)));
        }
        Err(format!("read VCP 0x{code:02X} failed"))
    }

    fn set(&mut self, id: &str, code: u8, value: u32) -> Result<(), String> {
        let h = self.handle(id)?;
        for attempt in 0..3 {
            if unsafe { SetVCPFeature(h, code, value) } != 0 {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(30 * (attempt + 1)));
        }
        Err(format!("write VCP 0x{code:02X}={value} failed"))
    }
}
