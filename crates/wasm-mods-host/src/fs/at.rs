//! Descriptor-relative access: every path is walked from its root's descriptor with
//! `openat` and no symlinks followed, so nothing outside a grant is reachable.

use super::*;

fn cstr(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

fn openat(dir: RawFd, name: &OsStr, flags: libc::c_int, mode: libc::mode_t) -> io::Result<OwnedFd> {
    let name = cstr(name)?;
    // SAFETY: a valid C string; the result is a new descriptor we own.
    let fd = unsafe { libc::openat(dir, name.as_ptr(), flags | libc::O_CLOEXEC, mode as libc::c_uint) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

fn fstatat(dir: RawFd, name: &OsStr) -> io::Result<libc::stat> {
    let name = cstr(name)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers.
    let r = unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(st) }
}

fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointer.
    if unsafe { libc::fstat(fd, &mut st) } < 0 { Err(io::Error::last_os_error()) } else { Ok(st) }
}

fn kind_of(st: &libc::stat) -> FsKind {
    match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => FsKind::File,
        libc::S_IFDIR => FsKind::Dir,
        libc::S_IFLNK => FsKind::Symlink,
        _ => FsKind::Other,
    }
}

pub fn stat_of(st: &libc::stat) -> FsStat {
    let modified = st.st_mtime as f64 + st.st_mtime_nsec as f64 / 1e9;
    FsStat { kind: kind_of(st), size: st.st_size.max(0) as u64, modified: Some(modified) }
}

/// An error in plain words, naming `path`. A component that is a symlink says so.
fn explain(e: &io::Error, path: &str, symlink: bool) -> String {
    if symlink {
        return format!("not allowed: {path} goes through a symbolic link (WASM mods never follow them)");
    }
    match e.kind() {
        io::ErrorKind::NotFound => format!("no such file or folder: {path}"),
        io::ErrorKind::PermissionDenied => format!("permission denied: {path}"),
        io::ErrorKind::AlreadyExists => format!("already exists: {path}"),
        _ => format!("{path}: {e}"),
    }
}

fn is_symlink_at(dir: RawFd, name: &OsStr) -> bool {
    fstatat(dir, name).is_ok_and(|st| kind_of(&st) == FsKind::Symlink)
}

