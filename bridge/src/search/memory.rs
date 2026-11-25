// SPDX-License-Identifier: GPL-3.0-or-later
//! Snapshot of physical memory available to a search. Admission is advisory:
//! operating-system estimates can change while files are being read.

#[cfg(target_os = "linux")]
pub(super) fn available_bytes() -> Option<u64> {
    use std::fs;

    let memory = fs::read_to_string("/proc/meminfo").ok().and_then(|text| {
        text.lines().find_map(|line| {
            line.strip_prefix("MemAvailable:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()?
                .checked_mul(1024)
        })
    });
    let limit = fs::read("/proc/self/cgroup").ok().and_then(|membership| {
        let mounts = fs::read("/proc/self/mountinfo").unwrap_or_default();
        cgroup::remaining(&membership, &mounts)
    });
    memory.into_iter().chain(limit).min()
}

#[cfg(target_os = "linux")]
mod cgroup {
    use std::{
        ffi::{OsStr, OsString},
        fs,
        os::unix::ffi::{OsStrExt, OsStringExt},
        path::{Component, Path, PathBuf},
    };

    // mountinfo escapes these four bytes. Paths need not be valid UTF-8.
    fn mount_path(mut bytes: &[u8]) -> PathBuf {
        let mut decoded = Vec::with_capacity(bytes.len());
        while let [first, rest @ ..] = bytes {
            let (byte, tail) = match (*first, rest) {
                (b'\\', [b'0', b'4', b'0', tail @ ..]) => (b' ', tail),
                (b'\\', [b'0', b'1', b'1', tail @ ..]) => (b'\t', tail),
                (b'\\', [b'0', b'1', b'2', tail @ ..]) => (b'\n', tail),
                (b'\\', [b'1', b'3', b'4', tail @ ..]) => (b'\\', tail),
                other => other,
            };
            decoded.push(byte);
            bytes = tail;
        }
        PathBuf::from(OsString::from_vec(decoded))
    }

    fn memory_mount(line: &[u8], unified: bool) -> Option<(PathBuf, PathBuf)> {
        let mut fields = line.split(|&b| b == b' ');
        let root = fields.nth(3)?;
        let point = fields.next()?;
        fields.find(|&field| field == b"-")?;
        let filesystem = fields.next()?;
        let options = fields.nth(1)?;
        let relevant = if unified {
            filesystem == b"cgroup2"
        } else {
            filesystem == b"cgroup" && options.split(|&b| b == b',').any(|name| name == b"memory")
        };
        relevant.then(|| (mount_path(root), mount_path(point)))
    }

    fn headroom(point: &Path, suffix: &Path, maximum: &str, usage: &str) -> Option<u64> {
        // A cgroup namespace can show a membership outside the visible tree
        // using "..". Never walk beyond the mounted hierarchy.
        let mut leaf = point.to_owned();
        if suffix
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            leaf.push(suffix);
        }
        let number = |p| fs::read_to_string(p).ok()?.trim().parse::<u64>().ok();
        leaf.ancestors()
            .take_while(|p| p.starts_with(point))
            .filter_map(|dir| {
                Some(number(dir.join(maximum))?.saturating_sub(number(dir.join(usage))?))
            })
            .min()
    }

    pub(super) fn remaining(membership: &[u8], mounts: &[u8]) -> Option<u64> {
        membership
            .split(|&b| b == b'\n')
            .filter_map(|line| {
                let mut fields = line.splitn(3, |&b| b == b':');
                let controllers = fields.nth(1)?;
                let group = Path::new(OsStr::from_bytes(fields.next()?));
                let from_root = group.strip_prefix("/").ok()?;
                let unified = controllers.is_empty();
                let (maximum, usage, default_mount) = if unified {
                    ("memory.max", "memory.current", "/sys/fs/cgroup")
                } else if controllers
                    .split(|&b| b == b',')
                    .any(|name| name == b"memory")
                {
                    (
                        "memory.limit_in_bytes",
                        "memory.usage_in_bytes",
                        "/sys/fs/cgroup/memory",
                    )
                } else {
                    return None;
                };
                let mut points = mounts
                    .split(|&b| b == b'\n')
                    .filter_map(|line| memory_mount(line, unified))
                    .peekable();
                if points.peek().is_none() {
                    return headroom(Path::new(default_mount), from_root, maximum, usage);
                }
                points
                    .filter_map(|(root, point)| {
                        // An inherited mount can be rooted above the new cgroup
                        // namespace ("/.."). Its own limit still applies.
                        let inherited_root = root
                            .components()
                            .all(|c| matches!(c, Component::RootDir | Component::ParentDir));
                        // A bind mount can start halfway down the hierarchy.
                        let suffix = match group.strip_prefix(&root) {
                            Ok(suffix) => suffix,
                            Err(_) if inherited_root => Path::new(""),
                            Err(_) => return None,
                        };
                        headroom(&point, suffix, maximum, usage)
                    })
                    .min()
            })
            .min()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn nested_limits_follow_the_mount_root_and_point() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let point =
                std::env::temp_dir().join(format!("hn cgroup {} {stamp}", std::process::id()));
            fs::create_dir_all(point.join("team/app")).unwrap();
            let mut encoded_point = Vec::new();
            for &byte in point.as_os_str().as_bytes() {
                match byte {
                    b' ' => encoded_point.extend_from_slice(br"\040"),
                    b'\t' => encoded_point.extend_from_slice(br"\011"),
                    b'\n' => encoded_point.extend_from_slice(br"\012"),
                    b'\\' => encoded_point.extend_from_slice(br"\134"),
                    _ => encoded_point.push(byte),
                }
            }
            for (membership, kind, max, used) in [
                (
                    "0::/tenant A/team/app",
                    "cgroup2 none rw",
                    "memory.max",
                    "memory.current",
                ),
                (
                    "4:cpu,memory:/tenant A/team/app",
                    "cgroup none rw,cpu,memory",
                    "memory.limit_in_bytes",
                    "memory.usage_in_bytes",
                ),
            ] {
                let mounts = |root: &[u8]| {
                    [
                        b"30 5 0:22 ",
                        root,
                        b" ",
                        &encoded_point,
                        format!(" rw shared:4 - {kind}\n").as_bytes(),
                    ]
                    .concat()
                };
                let tenant = mounts(br"/tenant\040A");
                for (dir, limit, usage) in [
                    ("", "1000", "100"),
                    ("team", "120", "70"),
                    ("team/app", "80", "60"),
                ] {
                    fs::write(point.join(dir).join(max), limit).unwrap();
                    fs::write(point.join(dir).join(used), usage).unwrap();
                }
                let available = || remaining(membership.as_bytes(), &tenant);
                assert_eq!(available(), Some(20)); // the leaf is limiting
                fs::write(point.join("team/app").join(max), "1000").unwrap();
                assert_eq!(available(), Some(50)); // the ancestor is limiting
                fs::write(point.join(max), "110").unwrap();
                assert_eq!(available(), Some(10)); // the root of the bind mount is limiting
                let sibling = membership.replace("tenant A", "tenant AB");
                assert_eq!(remaining(sibling.as_bytes(), &tenant), None);
                assert_eq!(remaining(membership.as_bytes(), &mounts(b"/..")), Some(10));
            }
            fs::remove_dir_all(point).unwrap();
        }

        #[test]
        fn v1_controllers_and_native_mount_paths() {
            assert!(
                memory_mount(b"30 5 0:22 / /tmp/cpu rw - cgroup memory rw,cpu", false).is_none()
            );
            assert_eq!(
                mount_path(b"/tmp/\xff\\040a\\134b\\011c\\012d")
                    .as_os_str()
                    .as_bytes(),
                b"/tmp/\xff a\\b\tc\nd"
            );
        }
    }
}

