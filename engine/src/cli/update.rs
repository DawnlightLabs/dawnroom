//! `dawnroom update`: for installs Studio doesn't update, which is the CLI-only install
//! (`install.ps1 --cli`, `install.sh --cli`) and the Linux tarball install. It follows the install
//! scripts exactly: the latest release from GitHub (`DAWNROOM_RELEASES` to test against another),
//! every file checked against the release's SHA256SUMS before anything on disk changes, then the
//! engine stopped and `dawnroom` + `registry/` swapped beside the running exe. The data folder is
//! never touched. Installs that came with Studio (Windows installer, .deb) say how they update.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{BIN, Client, Fail, R, stop};

const GITHUB_LATEST: &str = "https://api.github.com/repos/DawnlightLabs/dawnroom/releases/latest";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// True when version `a` is newer than `b` ("0.1.1" > "0.1.0"; a leading v and -tails ignored).
pub(crate) fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.trim_start_matches(['v', 'V']).split(['-', '+']).next().unwrap_or("").split('.').map(|n| n.parse().unwrap_or(0)).collect() };
    let (x, y) = (parts(a), parts(b));
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (x.get(i).copied().unwrap_or(0), y.get(i).copied().unwrap_or(0));
        if p != q {
            return p > q;
        }
    }
    false
}

/// The sha256 SHA256SUMS lists for `name` (sha256sum format: `<hex>  <name>` or `<hex> *<name>`).
pub(crate) fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hex, file) = l.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name && hex.len() == 64).then(|| hex.to_ascii_lowercase())
    })
}

/// What kind of install the running exe belongs to.
#[derive(Debug, PartialEq)]
pub(crate) enum Layout {
    /// `dawnroom` + `registry/` in a folder of their own: this command updates it.
    Cli,
    /// The Windows installer (Studio beside the engine): Studio updates both.
    Studio,
    /// A system package (.deb): the package manager or Studio updates it.
    Package,
    /// A build from the repo (`target/`).
    Dev,
}

pub(crate) fn layout(exe: &Path) -> Layout {
    let dir = exe.parent().unwrap_or(Path::new("."));
    let has = |n: &str| dir.join(n).exists();
    if dir.components().any(|c| c.as_os_str() == "target") {
        Layout::Dev
    } else if dir.starts_with("/usr") {
        Layout::Package
    } else if has("dawnroom-studio.exe") || has("dawnroom-studio") {
        Layout::Studio
    } else if dir.join("registry").join("runtimes").is_dir() {
        Layout::Cli
    } else {
        Layout::Package
    }
}

fn archive_name(version: &str) -> String {
    if cfg!(windows) { format!("dawnroom-{version}-windows-x64.zip") } else { format!("dawnroom-{version}-linux-x64.tar.gz") }
}

fn fail(code: &str, why: impl std::fmt::Display) -> Fail {
    Fail::new(code, format!("{} wasn't updated: {why} Nothing was changed.", super::PRODUCT_NAME))
}

