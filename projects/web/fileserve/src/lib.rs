use std::path::{Path, PathBuf};
use tokio::fs;
use std::io;

#[derive(Clone)]
pub struct FileServer {
    pub root: PathBuf,
}

impl FileServer {
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub async fn list_dir(&self, relative_path: &str) -> io::Result<Vec<DirEntry>> {
        let path = self.root.join(relative_path);
        if !path.starts_with(&self.root) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Path traversal"));
        }

        let mut entries = Vec::new();
        let mut read_dir = fs::read_dir(&path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.len() };

            entries.push(DirEntry {
                name: file_name,
                is_dir,
                size,
            });
        }

        entries.sort_by(|a, b| {
            if a.is_dir == b.is_dir {
                a.name.cmp(&b.name)
            } else {
                b.is_dir.cmp(&a.is_dir)
            }
        });

        Ok(entries)
    }

    pub async fn read_file(&self, relative_path: &str) -> io::Result<Vec<u8>> {
        let path = self.root.join(relative_path);
        if !path.starts_with(&self.root) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Path traversal"));
        }

        fs::read(&path).await
    }

    pub async fn write_file(&self, relative_path: &str, data: &[u8]) -> io::Result<()> {
        let path = self.root.join(relative_path);
        if !path.starts_with(&self.root) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Path traversal"));
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(&path, data).await
    }

    pub async fn file_exists(&self, relative_path: &str) -> bool {
        let path = self.root.join(relative_path);
        if !path.starts_with(&self.root) {
            return false;
        }

        fs::try_exists(&path).await.unwrap_or(false)
    }
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}