#[cfg(target_os = "freebsd")]
pub(super) fn available_bytes() -> Option<u64> {
    use std::{ffi::CStr, ptr};
    fn counter(name: &CStr) -> Option<u64> {
        let mut value = [0u8; 8];
        let mut len = value.len();
        // SAFETY: the kernel may write at most len initialized bytes; no value is set.
        let result = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                &mut len,
                ptr::null(),
                0,
            )
        };
        if result != 0 {
            return None;
        }
        match len {
            4 => Some(u32::from_ne_bytes(value[..4].try_into().ok()?) as u64),
            8 => Some(u64::from_ne_bytes(value)),
            _ => None,
        }
    }
    let pages = counter(c"vm.stats.vm.v_free_count")?
        .checked_add(counter(c"vm.stats.vm.v_inactive_count").unwrap_or(0))?;
    pages.checked_mul(counter(c"vm.stats.vm.v_page_size")?)
}

#[cfg(target_os = "macos")]
pub(super) fn available_bytes() -> Option<u64> {
    extern "C" {
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }
    // SAFETY: the Mach call writes into its matching structure with the size
    // expressed in Mach integers. Release the host send right before returning.
    unsafe {
        let host = libc::mach_host_self();
        let mut stats = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let result = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        );
        mach_port_deallocate(libc::mach_task_self(), host);
        if result != libc::KERN_SUCCESS || count < libc::HOST_VM_INFO64_COUNT {
            return None;
        }
        let stats = stats.assume_init();
        let pages = u64::from(stats.free_count) + u64::from(stats.inactive_count);
        let page_size = u64::try_from(libc::sysconf(libc::_SC_PAGESIZE)).ok()?;
        pages.checked_mul(page_size)
    }
}

#[cfg(windows)]
pub(super) fn available_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: GlobalMemoryStatusEx receives a writable, correctly sized structure.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullAvailPhys)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    windows
)))]
pub(super) fn available_bytes() -> Option<u64> {
    None
}