pub(super) async fn update(c: &Client) -> R<Value> {
    let exe = std::env::current_exe().and_then(|e| e.canonicalize()).map_err(|e| fail("no_exe", e))?;
    let exe = PathBuf::from(exe.to_string_lossy().trim_start_matches(r"\\?\").to_string());
    match layout(&exe) {
        Layout::Studio => return Err(Fail::new("studio_updates", "This Dawnroom came with Studio, which updates itself (Settings → Tray and updates). Open Dawnroom, or run the latest installer from dawnroom.dawnlightlabs.com/download.")),
        Layout::Package => return Err(Fail::new("package_updates", "This Dawnroom was installed as a system package (.deb). Install the new .deb from dawnroom.dawnlightlabs.com/download.")),
        Layout::Dev => return Err(Fail::new("dev_build", format!("This is a development build ({}): rebuild it from the repo instead.", exe.display()))),
        Layout::Cli => {}
    }
    let dir = exe.parent().unwrap().to_path_buf();
    let _ = std::fs::remove_file(dir.join(format!("{BIN}.old{}", std::env::consts::EXE_SUFFIX))); // left by the last update

    let releases = std::env::var("DAWNROOM_RELEASES").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| GITHUB_LATEST.into());
    let http = reqwest::Client::builder().user_agent(concat!("dawnroom-cli/", env!("CARGO_PKG_VERSION"))).build().map_err(|e| fail("http_error", e))?;
    let get = |url: String| {
        let http = http.clone();
        async move {
            let r = http.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| fail("download_failed", format!("couldn't download {url} ({e}).")))?;
            r.bytes().await.map_err(|e| fail("download_failed", format!("the download of {url} broke off ({e})."))).map(|b| b.to_vec())
        }
    };
    let rel: Value = serde_json::from_slice(&get(releases.clone()).await?).map_err(|_| fail("bad_release", format!("{releases} didn't answer with a release.")))?;
    let latest = rel["tag_name"].as_str().map(|t| t.trim_start_matches(['v', 'V']).to_string()).ok_or_else(|| fail("bad_release", "couldn't read the latest version."))?;
    if !newer(&latest, VERSION) {
        if !c.json {
            println!("{} {VERSION} is the latest.", super::PRODUCT_NAME);
        }
        return Ok(json!({ "updated": false, "version": VERSION, "latest": latest }));
    }
    let asset = |name: &str| rel["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name)).and_then(|x| x["browser_download_url"].as_str()).map(String::from);

    // Everything is downloaded and checked before anything on disk changes.
    let name = archive_name(&latest);
    let sums = String::from_utf8_lossy(&get(asset("SHA256SUMS").ok_or_else(|| fail("bad_release", "the release has no SHA256SUMS."))?).await?).into_owned();
    if !c.json {
        eprintln!("Downloading {name}…");
    }
    let bytes = get(asset(&name).ok_or_else(|| fail("bad_release", format!("the release has no {name}.")))?).await?;
    let want = sum_for(&sums, &name).ok_or_else(|| fail("bad_release", format!("{name} isn't listed in SHA256SUMS.")))?;
    if format!("{:x}", Sha256::digest(&bytes)) != want {
        return Err(fail("checksum_mismatch", format!("{name} doesn't match its checksum (download damaged or tampered with).")));
    }
    let new = dir.join(".update-new");
    let _ = std::fs::remove_dir_all(&new);
    std::fs::create_dir_all(&new).map_err(|e| fail("io", e))?;
    unpack(&bytes, &name, &new).map_err(|e| (std::fs::remove_dir_all(&new).ok(), fail("bad_archive", format!("couldn't unpack {name} ({e})."))).1)?;
    let new_exe = new.join(format!("{BIN}{}", std::env::consts::EXE_SUFFIX));
    if !new_exe.is_file() || !new.join("registry").join("runtimes").is_dir() {
        let _ = std::fs::remove_dir_all(&new);
        return Err(fail("bad_archive", format!("{name} has no {BIN} or registry.")));
    }
    // Full Linux installs also carry Studio as an AppImage beside the engine; it's swapped too.
    let appimage = dir.join("Dawnroom.AppImage");
    let app_bytes = if appimage.is_file() && cfg!(not(windows)) {
        let app = format!("Dawnroom_{latest}_amd64.AppImage");
        let b = get(asset(&app).ok_or_else(|| fail("bad_release", format!("the release has no {app}.")))?).await?;
        if Some(format!("{:x}", Sha256::digest(&b))) != sum_for(&sums, &app) {
            let _ = std::fs::remove_dir_all(&new);
            return Err(fail("checksum_mismatch", format!("{app} doesn't match its checksum.")));
        }
        Some(b)
    } else {
        None
    };

    // A running engine lets go of its files, then the swap: the running exe is renamed aside
    // (Windows allows that; it's deleted at the next update), the new one takes its name.
    let _ = stop(c).await;
    let old = dir.join(format!("{BIN}.old{}", std::env::consts::EXE_SUFFIX));
    let swap = || -> std::io::Result<()> {
        std::fs::rename(&exe, &old)?;
        if let Err(e) = std::fs::rename(&new_exe, &exe) {
            let _ = std::fs::rename(&old, &exe);
            return Err(e);
        }
        let reg = dir.join("registry");
        let reg_old = dir.join(".registry-old");
        let _ = std::fs::remove_dir_all(&reg_old);
        if reg.exists() {
            std::fs::rename(&reg, &reg_old)?;
        }
        std::fs::rename(new.join("registry"), &reg)?;
        let _ = std::fs::remove_dir_all(&reg_old);
        if let Some(b) = &app_bytes {
            let tmp = dir.join(".Dawnroom.AppImage.new");
            std::fs::write(&tmp, b)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            }
            std::fs::rename(&tmp, &appimage)?;
        }
        // Linux can delete the old binary now; Windows holds it while this process runs.
        if cfg!(not(windows)) {
            let _ = std::fs::remove_file(&old);
        }
        Ok(())
    };
    let done = swap();
    let _ = std::fs::remove_dir_all(&new);
    done.map_err(|e| Fail::new("io", format!("The update stopped halfway ({e}). Run `{BIN} update` again, or reinstall with the install script.")))?;
    if !c.json {
        let what = if app_bytes.is_some() { "the engine and Studio" } else { "the engine" };
        println!("{} {latest} installed (was {VERSION}): {what}. Your models and images are where they were.", c.st.accent("Updated"));
    }
    Ok(json!({ "updated": true, "from": VERSION, "version": latest, "appimage": app_bytes.is_some() }))
}

