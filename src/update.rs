// SPDX-FileCopyrightText: 2026 Abhilesh Singh
// SPDX-License-Identifier: GPL-3.0-or-later

//! Updating Glance from its GitHub releases.
//!
//! The newest version is found without GitHub's API: the page for the latest
//! release redirects to that release's tag, and the tag is the version. Every
//! release carries the same files under the same names, and a `SHA256SUMS`
//! listing them, so the file a copy of Glance needs is always found the same
//! way, and is checked against that list before it is used.
//!
//! How the new version goes in depends on how this copy was installed:
//!
//! - an AppImage replaces its own file, and the next start is the new one;
//! - a Flatpak installed from the release's bundle reinstalls from the new
//!   bundle, through Flatpak on the host;
//! - the Arch package is installed with pacman, which needs the
//!   administrator's password, asked for by the desktop through pkexec;
//! - a Flatpak from Flathub is updated by Flatpak itself, and a copy built
//!   from source by whoever built it: for those an update is only announced.
//!
//! Everything here blocks, and is run on a thread of its own.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use gtk::prelude::*;
use gtk::{gio, glib};
use soup::prelude::*;

pub const REPOSITORY: &str = "https://github.com/ABHILESH1412/glance";

/// Where releases are looked for: GitHub, unless `GLANCE_UPDATE_SOURCE`
/// names another place laid out the same way — a fork, or a test.
pub fn repository() -> String {
    std::env::var("GLANCE_UPDATE_SOURCE").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| REPOSITORY.to_string())
}
const APP_ID: &str = "io.github.abhilesh1412.Glance";
/// The Arch package's name, which pacman knows it by.
const PACKAGE: &str = "glance-image-viewer";
/// The largest file a release holds is the AppImage, about 50 MB; anything
/// far beyond that is not one of ours.
const LARGEST: u64 = 512 * 1024 * 1024;

/// A version, major.minor.patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// "2.0.1", or a tag's "v2.0.1".
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|p| p.parse::<u32>().ok());
        let version = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    /// The version running.
    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }

    pub fn tag(self) -> String {
        format!("v{self}")
    }

    /// The release's page, with what changed.
    pub fn page(self) -> String {
        format!("{}/releases/tag/{}", repository(), self.tag())
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// How this copy of Glance was installed, and so how it is updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// An AppImage, by the file it runs from.
    AppImage(PathBuf),
    /// A Flatpak, in the system's installation or the user's.
    Flatpak { system: bool },
    /// The Arch package, installed with pacman.
    Pacman,
    /// Built from source, or installed some other way: only told about.
    Other,
}

impl Install {
    /// How the running copy was installed.
    pub fn detect() -> Install {
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file());
        let flatpak_info = std::fs::read_to_string("/.flatpak-info").ok();
        let exe = std::env::current_exe().ok();
        let packaged = exe.as_deref() == Some(Path::new("/usr/bin/glance")) && pacman_has_us();
        Install::from_parts(appimage, flatpak_info.as_deref(), packaged)
    }

    fn from_parts(appimage: Option<PathBuf>, flatpak_info: Option<&str>, pacman: bool) -> Install {
        if let Some(path) = appimage {
            return Install::AppImage(path);
        }
        if let Some(info) = flatpak_info {
            // A system installation keeps its apps under /var/lib/flatpak.
            let system = info.lines().any(|l| l.starts_with("app-path=") && l.contains("/var/lib/flatpak/"));
            return Install::Flatpak { system };
        }
        if pacman { Install::Pacman } else { Install::Other }
    }

    /// The release file this kind of copy is updated from, if any. Only
    /// 64-bit Intel and AMD builds are released.
    pub fn asset(&self) -> Option<&'static str> {
        if std::env::consts::ARCH != "x86_64" {
            return None;
        }
        match self {
            Install::AppImage(_) => Some("Glance-x86_64.AppImage"),
            Install::Flatpak { .. } => Some("Glance-x86_64.flatpak"),
            Install::Pacman => Some("glance-image-viewer-x86_64.pkg.tar.zst"),
            Install::Other => None,
        }
    }

    /// Whether an update goes in without asking anything: the Arch package
    /// always needs the password.
    pub fn installs_quietly(&self) -> bool {
        matches!(self, Install::AppImage(_) | Install::Flatpak { .. })
    }
}

