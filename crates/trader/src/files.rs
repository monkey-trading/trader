//! Files the bot writes for itself alone: the console's key, its login's hash, and the controls it
//! set. Each is written whole or not at all, readable only by the bot's user.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use anyhow::Context;

/// Creates `dir` and its missing parents, each readable only by the bot's user.
/// Error cases:
/// - a directory cannot be created
pub fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("creating {}", dir.display()))
}

/// Writes `bytes` to `path` readable only by the bot's user, through a temporary file that is
/// synced and renamed over it, so a crash leaves the old file or the new one, never part of one.
/// Error cases:
/// - the directory cannot be created, or a write, sync, or rename fails
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().with_context(|| format!("{} has no directory", path.display()))?;
    create_private_dir(dir)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = Path::new(&temporary);
    let write = || -> std::io::Result<()> {
        // Opened first, so a directory that cannot be opened fails before the file is touched.
        let directory = File::open(dir)?;
        let mut file = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, path)?;
        directory.sync_all()
    };
    write().with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A directory that cannot be opened for reading fails the write before the file is touched, so a write that
    /// reports failure has changed nothing.
    #[test]
    fn a_directory_that_cannot_be_opened_leaves_the_file_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        write_private(&path, b"old").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o300)).unwrap();
        let written = write_private(&path, b"new");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(written.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
    }

    /// The file holds what was written, readable by its owner alone, with no temporary left.
    #[test]
    fn writes_whole_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/secret");
        write_private(&path, b"first").unwrap();
        write_private(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        let names: Vec<_> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["secret"]);
    }
}