fn unpack(bytes: &[u8], name: &str, to: &Path) -> Result<(), String> {
    if name.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        z.extract(to).map_err(|e| e.to_string())
    } else {
        // Linux always has tar; the archive is written to the folder first.
        let tgz = to.join(".archive.tar.gz");
        std::fs::write(&tgz, bytes).map_err(|e| e.to_string())?;
        let ok = std::process::Command::new("tar").arg("-xzf").arg(&tgz).arg("-C").arg(to).status().map_err(|e| e.to_string())?.success();
        let _ = std::fs::remove_file(&tgz);
        ok.then_some(()).ok_or_else(|| "tar failed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.1.1", "0.1.0"));
        assert!(newer("v0.10.0", "0.9.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn sums() {
        let h = "a".repeat(64);
        let s = format!("{h}  dawnroom-0.1.1-windows-x64.zip\n{}  *SHA-other\n", "b".repeat(64));
        assert_eq!(sum_for(&s, "dawnroom-0.1.1-windows-x64.zip"), Some(h));
        assert_eq!(sum_for(&s, "SHA-other").map(|x| x.len()), Some(64));
        assert_eq!(sum_for(&s, "missing"), None);
    }

    #[test]
    fn layouts() {
        let t = std::env::temp_dir().join(format!("dawnroom-layout-{}", std::process::id()));
        let cli = t.join("cli");
        std::fs::create_dir_all(cli.join("registry").join("runtimes")).unwrap();
        assert_eq!(layout(&cli.join("dawnroom.exe")), Layout::Cli);
        let studio = t.join("studio");
        std::fs::create_dir_all(studio.join("registry").join("runtimes")).unwrap();
        std::fs::write(studio.join("dawnroom-studio.exe"), b"").unwrap();
        assert_eq!(layout(&studio.join("dawnroom.exe")), Layout::Studio);
        let pkg = t.join("usr").join("bin");
        std::fs::create_dir_all(&pkg).unwrap();
        assert_eq!(layout(&pkg.join("dawnroom")), Layout::Package);
        if cfg!(unix) {
            // A .deb puts Studio beside the engine in /usr/bin: still a system package.
            assert_eq!(layout(Path::new("/usr/bin/dawnroom")), Layout::Package);
        }
        assert_eq!(layout(Path::new("D:/x/engine/target/release/dawnroom.exe")), Layout::Dev);
        let _ = std::fs::remove_dir_all(&t);
    }
}