/// The root folder, opened (following links in its own path: the folder allowed).
fn open_root(root: &Root) -> io::Result<OwnedFd> {
    let dir = cstr(root.dir.as_os_str())?;
    // SAFETY: a valid C string.
    let fd = unsafe { libc::open(dir.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

/// Walks `names` down from `from`, each a real folder (no symlinks).
fn walk_dirs(from: OwnedFd, names: &[OsString], path: &str) -> Result<OwnedFd, String> {
    let mut dir = from;
    for name in names {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;
        dir = openat(dir.as_raw_fd(), name, flags, 0)
            .map_err(|e| explain(&e, path, is_symlink_at(dir.as_raw_fd(), name)))?;
    }
    Ok(dir)
}

/// The folder holding the last component, and that component's name.
fn parent_of(at: &Located, path: &str) -> Result<(OwnedFd, OsString), String> {
    let (name, parents) = at.rel.split_last().ok_or_else(|| format!("{path} is the granted folder itself"))?;
    let root = open_root(&at.root).map_err(|e| explain(&e, path, false))?;
    Ok((walk_dirs(root, parents, path)?, name.clone()))
}

/// The folder `at` names (the root itself when `rel` is empty).
pub fn open_dir_at(at: &Located, path: &str) -> Result<OwnedFd, String> {
    let root = open_root(&at.root).map_err(|e| explain(&e, path, false))?;
    walk_dirs(root, &at.rel, path)
}

/// Opens the regular file `at` names for reading.
pub fn open_file(at: &Located, path: &str) -> Result<File, String> {
    if at.rel.is_empty() {
        return Err(format!("{path} is a folder"));
    }
    let (dir, name) = parent_of(at, path)?;
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY;
    let fd = openat(dir.as_raw_fd(), &name, flags, 0)
        .map_err(|e| explain(&e, path, is_symlink_at(dir.as_raw_fd(), &name)))?;
    let st = fstat(fd.as_raw_fd()).map_err(|e| explain(&e, path, false))?;
    match kind_of(&st) {
        FsKind::File => Ok(File::from(fd)),
        FsKind::Dir => Err(format!("{path} is a folder")),
        _ => Err(format!("{path} is not a regular file")),
    }
}

pub fn stat(at: &Located, path: &str) -> Result<FsStat, String> {
    if at.rel.is_empty() {
        let root = open_root(&at.root).map_err(|e| explain(&e, path, false))?;
        return fstat(root.as_raw_fd()).map(|s| stat_of(&s)).map_err(|e| explain(&e, path, false));
    }
    let (dir, name) = parent_of(at, path)?;
    fstatat(dir.as_raw_fd(), &name).map(|s| stat_of(&s)).map_err(|e| explain(&e, path, false))
}

pub fn read_file(at: &Located, path: &str) -> Result<Vec<u8>, String> {
    let file = open_file(at, path)?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    if size > READ_FILE_MAX {
        return Err(format!(
            "{path} is too big to read whole ({size} bytes; at most {READ_FILE_MAX}): open it and read ranges"
        ));
    }
    let mut out = Vec::with_capacity(size as usize);
    (&file).take(READ_FILE_MAX + 1).read_to_end(&mut out).map_err(|e| explain(&e, path, false))?;
    if out.len() as u64 > READ_FILE_MAX {
        return Err(format!("{path} grew too big to read whole"));
    }
    Ok(out)
}

pub fn list(at: &Located, path: &str) -> Result<(Vec<DirEntry>, bool), String> {
    let dir = open_dir_at(at, path)?;
    // fdopendir takes over a descriptor of its own.
    // SAFETY: dup of a valid descriptor.
    let dup = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return Err(explain(&io::Error::last_os_error(), path, false));
    }
    // SAFETY: `dup` is ours; closedir closes it.
    let stream = unsafe { libc::fdopendir(dup) };
    if stream.is_null() {
        unsafe { libc::close(dup) };
        return Err(explain(&io::Error::last_os_error(), path, false));
    }
    let mut names = Vec::new();
    let mut truncated = false;
    loop {
        // SAFETY: a valid stream; the entry is read before the next call.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        let name = OsStr::from_bytes(name.to_bytes());
        if name == "." || name == ".." {
            continue;
        }
        if names.len() >= LIST_MAX {
            truncated = true;
            break;
        }
        names.push(name.to_owned());
    }
    unsafe { libc::closedir(stream) };
    names.sort();
    let entries = names
        .into_iter()
        .filter_map(|name| {
            let st = fstatat(dir.as_raw_fd(), &name).ok()?;
            Some(DirEntry { name: name.to_string_lossy().into_owned(), stat: stat_of(&st) })
        })
        .collect();
    Ok((entries, truncated))
}

/// Replaces the file `at` names with `data`, atomically.
pub fn write(at: &Located, path: &str, data: &[u8]) -> Result<(), String> {
    if data.len() > WRITE_MAX {
        return Err(format!("too big to write: {} bytes (at most {WRITE_MAX})", data.len()));
    }
    let (dir, name) = parent_of(at, path)?;
    let dfd = dir.as_raw_fd();
    let mut mode = 0o644;
    match fstatat(dfd, &name) {
        Ok(st) => match kind_of(&st) {
            FsKind::File => mode = st.st_mode & 0o777,
            FsKind::Symlink => return Err(explain(&io::ErrorKind::Other.into(), path, true)),
            FsKind::Dir => return Err(format!("{path} is a folder")),
            FsKind::Other => return Err(format!("{path} is not a regular file")),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(explain(&e, path, false)),
    }
    static TEMP: AtomicU64 = AtomicU64::new(0);
    let tmp = OsString::from(format!(".wasm-mods-{}-{}.tmp", std::process::id(), TEMP.fetch_add(1, Ordering::Relaxed)));
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
    let fd = openat(dfd, &tmp, flags, mode).map_err(|e| explain(&e, path, false))?;
    let unlink_tmp = || {
        if let Ok(t) = cstr(&tmp) {
            unsafe { libc::unlinkat(dfd, t.as_ptr(), 0) };
        }
    };
    let mut file = File::from(fd);
    if let Err(e) = file.write_all(data).and_then(|_| file.sync_all()) {
        unlink_tmp();
        return Err(explain(&e, path, false));
    }
    drop(file);
    let (from, to) = (cstr(&tmp).map_err(|e| e.to_string())?, cstr(&name).map_err(|e| e.to_string())?);
    // SAFETY: valid descriptors and C strings.
    if unsafe { libc::renameat(dfd, from.as_ptr(), dfd, to.as_ptr()) } < 0 {
        let e = io::Error::last_os_error();
        unlink_tmp();
        return Err(explain(&e, path, false));
    }
    unsafe { libc::fsync(dfd) };
    Ok(())
}

/// Makes the folder `at` names and any missing parents below the root (the root too,
/// when it is a granted folder).
pub fn create_dir(at: &Located, path: &str) -> Result<(), String> {
    if at.root.only.is_some() {
        return Err(format!("not allowed: {path} was picked as a file"));
    }
    let root = match open_root(&at.root) {
        Ok(fd) => fd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&at.root.dir).map_err(|e| explain(&e, path, false))?;
            open_root(&at.root).map_err(|e| explain(&e, path, false))?
        }
        Err(e) => return Err(explain(&e, path, false)),
    };
    let mut dir = root;
    for name in &at.rel {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;
        dir = match openat(dir.as_raw_fd(), name, flags, 0) {
            Ok(fd) => fd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let c = cstr(name).map_err(|e| e.to_string())?;
                // SAFETY: valid descriptor and C string.
                if unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o755) } < 0 {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::AlreadyExists {
                        return Err(explain(&e, path, false));
                    }
                }
                openat(dir.as_raw_fd(), name, flags, 0)
                    .map_err(|e| explain(&e, path, is_symlink_at(dir.as_raw_fd(), name)))?
            }
            Err(e) => return Err(explain(&e, path, is_symlink_at(dir.as_raw_fd(), name))),
        };
    }
    Ok(())
}

/// Deletes the file (or symlink, which is a file of its own here) or empty folder `at` names.
pub fn delete(at: &Located, path: &str) -> Result<(), String> {
    let (dir, name) = parent_of(at, path)?;
    let st = fstatat(dir.as_raw_fd(), &name).map_err(|e| explain(&e, path, false))?;
    let flags = if kind_of(&st) == FsKind::Dir { libc::AT_REMOVEDIR } else { 0 };
    let c = cstr(&name).map_err(|e| e.to_string())?;
    // SAFETY: valid descriptor and C string.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), flags) } < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOTEMPTY) {
            return Err(format!("{path} is a folder that isn't empty"));
        }
        return Err(explain(&e, path, false));
    }
    Ok(())
}
