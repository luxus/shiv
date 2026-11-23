use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` via a unique sibling temporary file, flushing and
/// renaming into place. On any error the temporary file is removed.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with_permissions(path, bytes, None)
}

/// Like [`write_atomic`], applying `permissions` to the temporary file before
/// renaming into place.
pub(crate) fn write_atomic_with_permissions(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(
        ".{name}.shiv-{}-{}.tmp",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| -> io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(perms) = permissions {
            fs::set_permissions(&tmp, perms)?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// True when `name` is a single safe path component (no separators, `..`,
/// `.`, NUL bytes, or control characters).
pub(crate) fn is_plain_filename(name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name.chars().any(char::is_control) {
        return false;
    }
    let mut comps = Path::new(name).components();
    matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(c)), None) if c == name
    )
}

/// Resolve `dir.join(name)` only if `name` is a single safe filename.
pub(crate) fn safe_child(dir: &Path, name: &str) -> Option<PathBuf> {
    is_plain_filename(name).then(|| dir.join(name))
}
