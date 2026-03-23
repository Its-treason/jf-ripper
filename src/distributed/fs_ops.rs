use std::fs;
use std::path::{Path, PathBuf};

use super::job::{DistributedJob, Heartbeat};

pub struct SharedDirs {
    pub root: PathBuf,
    pub pending: PathBuf,
    pub active: PathBuf,
    pub completed: PathBuf,
    pub failed: PathBuf,
    pub media: PathBuf,
}

impl SharedDirs {
    pub fn new(shared_dir: &str) -> Self {
        let root = PathBuf::from(shared_dir);
        Self {
            pending: root.join("pending"),
            active: root.join("active"),
            completed: root.join("completed"),
            failed: root.join("failed"),
            media: root.join("media"),
            root,
        }
    }

    pub fn ensure_dirs(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.pending)?;
        fs::create_dir_all(&self.active)?;
        fs::create_dir_all(&self.completed)?;
        fs::create_dir_all(&self.failed)?;
        fs::create_dir_all(&self.media)?;
        Ok(())
    }
}

fn job_filename(id: &uuid::Uuid) -> String {
    format!("{}.job.json", id)
}

fn heartbeat_filename(id: &uuid::Uuid) -> String {
    format!("{}.heartbeat", id)
}

pub fn write_job_atomic(
    dir: &Path,
    job: &DistributedJob,
) -> Result<(), Box<dyn std::error::Error>> {
    let filename = job_filename(&job.id);
    let tmp = dir.join(format!(".{}.tmp", filename));
    let final_path = dir.join(&filename);
    let json = serde_json::to_string_pretty(job)?;
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &final_path)?;
    Ok(())
}

/// Atomically claim a job by renaming from pending/ to active/.
/// Returns true if this worker successfully claimed it.
pub fn claim_job(dirs: &SharedDirs, job_id: &uuid::Uuid) -> bool {
    let src = dirs.pending.join(job_filename(job_id));
    let dst = dirs.active.join(job_filename(job_id));
    fs::rename(&src, &dst).is_ok()
}

pub fn complete_job(dirs: &SharedDirs, job_id: &uuid::Uuid) -> Result<(), std::io::Error> {
    let src = dirs.active.join(job_filename(job_id));
    let dst = dirs.completed.join(job_filename(job_id));
    fs::rename(&src, &dst)?;
    remove_heartbeat(dirs, job_id);
    Ok(())
}

pub fn fail_job(
    dirs: &SharedDirs,
    job: &mut DistributedJob,
    error: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    job.error_message = Some(error.to_string());
    job.attempt += 1;

    if job.attempt >= job.max_retries {
        // Move to failed
        write_job_atomic(&dirs.failed, job)?;
        let active = dirs.active.join(job_filename(&job.id));
        let _ = fs::remove_file(&active);
    } else {
        // Requeue to pending
        requeue_job(dirs, job)?;
    }
    remove_heartbeat(dirs, &job.id);
    Ok(())
}

pub fn requeue_job(
    dirs: &SharedDirs,
    job: &DistributedJob,
) -> Result<(), Box<dyn std::error::Error>> {
    write_job_atomic(&dirs.pending, job)?;
    let active = dirs.active.join(job_filename(&job.id));
    let _ = fs::remove_file(&active);
    remove_heartbeat(dirs, &job.id);
    Ok(())
}

pub fn write_heartbeat(
    dirs: &SharedDirs,
    job_id: &uuid::Uuid,
    hb: &Heartbeat,
) -> Result<(), Box<dyn std::error::Error>> {
    let filename = heartbeat_filename(job_id);
    let tmp = dirs.active.join(format!(".{}.tmp", filename));
    let final_path = dirs.active.join(&filename);
    let json = serde_json::to_string(hb)?;
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, &final_path)?;
    Ok(())
}

pub fn read_heartbeat(dirs: &SharedDirs, job_id: &uuid::Uuid) -> Option<Heartbeat> {
    let path = dirs.active.join(heartbeat_filename(job_id));
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn remove_heartbeat(dirs: &SharedDirs, job_id: &uuid::Uuid) {
    let path = dirs.active.join(heartbeat_filename(job_id));
    let _ = fs::remove_file(&path);
}

pub fn is_heartbeat_stale(dirs: &SharedDirs, job_id: &uuid::Uuid, timeout_secs: u64) -> bool {
    match read_heartbeat(dirs, job_id) {
        Some(hb) => {
            let age = chrono::Utc::now().signed_duration_since(hb.timestamp);
            age.num_seconds() > timeout_secs as i64
        }
        None => true, // No heartbeat = stale
    }
}

pub fn list_jobs_in(dir: &Path) -> Vec<DistributedJob> {
    let mut jobs = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return jobs,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(job) = serde_json::from_str::<DistributedJob>(&data) {
                    jobs.push(job);
                }
            }
        }
    }
    jobs
}

pub fn maybe_cleanup_media(dirs: &SharedDirs, media_file: &str) -> Result<(), std::io::Error> {
    // Check if any pending or active jobs reference this media file
    let pending = list_jobs_in(&dirs.pending);
    let active = list_jobs_in(&dirs.active);
    let referenced = pending
        .iter()
        .chain(active.iter())
        .any(|j| j.media_file == media_file);

    if !referenced {
        let path = dirs.media.join(media_file);
        if path.exists() {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}
