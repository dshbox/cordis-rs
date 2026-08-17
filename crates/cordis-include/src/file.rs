//! Config files: format detection, ordered round-trips, atomic writes.

use crate::error::{IncludeError, Result};
use crate::lock;
use crate::node::Node;
use crate::options::EntryOptions;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The serialization format of a [`LoaderFile`], picked from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// `.yml` / `.yaml`
    Yaml,
    /// `.json`
    Json,
}

/// The parsed content of one config file: the entry list plus any unknown
/// top-level keys, which are preserved on write-back.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Document {
    /// The entry tree serialized as a list, in file order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<EntryOptions>,
    /// Unknown top-level keys, round-tripped untouched.
    #[serde(flatten, default, skip_serializing_if = "IndexMap::is_empty")]
    pub extra: IndexMap<String, Node>,
}

impl Document {
    /// A document holding just the given entries.
    pub fn with_entries(entries: Vec<EntryOptions>) -> Self {
        Self {
            entries,
            extra: IndexMap::new(),
        }
    }
}

/// Shared state behind a [`LoaderFile`] handle.
struct FileInner {
    path: PathBuf,
    format: FileFormat,
    suspend: Mutex<usize>,
}

/// A handle to one config file on disk.
///
/// Handles are cheap to clone and share path, format, and suspend state, so
/// several trees (or the loader and the watcher) can coordinate writes
/// through the same file. While any [`FileSuspendGuard`] is held,
/// [`LoaderFile::write`] is a silent no-op — the file-level half of breaking
/// the write → watch → write feedback loop.
#[derive(Clone)]
pub struct LoaderFile {
    inner: Arc<FileInner>,
}

impl std::fmt::Debug for LoaderFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoaderFile")
            .field("path", &self.inner.path)
            .field("format", &self.inner.format)
            .finish_non_exhaustive()
    }
}

impl LoaderFile {
    /// Open a `.yml`, `.yaml`, or `.json` config file. The file does not
    /// have to exist yet; [`LoaderFile::read`] returns an empty document
    /// until the first write creates it.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let format = match path.extension().and_then(|ext| ext.to_str()) {
            Some("yml" | "yaml") => FileFormat::Yaml,
            Some("json") => FileFormat::Json,
            _ => return Err(IncludeError::UnknownFormat { path }),
        };
        Ok(Self {
            inner: Arc::new(FileInner {
                path,
                format,
                suspend: Mutex::new(0),
            }),
        })
    }

    /// The file path.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// The detected format.
    pub fn format(&self) -> FileFormat {
        self.inner.format
    }

    /// Read and parse the file. Missing and empty files yield an empty
    /// document; `${{ ... }}` templates are *not* expanded here — entries
    /// keep their raw config.
    pub fn read(&self) -> Result<Document> {
        let content = match fs::read_to_string(&self.inner.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Document::default());
            }
            Err(error) => return Err(error.into()),
        };
        if content.trim().is_empty() {
            return Ok(Document::default());
        }
        match self.inner.format {
            FileFormat::Yaml => {
                let value =
                    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content).map_err(|error| {
                        IncludeError::Parse {
                            format: "yaml",
                            source: Box::new(error),
                        }
                    })?;
                if value.is_null() {
                    return Ok(Document::default());
                }
                serde_yaml_ng::from_value(value).map_err(|error| IncludeError::Parse {
                    format: "yaml",
                    source: Box::new(error),
                })
            }
            FileFormat::Json => {
                let value =
                    serde_json::from_str::<serde_json::Value>(&content).map_err(|error| {
                        IncludeError::Parse {
                            format: "json",
                            source: Box::new(error),
                        }
                    })?;
                if value.is_null() {
                    return Ok(Document::default());
                }
                serde_json::from_value(value).map_err(|error| IncludeError::Parse {
                    format: "json",
                    source: Box::new(error),
                })
            }
        }
    }

    /// Serialize the document and replace the file atomically (write to a
    /// sibling `.tmp` file, fsync, rename). A no-op while suspended.
    pub fn write(&self, document: &Document) -> Result<()> {
        if self.is_suspended() {
            return Ok(());
        }
        if let Ok(metadata) = fs::metadata(&self.inner.path) {
            if metadata.permissions().readonly() {
                return Err(IncludeError::ReadOnly {
                    path: self.inner.path.clone(),
                });
            }
        }
        let content = match self.inner.format {
            FileFormat::Yaml => {
                serde_yaml_ng::to_string(document).map_err(|error| IncludeError::Parse {
                    format: "yaml",
                    source: Box::new(error),
                })?
            }
            FileFormat::Json => {
                let mut text = serde_json::to_string_pretty(document).map_err(|error| {
                    IncludeError::Parse {
                        format: "json",
                        source: Box::new(error),
                    }
                })?;
                text.push('\n');
                text
            }
        };
        if let Some(parent) = self.inner.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let tmp = self.tmp_path();
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.inner.path)?;
        Ok(())
    }

    /// Increment the suspend counter, returning a guard whose drop resumes
    /// writes. Hold this while reloading a file so the resulting tree
    /// patches are not written back.
    pub fn suspend(&self) -> FileSuspendGuard {
        {
            let mut suspend = lock(&self.inner.suspend);
            *suspend += 1;
        }
        FileSuspendGuard { file: self.clone() }
    }

    /// Whether any suspend guard is currently held for this file.
    pub fn is_suspended(&self) -> bool {
        *lock(&self.inner.suspend) > 0
    }

    fn tmp_path(&self) -> PathBuf {
        let file_name = self
            .inner
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.inner.path.with_file_name(format!("{file_name}.tmp"))
    }
}

/// RAII guard for the file-level suspend counter.
#[derive(Debug)]
pub struct FileSuspendGuard {
    file: LoaderFile,
}

impl Drop for FileSuspendGuard {
    fn drop(&mut self) {
        let mut suspend = lock(&self.file.inner.suspend);
        *suspend = suspend.saturating_sub(1);
    }
}
