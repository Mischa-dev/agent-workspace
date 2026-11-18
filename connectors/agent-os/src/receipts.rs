//! Receipts: what the connector did for each start, written before and
//! after every Agent OS call, so a lost answer or a restart never submits a
//! second task for the same run.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub start_key: String,
    pub run_id: String,
    pub host_agent_ref: String,
    /// The start command, fetched again after a restart for a fresh
    /// credential to report with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    /// The `task-submit` call, kept whole so a retry sends the very same
    /// one (same idempotency key, same source revision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submission: Option<Value>,
    /// Agent OS's task, as last seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Value>,
    /// How it ended (`done`, `passed`, `failed`, `cancelled`, `refused`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<String>,
}

impl Receipt {
    /// Agent OS's id for the task, once it took it.
    pub fn task_id(&self) -> Option<&str> {
        self.task.as_ref()?.get("id")?.as_str()
    }

    /// Agent OS took the task for certain: it has a runtime run.
    pub fn accepted(&self) -> bool {
        self.task
            .as_ref()
            .and_then(|task| task.get("runtime_run_id"))
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
    }
}

pub struct Receipts {
    dir: PathBuf,
}

impl Receipts {
    pub fn open(state_dir: &Path) -> std::io::Result<Self> {
        let dir = state_dir.join("receipts");
        create_private_dir(&dir)?;
        Ok(Self { dir })
    }

    fn path(&self, start_key: &str) -> PathBuf {
        let safe: String = start_key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    pub fn load(&self, start_key: &str) -> Option<Receipt> {
        let text = fs::read_to_string(self.path(start_key)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, receipt: &Receipt) -> std::io::Result<()> {
        let value = serde_json::to_value(receipt).map_err(std::io::Error::other)?;
        write_private(&self.path(&receipt.start_key), &value)
    }

    /// Receipts of tasks not yet finished, as after a restart.
    pub fn unfinished(&self) -> Vec<Receipt> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut found: Vec<Receipt> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<Receipt>(&text).ok())
            .filter(|receipt| receipt.finished.is_none())
            .collect();
        found.sort_by(|a, b| a.start_key.cmp(&b.start_key));
        found
    }
}

pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes a file only its owner can read, whole or not at all.
pub fn write_private(path: &Path, value: &Value) -> std::io::Result<()> {
    write_private_bytes(
        path,
        &serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?,
    )
}

pub fn write_private_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::now_v7().simple()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}
