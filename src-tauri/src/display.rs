//! A terminal started outside the desktop session often has no `WAYLAND_DISPLAY`
//! or `DISPLAY`. GTK then refuses to start. If a socket for this user exists,
//! use it.

use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

pub fn ensure_display() -> bool {
    if has_display() {
        return true;
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", uid())));
    if let Some(name) = wayland_socket(&runtime) {
        eprintln!("omamail: this terminal has no display; using {name}");
        std::env::set_var("WAYLAND_DISPLAY", &name);
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        }
        return true;
    }
    if Path::new("/tmp/.X11-unix/X0").exists() {
        eprintln!("omamail: this terminal has no display; using :0");
        std::env::set_var("DISPLAY", ":0");
        return true;
    }
    false
}

fn has_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

fn wayland_socket(runtime: &Path) -> Option<String> {
    let mut names = std::fs::read_dir(runtime)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("wayland-") || name.ends_with(".lock") {
                return None;
            }
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_socket())
                .map(|_| name)
        })
        .collect::<Vec<_>>();
    names.sort();
    names.pop()
}

fn uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let rest = line.strip_prefix("Uid:")?;
                rest.split_whitespace().next()?.parse().ok()
            })
        })
        .unwrap_or(0)
}
