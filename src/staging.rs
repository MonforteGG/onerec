use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TAKE: AtomicU64 = AtomicU64::new(0);

pub struct StagingArea {
    dir: PathBuf,
}

#[derive(Debug)]
pub struct StagingFile {
    path: PathBuf,
    unlinked: bool,
}

impl StagingArea {
    pub fn open() -> io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("onerec-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn next_take(&self) -> io::Result<StagingFile> {
        loop {
            let n = NEXT_TAKE.fetch_add(1, Ordering::Relaxed);
            let path = self.dir.join(format!("take-{n}.f32"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(StagingFile::reserved(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

impl StagingFile {
    pub(crate) fn reserved(path: PathBuf) -> Self {
        Self {
            path,
            unlinked: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn unlink(&mut self) -> io::Result<()> {
        if self.unlinked {
            return Ok(());
        }
        fs::remove_file(&self.path)?;
        self.unlinked = true;
        Ok(())
    }

    pub(crate) fn take(&mut self) -> Self {
        std::mem::replace(
            self,
            Self {
                path: PathBuf::new(),
                unlinked: true,
            },
        )
    }
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        if !self.unlinked {
            let _ = fs::remove_file(&self.path);
            self.unlinked = true;
        }
    }
}
