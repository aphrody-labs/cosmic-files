// SPDX-License-Identifier: GPL-3.0-only
//! Bun OS: COSMIC Files on Windows drives the same Shell namespace as Explorer, through the
//! WinClean shell surface (`bun-windows-winclean::shell`): Explorer's own context menu
//! (shell extensions included), the Properties sheet, the Windows "Open with" dialog, "Show in
//! Explorer", drives, the Bun OS Linux filesystem (`\\wsl.localhost\BunOS`) and Explorer
//! thumbnails. The Shell calls are modal or block on COM, so each runs on its own STA thread.

use std::path::{Path, PathBuf};

use bun_windows_winclean::shell::{namespace, popup};
use serde_json::{Value, json};

fn strings(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| p.display().to_string()).collect()
}

fn on_thread(name: &str, job: impl FnOnce() -> std::io::Result<Value> + Send + 'static) {
    let label = name.to_owned();
    let _ = std::thread::Builder::new().name(format!("windows-shell-{name}")).spawn(move || {
        if let Err(err) = job() {
            log::warn!("windows shell {label}: {err}");
        }
    });
}

/// Explorer's context menu ("Show more options") for the selection, at the cursor.
pub fn context_menu(paths: Vec<PathBuf>) {
    on_thread("menu", move || popup::show(&strings(&paths), None, None));
}

/// Explorer's Properties sheet for each selected item.
pub fn properties(paths: Vec<PathBuf>) {
    for path in paths {
        on_thread("properties", move || {
            namespace::properties(&json!({ "path": path.display().to_string(), "action": "dialog" }))
        });
    }
}

/// The Windows "Open with" dialog (the `openas` verb) for one item.
pub fn open_with(path: PathBuf) {
    on_thread("open-with", move || {
        namespace::context_menu_tool(&json!({
            "path": path.display().to_string(), "action": "invoke", "verb": "openas", "wait": true,
        }))
    });
}

/// Open Explorer on the item's folder with the item selected.
pub fn reveal(paths: Vec<PathBuf>) {
    for path in paths {
        on_thread("reveal", move || namespace::reveal(&json!({ "path": path.display().to_string() })));
    }
}

/// Open the Bun OS terminal in `dir`: Rio running nushell when installed under
/// `%SystemRoot%\BunOS\bin`, else Windows Terminal (whose default profile is nushell).
pub fn open_terminal(dir: &Path) -> std::io::Result<()> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let rio = root.join(r"BunOS\bin\rio.exe");
    let mut command = if rio.is_file() {
        let mut c = std::process::Command::new(rio);
        c.arg("--working-dir").arg(dir);
        c
    } else {
        let mut c = std::process::Command::new("wt.exe");
        c.arg("-d").arg(dir);
        c
    };
    command.current_dir(dir);
    command.spawn().map(|_| ())
}

/// Sidebar places Explorer shows under "This PC": every drive root with its label.
pub fn drives() -> Vec<(String, PathBuf)> {
    let Ok(value) = namespace::drives() else { return Vec::new() };
    value["drives"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let root = d["root"].as_str()?;
            let label = d["label"].as_str().filter(|l| !l.is_empty()).unwrap_or(match d["type"].as_str() {
                Some("removable") => "USB",
                Some("network") => "Network",
                Some("cdrom") => "CD",
                _ => "Local Disk",
            });
            Some((format!("{label} ({})", root.trim_end_matches('\\')), PathBuf::from(root)))
        })
        .collect()
}

/// The Bun OS WSL distribution's filesystem, when the distribution is registered. Read from the
/// per-user Lxss registry so listing the sidebar never boots the WSL VM.
pub fn bunos_linux_root() -> Option<PathBuf> {
    use bun_windows_winclean::registry::{get_value, list_key};
    let lxss = list_key(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss").ok()?;
    let name = lxss["SubKeys"].as_array()?.iter().filter_map(Value::as_str).find_map(|key| {
        let value = get_value(key, "DistributionName").ok()?;
        let name = value["Value"].as_str()?.to_owned();
        name.eq_ignore_ascii_case("BunOS").then_some(name)
    })?;
    Some(PathBuf::from(format!(r"\\wsl.localhost\{name}")))
}

/// The thumbnail Explorer would draw for `path` (registered `IThumbnailProvider`s), as PNG bytes.
pub fn thumbnail_png(path: &Path, size: u32) -> Option<Vec<u8>> {
    let value = namespace::thumbnail(&json!({ "path": path.display().to_string(), "size": size })).ok()?;
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(value["dataBase64"].as_str()?).ok()
}
