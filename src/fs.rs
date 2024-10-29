// MIT License
//
// Copyright (c) 2023 Astral Software Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use fs2::FileExt;
use tempfile::NamedTempFile;
use tracing::{debug, error, info, trace};

pub static CWD: LazyLock<PathBuf> =
    LazyLock::new(|| std::env::current_dir().expect("The current directory must be exist"));

/// A file lock that is automatically released when dropped.
#[derive(Debug)]
pub struct LockedFile(fs_err::File);

impl LockedFile {
    /// Inner implementation for [`LockedFile::acquire_blocking`] and [`LockedFile::acquire`].
    fn lock_file_blocking(file: fs_err::File, resource: &str) -> Result<Self, std::io::Error> {
        trace!(
            "Checking lock for `{resource}` at `{}`",
            file.path().display(),
        );
        match file.file().try_lock_exclusive() {
            Ok(()) => {
                debug!("Acquired lock for `{resource}`");
                Ok(Self(file))
            }
            Err(err) => {
                // Log error code and enum kind to help debugging more exotic failures
                if err.kind() != std::io::ErrorKind::WouldBlock {
                    trace!("Try lock error: {err:?}");
                }
                info!(
                    "Waiting to acquire lock for `{resource}` at `{}`",
                    file.path().display(),
                );
                file.file().lock_exclusive().map_err(|err| {
                    // Not an fs_err method, we need to build our own path context
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!(
                            "Could not acquire lock for `{resource}` at `{}`: {}",
                            file.path().display(),
                            err
                        ),
                    )
                })?;

                debug!("Acquired lock for `{resource}`");
                Ok(Self(file))
            }
        }
    }

    /// The same as [`LockedFile::acquire`], but for synchronous contexts. Do not use from an async
    /// context, as this can block the runtime while waiting for another process to release the
    /// lock.
    pub fn acquire_blocking(
        path: impl AsRef<Path>,
        resource: impl Display,
    ) -> Result<Self, std::io::Error> {
        let file = fs_err::File::create(path.as_ref())?;
        let resource = resource.to_string();
        Self::lock_file_blocking(file, &resource)
    }

    /// Acquire a cross-process lock for a resource using a file at the provided path.
    pub async fn acquire(
        path: impl AsRef<Path>,
        resource: impl Display,
    ) -> Result<Self, std::io::Error> {
        let file = fs_err::File::create(path.as_ref())?;
        let resource = resource.to_string();
        tokio::task::spawn_blocking(move || Self::lock_file_blocking(file, &resource)).await?
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        if let Err(err) = self.0.file().unlock() {
            error!(
                "Failed to unlock {}; program may be stuck: {}",
                self.0.path().display(),
                err
            );
        } else {
            debug!("Released lock at `{}`", self.0.path().display());
        }
    }
}

/// Return a [`NamedTempFile`] in the specified directory.
///
/// Sets the permissions of the temporary file to `0o666`, to match the non-temporary file default.
/// ([`NamedTempfile`] defaults to `0o600`.)
#[cfg(unix)]
pub fn tempfile_in(path: &Path) -> std::io::Result<NamedTempFile> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o666))
        .tempfile_in(path)
}

/// Return a [`NamedTempFile`] in the specified directory.
#[cfg(not(unix))]
pub fn tempfile_in(path: &Path) -> std::io::Result<NamedTempFile> {
    tempfile::Builder::new().tempfile_in(path)
}

/// Write `data` to `path` atomically using a temporary file and atomic rename.
pub fn write_atomic(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    let temp_file = tempfile_in(
        path.as_ref()
            .parent()
            .expect("Write path must have a parent"),
    )?;
    fs_err::write(&temp_file, &data)?;
    temp_file.persist(&path).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "Failed to persist temporary file to {}: {}",
                path.as_ref().display(),
                err.error
            ),
        )
    })?;
    Ok(())
}

/// Copy `from` to `to` atomically using a temporary file and atomic rename.
pub fn copy_atomic(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<()> {
    let temp_file = tempfile_in(to.as_ref().parent().expect("Write path must have a parent"))?;
    fs_err::copy(from.as_ref(), &temp_file)?;
    temp_file.persist(&to).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "Failed to persist temporary file to {}: {}",
                to.as_ref().display(),
                err.error
            ),
        )
    })?;
    Ok(())
}

/// Recursively copy a directory and its contents.
pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs_err::create_dir_all(&dst)?;
    for entry in fs_err::read_dir(src.as_ref())? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs_err::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Normalizes a path to use `/` as a separator everywhere, even on platforms
/// that recognize other characters as separators.
#[cfg(unix)]
pub(crate) fn normalize_path(path: String) -> String {
    // UNIX only uses /, so we're good.
    path
}

/// Normalizes a path to use `/` as a separator everywhere, even on platforms
/// that recognize other characters as separators.
#[cfg(not(unix))]
pub(crate) fn normalize_path(mut path: String) -> String {
    use std::path::is_separator;

    let mut bytes = std::mem::take(&mut path).into_bytes();
    for c in &mut bytes {
        if *c == b'/' || !is_separator(char::from(*c)) {
            continue;
        }
        *c = b'/';
    }
    String::from_utf8(bytes).expect("Path is valid UTF-8")
}

/// Compute a path describing `path` relative to `base`.
///
/// `lib/python/site-packages/foo/__init__.py` and `lib/python/site-packages` -> `foo/__init__.py`
/// `lib/marker.txt` and `lib/python/site-packages` -> `../../marker.txt`
/// `bin/foo_launcher` and `lib/python/site-packages` -> `../../../bin/foo_launcher`
///
/// Returns `Err` if there is no relative path between `path` and `base` (for example, if the paths
/// are on different drives on Windows).
pub fn relative_to(
    path: impl AsRef<Path>,
    base: impl AsRef<Path>,
) -> Result<PathBuf, std::io::Error> {
    // Find the longest common prefix, and also return the path stripped from that prefix
    let (stripped, common_prefix) = base
        .as_ref()
        .ancestors()
        .find_map(|ancestor| {
            // Simplifying removes the UNC path prefix on windows.
            dunce::simplified(path.as_ref())
                .strip_prefix(dunce::simplified(ancestor))
                .ok()
                .map(|stripped| (stripped, ancestor))
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "Trivial strip failed: {} vs. {}",
                    path.as_ref().display(),
                    base.as_ref().display()
                ),
            )
        })?;

    // go as many levels up as required
    let levels_up = base.as_ref().components().count() - common_prefix.components().count();
    let up = std::iter::repeat("..").take(levels_up).collect::<PathBuf>();

    Ok(up.join(stripped))
}
