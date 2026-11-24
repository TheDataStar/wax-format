//! The box measures itself and writes its capability record (contract §2.2,
//! §9) to `/var/lib/deltos/capability.json`. Design: `docs/box-capability.md`.
//!
//! Everything here is a reading of the running machine, never a board name
//! and never a default. Where a reading cannot be taken, the measurement
//! fails and says which one; it does not guess.
//!
//! Every input is read under a `root` (normally `/`), so the tests can hand
//! in a made-up `/proc` and `/sys`.

use crate::fit::{BoxArch, Capability};
use crate::manifest::Profile;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub const GIB: u64 = 1024 * 1024 * 1024;
pub const GB: u64 = 1_000_000_000;

/// The record's own schema (contract §10: state carries a version its owner
/// refuses when newer).
pub const SCHEMA: u32 = 1;

/// The processes that are DeltOS services. While any is running the
/// operating system's own share of memory cannot be read
/// (`os_ram_bytes`: "with no DeltOS service running", contract §9). Names as
/// the kernel keeps them, cut at 15 bytes.
pub const SERVICES: [&str; 5] = [
    "deltd",
    "deltos-identity",
    "deltos-shelld",
    "hostapd",
    "dnsmasq",
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Read(String),
    #[error("this box is {arch}; DeltOS runs on aarch64 or x86_64 (contract §2.1)")]
    Arch { arch: String },
    #[error("the box has a Wi-Fi radio but `iw` is not installed, so whether it can be a hotspot cannot be measured (Debian/Ubuntu: apt install iw)")]
    NoIw,
    #[error("the box already has a capability record at {0}; its profile was chosen when it was set up (changing it is an admin action, not built)")]
    Exists(PathBuf),
    #[error("the box has not been set up yet: no capability record at {0} (run `deltos-capability provision --profile ...` once)")]
    Missing(PathBuf),
    #[error("{0}: written by a newer DeltOS (schema {1}); this one understands schema {SCHEMA}")]
    Newer(PathBuf, u32),
    #[error("the {profile} profile needs {need}; this box has {have} (contract §3)")]
    BelowFloor {
        profile: &'static str,
        need: String,
        have: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// The record as stored: contract §9's fields plus the schema number.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Record {
    pub schema: u32,
    #[serde(flatten)]
    pub capability: Capability,
}

/// Contract §3's floors. RAM is compared in GiB against the installed size
/// (see [`installed_ram`]); storage in GB as drives are sold, against the
/// drive's size class (see [`drive_class`]).
pub fn floor(p: Profile) -> (u64, u64) {
    match p {
        Profile::Kiosk | Profile::Classroom => (2 * GIB, 32 * GB),
        Profile::CommunityHub | Profile::FieldOps => (16 * GIB, 256 * GB),
    }
}

pub fn check_floor(c: &Capability) -> Result<()> {
    let (ram, storage) = floor(c.profile);
    let profile = c.profile.wire();
    if c.ram_bytes < ram {
        return Err(Error::BelowFloor {
            profile,
            need: format!("{} GiB of memory", ram / GIB),
            have: format!("{} GiB", c.ram_bytes / GIB),
        });
    }
    if drive_class(c.storage_bytes) < storage {
        return Err(Error::BelowFloor {
            profile,
            need: format!("a {} GB drive", storage / GB),
            have: format!(
                "a {} GB drive ({} bytes)",
                drive_class(c.storage_bytes) / GB,
                c.storage_bytes
            ),
        });
    }
    Ok(())
}

/// The size a drive is sold as: the next power of two in GB at or above its
/// bytes. A "32 GB" card holds 31.1 to 32.0 billion bytes, never more, so
/// its bytes alone would put every such card under a 32 GB floor; the same
/// for 250 GB and 256 GB drives against a 256 GB floor. Used for the profile
/// floor only: the record keeps the true bytes, and installs compare free
/// space, never this.
pub fn drive_class(bytes: u64) -> u64 {
    let gb = bytes.div_ceil(GB).max(1);
    gb.next_power_of_two() * GB
}

/// The installed memory, from the kernel's `MemTotal`: rounded up to a whole
/// GiB, because firmware, graphics and the kernel always keep a slice before
/// Linux counts (a "2 GB" board shows about 1.8 GiB), and contract §2's
/// floors are written in the sizes boards are sold in.
pub fn installed_ram(mem_total: u64) -> u64 {
    mem_total.div_ceil(GIB) * GIB
}

/// One machine's readings, taken under `root`.
pub struct Machine {
    pub root: PathBuf,
    /// The state tree, whose drive is the box's storage.
    pub state: PathBuf,
    /// `iw phy`'s output, or `None` when `iw` is not installed.
    pub iw_phy: Box<dyn Fn() -> Option<String>>,
}

impl Machine {
    /// The real machine.
    pub fn this(state: &Path) -> Machine {
        Machine {
            root: PathBuf::from("/"),
            state: state.to_path_buf(),
            iw_phy: Box::new(|| {
                let out = ["/usr/sbin/iw", "/sbin/iw", "/usr/bin/iw"]
                    .iter()
                    .find(|p| Path::new(p).exists())
                    .and_then(|p| std::process::Command::new(p).arg("phy").output().ok())?;
                out.status
                    .success()
                    .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
            }),
        }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn read(&self, rel: &str) -> Result<String> {
        let p = self.p(rel);
        fs::read_to_string(&p).map_err(|e| Error::Read(format!("{}: {e}", p.display())))
    }

    /// `MemTotal` and `MemAvailable`, in bytes.
    pub fn meminfo(&self) -> Result<(u64, u64)> {
        let t = self.read("proc/meminfo")?;
        let field = |name: &str| -> Result<u64> {
            t.lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix(':'))
                .and_then(|v| v.trim().strip_suffix("kB")?.trim().parse::<u64>().ok())
                .map(|kb| kb * 1024)
                .ok_or_else(|| Error::Read(format!("/proc/meminfo has no {name}")))
        };
        Ok((field("MemTotal")?, field("MemAvailable")?))
    }

    /// The kernel's own architecture (not this program's build).
    pub fn arch(&self) -> Result<BoxArch> {
        let a = match self.read("proc/sys/kernel/arch") {
            Ok(a) => a.trim().to_string(),
            // older kernels: uname's machine field
            Err(_) => uname_machine()
                .ok_or_else(|| Error::Read("cannot read the machine's architecture".into()))?,
        };
        match a.as_str() {
            "aarch64" | "arm64" => Ok(BoxArch::Aarch64),
            "x86_64" | "amd64" => Ok(BoxArch::X86_64),
            _ => Err(Error::Arch { arch: a }),
        }
    }

    /// The size of the whole drive the state tree is on (a "32 GB" card is
    /// 32,000,000,000 bytes or a little under), from the kernel's block
    /// devices. Where the state tree is on no block device (a container's
    /// overlay), the size of its filesystem instead.
    pub fn storage_bytes(&self) -> Result<u64> {
        let mut dir = self.state.clone();
        while !dir.exists() {
            if !dir.pop() {
                break;
            }
        }
        let md = fs::metadata(&dir).map_err(|e| Error::Read(format!("{}: {e}", dir.display())))?;
        let dev = md.dev();
        let (major, minor) = (libc::major(dev), libc::minor(dev));
        if let Some(b) = self.disk_bytes(major, minor) {
            return Ok(b);
        }
        fs_bytes(&dir)
    }

    /// `/sys/dev/block/M:m`: a partition's drive is its parent folder.
    pub fn disk_bytes(&self, major: u32, minor: u32) -> Option<u64> {
        let link = self.p(&format!("sys/dev/block/{major}:{minor}"));
        let dev = fs::canonicalize(&link).ok()?;
        let disk = if dev.join("partition").exists() {
            dev.parent()?.to_path_buf()
        } else {
            dev
        };
        let sectors: u64 = fs::read_to_string(disk.join("size"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        // the kernel counts in 512-byte sectors whatever the drive's own size
        (sectors > 0).then_some(sectors * 512)
    }

    /// `none`, or the kernel driver of each graphics device that can compute
    /// (a render node), sorted and joined with `,`: for example `v3d` on a
    /// Pi-class board, `i915` or `amdgpu` on a mini PC.
    pub fn gpu(&self) -> String {
        let mut drivers: Vec<String> = fs::read_dir(self.p("sys/class/drm"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
            .filter_map(|e| fs::read_link(e.path().join("device/driver")).ok())
            .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        drivers.sort();
        drivers.dedup();
        if drivers.is_empty() {
            "none".into()
        } else {
            drivers.join(",")
        }
    }

    pub fn has_wifi(&self) -> bool {
        fs::read_dir(self.p("sys/class/net"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.path().join("phy80211").exists())
    }

    /// A radio that lists `AP` among its supported interface modes.
    pub fn can_ap(&self) -> Result<bool> {
        if !self.has_wifi() {
            return Ok(false);
        }
        let out = (self.iw_phy)().ok_or(Error::NoIw)?;
        Ok(iw_can_ap(&out))
    }

    pub fn has_rtc(&self) -> bool {
        fs::read_dir(self.p("sys/class/rtc"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("rtc"))
    }

    /// Is any DeltOS service running now?
    pub fn services_running(&self) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(self.p("proc"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            })
            .filter_map(|e| fs::read_to_string(e.path().join("comm")).ok())
            .map(|c| c.trim().to_string())
            .filter(|c| SERVICES.contains(&c.as_str()))
            .collect();
        found.sort();
        found.dedup();
        found
    }

    /// Take every reading. `os_ram_bytes` is taken only while no DeltOS
    /// service runs; otherwise `previous` is kept (or it stays absent).
    pub fn measure(
        &self,
        profile: Profile,
        previous: Option<u64>,
    ) -> Result<(Capability, Vec<String>)> {
        let (total, available) = self.meminfo()?;
        let ram_bytes = installed_ram(total);
        let running = self.services_running();
        let os_ram_bytes = if running.is_empty() {
            Some(ram_bytes - available.min(ram_bytes))
        } else {
            previous
        };
        let cap = Capability {
            profile,
            ram_bytes,
            storage_bytes: self.storage_bytes()?,
            arch: self.arch()?,
            gpu: self.gpu(),
            has_wifi: self.has_wifi(),
            can_ap: self.can_ap()?,
            has_rtc: self.has_rtc(),
            os_ram_bytes,
        };
        Ok((cap, running))
    }
}

/// `iw phy`: does any radio's "Supported interface modes" list hold `AP`?
pub fn iw_can_ap(out: &str) -> bool {
    let mut in_modes = false;
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("Supported interface modes") {
            in_modes = true;
            continue;
        }
        if in_modes {
            match t.strip_prefix("* ") {
                Some("AP") => return true,
                Some(_) => {}
                None => in_modes = false,
            }
        }
    }
    false
}

fn uname_machine() -> Option<String> {
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 {
        return None;
    }
    let m = unsafe { std::ffi::CStr::from_ptr(u.machine.as_ptr()) };
    Some(m.to_string_lossy().into_owned())
}

fn fs_bytes(dir: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| Error::Read(e.to_string()))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(Error::Read(format!(
            "{}: {}",
            dir.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(s.f_blocks as u64 * s.f_frsize as u64)
}

pub fn read(path: &Path) -> Result<Option<Record>> {
    let t = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Read(format!("{}: {e}", path.display()))),
    };
    let v: serde_json::Value =
        serde_json::from_str(&t).map_err(|e| Error::Read(format!("{}: {e}", path.display())))?;
    // a record from before the schema number (the test stand-in) is schema 1
    let schema = v.get("schema").and_then(|s| s.as_u64()).unwrap_or(1) as u32;
    if schema > SCHEMA {
        return Err(Error::Newer(path.to_path_buf(), schema));
    }
    let capability: Capability =
        serde_json::from_value(v).map_err(|e| Error::Read(format!("{}: {e}", path.display())))?;
    Ok(Some(Record {
        schema: SCHEMA,
        capability,
    }))
}

/// Write the record whole or not at all: a reader never sees half of one.
/// `0644`: every local component may read what the box is (contract §9).
pub fn write(path: &Path, cap: &Capability) -> Result<()> {
    let rec = Record {
        schema: SCHEMA,
        capability: cap.clone(),
    };
    let mut body = serde_json::to_vec_pretty(&rec).map_err(|e| Error::Read(e.to_string()))?;
    body.push(b'\n');
    let dir = path
        .parent()
        .ok_or_else(|| Error::Read(format!("{}: no folder", path.display())))?;
    let tmp = dir.join(format!(
        ".{}.new",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let io = |e: std::io::Error| Error::Read(format!("{}: {e}", tmp.display()));
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)
        .map_err(io)?;
    f.write_all(&body).map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    fs::rename(&tmp, path).map_err(io)?;
    fs::File::open(dir).and_then(|d| d.sync_all()).map_err(io)?;
    Ok(())
}

/// First setup: the profile is chosen here, once (contract §3).
pub fn provision(
    m: &Machine,
    record: &Path,
    profile: Profile,
) -> Result<(Capability, Vec<String>)> {
    if read(record)?.is_some() {
        return Err(Error::Exists(record.to_path_buf()));
    }
    let (cap, running) = m.measure(profile, None)?;
    check_floor(&cap)?;
    write(record, &cap)?;
    Ok((cap, running))
}

/// Every boot: measure again, keeping the chosen profile. A box below its
/// profile's floor (its drive moved to smaller hardware) is still recorded as
/// it is, and the answer says so: a record that hides the truth would let the
/// fit check install what the box cannot run.
pub fn remeasure(m: &Machine, record: &Path) -> Result<(Capability, Vec<String>, Option<Error>)> {
    let prev = read(record)?.ok_or_else(|| Error::Missing(record.to_path_buf()))?;
    let (cap, running) = m.measure(prev.capability.profile, prev.capability.os_ram_bytes)?;
    let below = check_floor(&cap).err();
    if cap != prev.capability {
        write(record, &cap)?;
    }
    Ok((cap, running, below))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_rounds_up_to_the_size_sold() {
        // a 2 GB Pi 4 shows about 1.8 GiB; the 2 GB VM 1,955 MiB
        assert_eq!(installed_ram(1_849_000 * 1024), 2 * GIB);
        assert_eq!(installed_ram(1955 * 1024 * 1024), 2 * GIB);
        assert_eq!(installed_ram(2 * GIB), 2 * GIB);
        assert_eq!(installed_ram(2 * GIB + 1), 3 * GIB);
        assert_eq!(installed_ram(15_800_000 * 1024), 16 * GIB);
    }

    #[test]
    fn drives_by_the_size_sold() {
        assert_eq!(drive_class(31_914_983_424), 32 * GB);
        assert_eq!(drive_class(31_104_958_464), 32 * GB);
        assert_eq!(drive_class(32 * GB), 32 * GB);
        assert_eq!(drive_class(15_931_539_456), 16 * GB);
        assert_eq!(drive_class(250_059_350_016), 256 * GB);
        assert_eq!(drive_class(1_000_204_886_016), 1024 * GB);
    }

    #[test]
    fn iw_modes() {
        let ap = "Wiphy phy0\n\tSupported interface modes:\n\t\t * IBSS\n\t\t * managed\n\t\t * AP\n\t\t * monitor\n\tBand 1:\n";
        let no = "Wiphy phy0\n\tSupported interface modes:\n\t\t * IBSS\n\t\t * managed\n\tBand 1:\n\t\t * AP/VLAN\n\tsoftware interface modes (can always be added):\n\t\t * AP/VLAN\n";
        assert!(iw_can_ap(ap));
        assert!(!iw_can_ap(no));
        assert!(!iw_can_ap(""));
    }
}
