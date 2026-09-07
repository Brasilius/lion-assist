//! All application persistence goes through this serialized, fail-closed store.
//! An OS quota is still required for a strict whole-installation physical disk cap.
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn read_bounded(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut data)?;
    ensure!(data.len() as u64 <= limit, "input exceeds {limit} bytes");
    Ok(data)
}
pub fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "input must be a regular file"
    );
    read_bounded(File::open(path)?, limit)
}

pub struct Store {
    root: PathBuf,
    budget: u64,
    _lock: File,
}
impl Drop for Store {
    fn drop(&mut self) {
        // Explicit unlock also releases flock if a concurrently spawned child
        // briefly inherited a descriptor before exec closed it.
        let _ = FileExt::unlock(&self._lock);
    }
}
impl Store {
    pub fn open(root: &Path, total: u64, reserve: u64) -> Result<Self> {
        ensure!(
            total <= 50_000_000_000 && reserve < total,
            "invalid storage budget"
        );
        if !root.exists() {
            fs::create_dir_all(root)?;
        }
        ensure!(
            !fs::symlink_metadata(root)?.file_type().is_symlink(),
            "data directory cannot be a symlink"
        );
        #[cfg(unix)]
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        let root = root.canonicalize()?;
        let lock_path = root.join(".lock");
        if let Ok(meta) = fs::symlink_metadata(&lock_path) {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "invalid lock file"
            );
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock_exclusive()
            .context("data directory is already in use")?;
        let store = Self {
            root,
            budget: total - reserve,
            _lock: lock,
        };
        // Count orphaned temporary files as well; never delete data to fit the cap.
        ensure!(
            store.used()? <= store.budget,
            "existing data exceeds storage budget"
        );
        Ok(store)
    }
    pub fn used(&self) -> Result<u64> {
        usage(&self.root)
    }
    pub fn budget(&self) -> u64 {
        self.budget
    }
    fn path(&self, name: &str) -> Result<PathBuf> {
        ensure!(
            !name.is_empty()
                && !name.starts_with('.')
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
            "invalid object name"
        );
        Ok(self.root.join(name))
    }
    pub fn contains(&self, name: &str) -> Result<bool> {
        Ok(self.path(name)?.try_exists()?)
    }
    pub fn get(&self, name: &str, limit: u64) -> Result<Vec<u8>> {
        read_file(&self.path(name)?, limit)
    }
    /// Immutable objects make retries idempotent and avoid an unbounded mutable index.
    pub fn put(&mut self, name: &str, bytes: &[u8]) -> Result<bool> {
        let path = self.path(name)?;
        if path.try_exists()? {
            return Ok(false);
        }
        let additional = (bytes.len() as u64).div_ceil(4096) * 4096 + 8192;
        ensure!(
            self.used()?
                .checked_add(additional)
                .is_some_and(|n| n <= self.budget),
            "storage quota exhausted; object was not written"
        );
        let temporary = self.root.join(format!(".{name}.part"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).context(
            "cannot create staging file (a previous interrupted write may need inspection)",
        )?;
        let result = (|| -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            ensure!(
                self.used()? <= self.budget,
                "physical allocation exceeded application budget"
            );
            fs::rename(&temporary, path)?;
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result?;
        Ok(true)
    }
    pub fn list(&self, prefix: &str, max: usize) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix) {
                names.push(name);
            }
            if names.len() >= max {
                break;
            }
        }
        names.sort();
        Ok(names)
    }
}
fn usage(path: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        !meta.file_type().is_symlink(),
        "symlinks are forbidden in managed storage"
    );
    #[cfg(unix)]
    let mut total = meta.len().max(meta.blocks().saturating_mul(512));
    #[cfg(not(unix))]
    let mut total = meta.len();
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            total = total
                .checked_add(usage(&entry?.path())?)
                .context("storage size overflow")?;
        }
    } else if !meta.is_file() {
        bail!("special files are forbidden in managed storage");
    }
    Ok(total)
}