/// Whether pacman installed /usr/bin/glance, as the package it knows.
fn pacman_has_us() -> bool {
    std::fs::read_dir("/var/lib/pacman/local").is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // "glance-image-viewer-2.0.0-1": the name, then version and release.
            name.strip_prefix(PACKAGE).and_then(|rest| rest.strip_prefix('-')).is_some_and(|rest| {
                rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            })
        })
    })
}

fn session() -> soup::Session {
    let session = soup::Session::new();
    session.set_user_agent(&format!("Glance/{} ", env!("CARGO_PKG_VERSION")));
    session.set_timeout(60);
    session
}

/// The newest version released, from where the latest release's page
/// redirects to.
pub fn latest() -> Result<Version, String> {
    let failed = |what: String| format!("Could not check for updates: {what}");
    let message = soup::Message::new("GET", &format!("{}/releases/latest", repository())).map_err(|e| failed(e.to_string()))?;
    message.add_flags(soup::MessageFlags::NO_REDIRECT);
    let stream = session().send(&message, gio::Cancellable::NONE).map_err(|e| failed(e.to_string()))?;
    let _ = stream.close(gio::Cancellable::NONE);
    let location = message
        .response_headers()
        .and_then(|headers| headers.one("Location"))
        .ok_or_else(|| failed(format!("GitHub answered {:?}", message.status())))?;
    version_in(&location).ok_or_else(|| failed(format!("no version in “{location}”")))
}

/// The version at the end of a release's address: ".../releases/tag/v2.0.1".
fn version_in(location: &str) -> Option<Version> {
    let (_, tag) = location.rsplit_once("/tag/")?;
    Version::parse(tag.split(['?', '#']).next()?)
}

/// What a release's SHA256SUMS says the file should add up to.
fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (sum, name) = line.split_once(char::is_whitespace)?;
        // `sha256sum` marks a file read as binary with an asterisk.
        let name = name.trim().trim_start_matches('*');
        (name == file && sum.len() == 64 && sum.chars().all(|c| c.is_ascii_hexdigit())).then(|| sum.to_ascii_lowercase())
    })
}

/// Where downloaded updates wait to be put in.
fn folder() -> PathBuf {
    glib::user_cache_dir().join("glance").join("updates")
}

