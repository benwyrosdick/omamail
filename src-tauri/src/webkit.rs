//! WebKitGTK's DMA-BUF renderer aborts on the proprietary Nvidia driver under
//! Wayland (`Gdk-Message: Error 71 (Protocol error) dispatching to Wayland
//! display`). Disable that renderer only when Nvidia is the GPU the session
//! actually composites on. A hybrid machine whose compositor sits on Intel or
//! AMD should keep DMA-BUF; turning it off there makes scrolling worse.

use std::path::Path;

const MESA_DRIVERS: [&str; 4] = ["i915", "xe", "amdgpu", "radeon"];

pub fn apply_nvidia_workaround() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some() {
        return;
    }
    if !Path::new("/proc/driver/nvidia/version").exists() {
        return;
    }
    if nvidia_backs_session(compositor_primary_driver(), &gpus()) {
        eprintln!("omamail: Nvidia is driving this session; disabling WebKit's DMA-BUF renderer");
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

fn nvidia_backs_session(primary: Option<Option<String>>, gpus: &[Gpu]) -> bool {
    if let Some(driver) = primary {
        return driver.as_deref() == Some("nvidia");
    }
    let mesa_present = gpus.iter().any(|gpu| MESA_DRIVERS.contains(&gpu.driver.as_str()));
    let nvidia_boot = gpus.iter().any(|gpu| gpu.driver == "nvidia" && gpu.boot_vga);
    !mesa_present || nvidia_boot
}

struct Gpu {
    driver: String,
    boot_vga: bool,
}

fn compositor_primary_driver() -> Option<Option<String>> {
    for var in ["AQ_DRM_DEVICES", "WLR_DRM_DEVICES"] {
        if let Some(devices) = std::env::var_os(var) {
            let devices = devices.to_string_lossy();
            if let Some(primary) = devices.split(':').find(|entry| !entry.is_empty()) {
                return Some(device_driver(Path::new(primary)));
            }
        }
    }
    None
}

fn gpus() -> Vec<Gpu> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("card"))
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
        })
        .filter_map(|entry| {
            let card = entry.path();
            Some(Gpu {
                driver: card_driver(&card)?,
                boot_vga: std::fs::read_to_string(card.join("device/boot_vga"))
                    .ok()
                    .is_some_and(|value| value.trim() == "1"),
            })
        })
        .collect()
}

fn card_driver(card: &Path) -> Option<String> {
    device_driver(&card.join("device"))
}

fn device_driver(device: &Path) -> Option<String> {
    std::fs::read_link(device.join("driver"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_only_session_is_worked_around() {
        let gpus = [Gpu { driver: "nvidia".into(), boot_vga: true }];
        assert!(nvidia_backs_session(None, &gpus));
    }

    #[test]
    fn intel_boot_gpu_keeps_dmabuf() {
        let gpus = [
            Gpu { driver: "i915".into(), boot_vga: true },
            Gpu { driver: "nvidia".into(), boot_vga: false },
        ];
        assert!(!nvidia_backs_session(None, &gpus));
    }

    #[test]
    fn compositor_primary_wins_over_boot_vga() {
        let gpus = [Gpu { driver: "nvidia".into(), boot_vga: true }];
        assert!(!nvidia_backs_session(Some(Some("amdgpu".into())), &gpus));
        assert!(nvidia_backs_session(Some(Some("nvidia".into())), &[]));
    }
}
