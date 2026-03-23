use std::path::Path;

use crate::config::Config;

use super::fs_ops::{self, SharedDirs};

pub fn list_jobs(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let shared_dir = config
        .distributed
        .shared_dir
        .as_deref()
        .ok_or("distributed.shared_dir not configured")?;
    let dirs = SharedDirs::new(shared_dir);

    print_jobs_in("Pending", &dirs.pending, &dirs);
    print_jobs_in("Active", &dirs.active, &dirs);
    print_jobs_in("Completed", &dirs.completed, &dirs);
    print_jobs_in("Failed", &dirs.failed, &dirs);

    Ok(())
}

fn print_jobs_in(label: &str, dir: &Path, dirs: &SharedDirs) {
    let jobs = fs_ops::list_jobs_in(dir);
    if jobs.is_empty() {
        return;
    }
    println!("\n=== {} ({}) ===", label, jobs.len());
    for job in &jobs {
        let id_short = &job.id.to_string()[..8];
        println!(
            "  {} | {:?} | {} | attempt {}/{}",
            id_short,
            job.content_type,
            job.relative_output_path,
            job.attempt,
            job.max_retries
        );
        if label == "Active" {
            if let Some(hb) = fs_ops::read_heartbeat(dirs, &job.id) {
                println!("       heartbeat: {} @ {}", hb.worker_name, hb.timestamp);
            } else {
                println!("       heartbeat: MISSING");
            }
        }
        if let Some(err) = &job.error_message {
            println!("       error: {}", err);
        }
    }
}

pub fn retry_jobs(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let shared_dir = config
        .distributed
        .shared_dir
        .as_deref()
        .ok_or("distributed.shared_dir not configured")?;
    let dirs = SharedDirs::new(shared_dir);

    let failed = fs_ops::list_jobs_in(&dirs.failed);
    if failed.is_empty() {
        println!("No failed jobs to retry.");
        return Ok(());
    }

    let mut count = 0;
    for mut job in failed {
        job.attempt = 0;
        job.error_message = None;
        fs_ops::write_job_atomic(&dirs.pending, &job)?;
        let failed_path = dirs.failed.join(format!("{}.job.json", job.id));
        let _ = std::fs::remove_file(&failed_path);
        count += 1;
    }

    println!("Retried {} jobs.", count);
    Ok(())
}

pub fn clean_jobs(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let shared_dir = config
        .distributed
        .shared_dir
        .as_deref()
        .ok_or("distributed.shared_dir not configured")?;
    let dirs = SharedDirs::new(shared_dir);

    let completed = fs_ops::list_jobs_in(&dirs.completed);
    if completed.is_empty() {
        println!("No completed jobs to clean.");
        return Ok(());
    }

    let mut count = 0;
    for job in &completed {
        let path = dirs.completed.join(format!("{}.job.json", job.id));
        let _ = std::fs::remove_file(&path);
        if config.distributed.cleanup_raw_media {
            let _ = fs_ops::maybe_cleanup_media(&dirs, &job.media_file);
        }
        count += 1;
    }

    println!("Cleaned {} completed jobs.", count);
    Ok(())
}

pub fn recover_stale(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let shared_dir = config
        .distributed
        .shared_dir
        .as_deref()
        .ok_or("distributed.shared_dir not configured")?;
    let dirs = SharedDirs::new(shared_dir);

    let active = fs_ops::list_jobs_in(&dirs.active);
    if active.is_empty() {
        println!("No active jobs.");
        return Ok(());
    }

    let mut count = 0;
    for mut job in active {
        if fs_ops::is_heartbeat_stale(&dirs, &job.id, config.distributed.stale_lock_timeout_secs) {
            println!(
                "Recovering stale job {} ({})",
                &job.id.to_string()[..8],
                job.relative_output_path
            );
            job.attempt += 1;
            if job.attempt >= job.max_retries {
                job.error_message =
                    Some("Stale heartbeat - max retries exceeded".to_string());
                fs_ops::write_job_atomic(&dirs.failed, &job)?;
                let active_path = dirs.active.join(format!("{}.job.json", job.id));
                let _ = std::fs::remove_file(&active_path);
                fs_ops::remove_heartbeat(&dirs, &job.id);
                println!(
                    "  -> moved to failed (attempt {}/{})",
                    job.attempt, job.max_retries
                );
            } else {
                fs_ops::requeue_job(&dirs, &job)?;
                println!(
                    "  -> requeued (attempt {}/{})",
                    job.attempt, job.max_retries
                );
            }
            count += 1;
        }
    }

    if count == 0 {
        println!("No stale jobs found.");
    } else {
        println!("Recovered {} stale jobs.", count);
    }
    Ok(())
}