/// Download `version`'s file for `install`, checked, and give back where it
/// is. `progress` hears the share done, from 0 to 1.
pub fn download(version: Version, install: &Install, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let asset = install.asset().ok_or("There is no ready-made update for this copy of Glance.")?;
    let failed = |what: String| format!("Could not download Glance {version}: {what}");
    let base = format!("{}/releases/download/{}", repository(), version.tag());
    let session = session();

    let get = |url: &str| -> Result<(soup::Message, gio::InputStream), String> {
        let message = soup::Message::new("GET", url).map_err(|e| failed(e.to_string()))?;
        let stream = session.send(&message, gio::Cancellable::NONE).map_err(|e| failed(e.to_string()))?;
        if message.status() != soup::Status::Ok {
            return Err(failed(format!("GitHub answered {:?}", message.status())));
        }
        Ok((message, stream))
    };

    let (_, sums) = get(&format!("{base}/SHA256SUMS"))?;
    let mut text = String::new();
    sums.into_read().take(64 * 1024).read_to_string(&mut text).map_err(|e| failed(e.to_string()))?;
    let expected = checksum_for(&text, asset).ok_or_else(|| failed(format!("the release does not list {asset}")))?;

    let (message, stream) = get(&format!("{base}/{asset}"))?;
    let size = message.response_headers().map(|h| h.content_length()).filter(|&n| n > 0);
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|e| failed(e.to_string()))?;
    let target = folder.join(asset);
    let partial = folder.join(format!("{asset}.download"));
    let mut file = std::fs::File::create(&partial).map_err(|e| failed(e.to_string()))?;
    let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256).ok_or_else(|| failed("no SHA-256".into()))?;
    let mut reader = stream.into_read();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let outcome = loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(read) => read,
            Err(e) => break Err(failed(e.to_string())),
        };
        checksum.update(&buffer[..read]);
        if let Err(e) = file.write_all(&buffer[..read]) {
            break Err(failed(e.to_string()));
        }
        done += read as u64;
        if done > LARGEST {
            break Err(failed("it was far larger than it should be".into()));
        }
        if let Some(size) = size {
            progress((done as f64 / size as f64).min(1.0));
        }
    };
    drop(file);
    let checked = outcome.and_then(|()| {
        (checksum.string().unwrap_or_default() == expected)
            .then_some(())
            .ok_or_else(|| failed("it did not arrive intact. Try again later".into()))
    });
    match checked.and_then(|()| std::fs::rename(&partial, &target).map_err(|e| failed(e.to_string()))) {
        Ok(()) => Ok(target),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// Whether Flatpak got this copy from Flathub, which updates it itself.
pub fn flatpak_from_flathub() -> bool {
    Command::new("flatpak-spawn")
        .args(["--host", "flatpak", "info", "--show-origin", APP_ID])
        .output()
        .is_ok_and(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "flathub")
}

/// Put a downloaded update in. It is used from the next start on.
pub fn install(install: &Install, file: &Path) -> Result<(), String> {
    let result = match install {
        Install::AppImage(target) => replace_appimage(file, target),
        Install::Flatpak { system } => run(
            Command::new("flatpak-spawn").arg("--host").arg("flatpak").args([
                "install",
                if *system { "--system" } else { "--user" },
                "--noninteractive",
                "-y",
                "--reinstall",
                "--bundle",
            ])
            .arg(file),
            "Flatpak",
        ),
        // pkexec has the desktop ask for the password, and runs only pacman.
        Install::Pacman => run(Command::new("pkexec").args(["pacman", "-U", "--noconfirm"]).arg(file), "pacman"),
        Install::Other => Err("This copy of Glance cannot update itself.".to_string()),
    };
    if result.is_ok() {
        let _ = std::fs::remove_file(file);
    }
    result
}

fn run(command: &mut Command, what: &str) -> Result<(), String> {
    let out = command.output().map_err(|e| format!("Could not start {what}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    // pkexec's own codes: the password dialog was closed, or not allowed.
    if what == "pacman" && matches!(out.status.code(), Some(126 | 127)) {
        return Err("The update was not installed: it needs the administrator's password.".to_string());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("it stopped with an error").trim();
    Err(format!("{what} could not install the update: {said}"))
}

/// The AppImage's file, replaced whole: the new one written beside it, then
/// renamed over it, so a failure leaves the old one as it was. The running
/// copy goes on running from the old file until it ends.
fn replace_appimage(file: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let failed = |e: std::io::Error| format!("Could not replace {}: {e}", target.display());
    let name = target.file_name().map_or_else(|| "Glance.AppImage".into(), |n| n.to_string_lossy().into_owned());
    let staged = target.with_file_name(format!(".{name}.update-{}", std::process::id()));
    let result = std::fs::copy(file, &staged)
        .and_then(|_| std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)))
        .and_then(|()| std::fs::File::open(&staged)?.sync_all())
        .and_then(|()| std::fs::rename(&staged, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result.map_err(failed)
}

/// The command that starts the new version once this one has ended: after
/// a moment, so this copy has let go of its name and does not take the new
/// one's start for a second window of its own.
pub fn relaunch_command(install: &Install) -> Option<Command> {
    let wait_then = |program: &str, args: &[&str]| {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 1; exec \"$@\"", "glance-restart", program]).args(args);
        command
    };
    match install {
        Install::AppImage(path) => Some(wait_then(&path.to_string_lossy(), &[])),
        Install::Flatpak { .. } => {
            let mut command = Command::new("flatpak-spawn");
            command.args(["--host", "sh", "-c", "sleep 1; exec flatpak run \"$0\"", APP_ID]);
            Some(command)
        }
        // Where pacman put it: once replaced, the running program's own
        // path reads "/usr/bin/glance (deleted)".
        Install::Pacman => Some(wait_then("/usr/bin/glance", &[])),
        Install::Other => {
            let exe = std::env::current_exe().ok()?;
            let exe = exe.to_string_lossy();
            Some(wait_then(exe.strip_suffix(" (deleted)").unwrap_or(&exe), &[]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_and_put_in_order() {
        assert_eq!(Version::parse("v2.0.1"), Some(Version(2, 0, 1)));
        assert_eq!(Version::parse("10.2.33"), Some(Version(10, 2, 33)));
        assert_eq!(Version::parse("2.0"), None);
        assert_eq!(Version::parse("2.0.1.4"), None);
        assert_eq!(Version::parse("v2.x.1"), None);
        assert!(Version(2, 0, 10) > Version(2, 0, 9));
        assert!(Version(2, 1, 0) > Version(2, 0, 99));
        assert!(Version(3, 0, 0) > Version(2, 9, 9));
        assert_eq!(Version(2, 0, 1).tag(), "v2.0.1");
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_latest_release_is_read_from_where_it_redirects() {
        let at = "https://github.com/ABHILESH1412/glance/releases/tag/v2.1.0";
        assert_eq!(version_in(at), Some(Version(2, 1, 0)));
        assert_eq!(version_in("https://github.com/ABHILESH1412/glance/releases"), None);
    }

    #[test]
    fn a_file_is_found_in_the_checksum_list() {
        let sums = format!(
            "{a}  glance-image-viewer-x86_64.pkg.tar.zst\n{b} *Glance-x86_64.AppImage\nshort  Glance-x86_64.flatpak\n",
            a = "a".repeat(64),
            b = "B".repeat(64)
        );
        assert_eq!(checksum_for(&sums, "glance-image-viewer-x86_64.pkg.tar.zst"), Some("a".repeat(64)));
        assert_eq!(checksum_for(&sums, "Glance-x86_64.AppImage"), Some("b".repeat(64)));
        assert_eq!(checksum_for(&sums, "Glance-x86_64.flatpak"), None, "not a whole checksum");
        assert_eq!(checksum_for(&sums, "Glance.AppImage"), None);
    }

    #[test]
    fn how_a_copy_was_installed_decides_how_it_is_updated() {
        let image = PathBuf::from("/home/me/Applications/Glance-x86_64.AppImage");
        assert_eq!(Install::from_parts(Some(image.clone()), None, false), Install::AppImage(image));
        let user = "[Instance]\napp-path=/home/me/.local/share/flatpak/app/io.github.abhilesh1412.Glance/x86_64\n";
        let system = "[Instance]\napp-path=/var/lib/flatpak/app/io.github.abhilesh1412.Glance/x86_64\n";
        assert_eq!(Install::from_parts(None, Some(user), false), Install::Flatpak { system: false });
        assert_eq!(Install::from_parts(None, Some(system), false), Install::Flatpak { system: true });
        assert_eq!(Install::from_parts(None, None, true), Install::Pacman);
        assert_eq!(Install::from_parts(None, None, false), Install::Other);
        assert!(Install::Flatpak { system: false }.installs_quietly());
        assert!(!Install::Pacman.installs_quietly());
        assert_eq!(Install::Other.asset(), None);
    }

    #[test]
    fn an_appimage_is_replaced_whole_and_left_runnable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("glance-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (old, new) = (dir.join("Glance.AppImage"), dir.join("download"));
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        replace_appimage(&new, &old).unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&old).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "nothing left over beside it");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
