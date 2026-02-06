//! Job tracking for async generation tasks
//!
//! Jobs are persisted as `.job.toml` files in `.flint/jobs/` so they
//! survive process restarts.

use flint_core::{FlintError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Status of a generation job
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Submitted,
    Processing,
    Complete,
    Failed,
}

/// A tracked generation job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationJob {
    /// Unique job ID (UUID)
    pub id: String,
    /// Provider-specific remote job ID (if any)
    #[serde(default)]
    pub remote_id: Option<String>,
    /// Provider name
    pub provider: String,
    /// Asset name being generated
    pub asset_name: String,
    /// Current status
    pub status: JobStatus,
    /// Progress percentage (0-100)
    #[serde(default)]
    pub progress: u8,
    /// ISO 8601 timestamp when submitted
    pub submitted_at: String,
    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
    /// The prompt that was used
    #[serde(default)]
    pub prompt: Option<String>,
    /// Output path once complete
    #[serde(default)]
    pub output_path: Option<String>,
}

impl GenerationJob {
    /// Create a new job
    pub fn new(provider: &str, asset_name: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            remote_id: None,
            provider: provider.to_string(),
            asset_name: asset_name.to_string(),
            status: JobStatus::Submitted,
            progress: 0,
            submitted_at: now_iso8601(),
            error: None,
            prompt: None,
            output_path: None,
        }
    }
}

/// File-based job store in `.flint/jobs/`
pub struct JobStore {
    root: PathBuf,
}

impl JobStore {
    /// Create a new job store at the given root directory
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    /// Default job store location
    pub fn default_store() -> Self {
        Self::new(".flint/jobs")
    }

    /// Save a job to disk
    pub fn save(&self, job: &GenerationJob) -> Result<()> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{}.job.toml", job.id));

        let wrapper = JobFile { job: job.clone() };
        let content = toml::to_string_pretty(&wrapper).map_err(|e| {
            FlintError::GenerationError(format!("Failed to serialize job: {}", e))
        })?;

        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Load a job by ID
    pub fn load(&self, job_id: &str) -> Result<GenerationJob> {
        let path = self.root.join(format!("{}.job.toml", job_id));
        if !path.exists() {
            return Err(FlintError::GenerationError(format!(
                "Job not found: {}",
                job_id
            )));
        }

        let content = std::fs::read_to_string(&path)?;
        let file: JobFile = toml::from_str(&content).map_err(|e| {
            FlintError::GenerationError(format!("Failed to parse job file: {}", e))
        })?;
        Ok(file.job)
    }

    /// List all tracked jobs
    pub fn list(&self) -> Result<Vec<GenerationJob>> {
        let mut jobs = Vec::new();

        if !self.root.exists() {
            return Ok(jobs);
        }

        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".job.toml"))
                .unwrap_or(false)
            {
                let content = std::fs::read_to_string(&path)?;
                if let Ok(file) = toml::from_str::<JobFile>(&content) {
                    jobs.push(file.job);
                }
            }
        }

        Ok(jobs)
    }

    /// Update a job's status and save
    pub fn update_status(
        &self,
        job_id: &str,
        status: JobStatus,
        progress: u8,
    ) -> Result<GenerationJob> {
        let mut job = self.load(job_id)?;
        job.status = status;
        job.progress = progress;
        self.save(&job)?;
        Ok(job)
    }
}

#[derive(Serialize, Deserialize)]
struct JobFile {
    job: GenerationJob,
}

fn now_iso8601() -> String {
    // Simple UTC timestamp without external chrono dependency
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Approximate breakdown — sufficient for job tracking
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    // Days since epoch to approximate date
    let mut y = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md as i64 {
            m = i;
            break;
        }
        remaining_days -= md as i64;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining_days + 1,
        hours,
        mins,
        s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_job_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_job_creation() {
        let job = GenerationJob::new("flux", "brick_wall");
        assert_eq!(job.provider, "flux");
        assert_eq!(job.asset_name, "brick_wall");
        assert_eq!(job.status, JobStatus::Submitted);
        assert_eq!(job.progress, 0);
        assert!(!job.id.is_empty());
        assert!(job.submitted_at.contains('T'));
    }

    #[test]
    fn test_job_serialize_roundtrip() {
        let job = GenerationJob::new("meshy", "tavern_chair");
        let wrapper = JobFile { job: job.clone() };
        let toml_str = toml::to_string_pretty(&wrapper).unwrap();
        let parsed: JobFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.job.id, job.id);
        assert_eq!(parsed.job.provider, "meshy");
        assert_eq!(parsed.job.asset_name, "tavern_chair");
    }

    #[test]
    fn test_job_store_save_load() {
        let dir = temp_dir();
        let store = JobStore::new(&dir);

        let job = GenerationJob::new("flux", "stone_texture");
        store.save(&job).unwrap();

        let loaded = store.load(&job.id).unwrap();
        assert_eq!(loaded.id, job.id);
        assert_eq!(loaded.provider, "flux");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_job_store_list() {
        let dir = temp_dir();
        let store = JobStore::new(&dir);

        let job1 = GenerationJob::new("flux", "texture_a");
        let job2 = GenerationJob::new("meshy", "model_b");
        store.save(&job1).unwrap();
        store.save(&job2).unwrap();

        let jobs = store.list().unwrap();
        assert_eq!(jobs.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_job_store_update_status() {
        let dir = temp_dir();
        let store = JobStore::new(&dir);

        let job = GenerationJob::new("meshy", "chair");
        store.save(&job).unwrap();

        let updated = store
            .update_status(&job.id, JobStatus::Processing, 50)
            .unwrap();
        assert_eq!(updated.status, JobStatus::Processing);
        assert_eq!(updated.progress, 50);

        // Verify persistence
        let reloaded = store.load(&job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Processing);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_job_not_found() {
        let dir = temp_dir();
        let store = JobStore::new(&dir);
        let result = store.load("nonexistent-id");
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
