use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use chrono::Utc;

use crate::config::Config;

use super::fs_ops::{self, SharedDirs};
use super::job::{ContentType, Heartbeat};

pub fn run_worker(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let shared_dir = config
        .distributed
        .shared_dir
        .as_deref()
        .ok_or("distributed.shared_dir not configured")?;
    let dirs = SharedDirs::new(shared_dir);
    dirs.ensure_dirs()?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    ctrlc::set_handler(move || {
        println!("\nShutting down worker...");
        shutdown_clone.store(true, Ordering::SeqCst);
    })?;

    println!(
        "Worker '{}' started, polling {}",
        config.distributed.worker_name, shared_dir
    );

    while !shutdown.load(Ordering::SeqCst) {
        // Recover stale jobs on each poll cycle
        recover_stale_jobs(&dirs, config);

        // Poll for pending jobs
        let pending = fs_ops::list_jobs_in(&dirs.pending);

        for mut job in pending {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            if !fs_ops::claim_job(&dirs, &job.id) {
                continue; // Another worker claimed it
            }

            println!("Claimed job {} ({})", job.id, job.relative_output_path);

            // Start heartbeat thread with per-job stop flag
            let job_done = Arc::new(AtomicBool::new(false));
            let job_done_clone = job_done.clone();
            let hb_shared_dir = shared_dir.to_string();
            let hb_job_id = job.id;
            let hb_worker = config.distributed.worker_name.clone();

            let hb_handle = thread::spawn(move || {
                let hb_dirs = SharedDirs::new(&hb_shared_dir);
                while !job_done_clone.load(Ordering::SeqCst) {
                    let hb = Heartbeat {
                        worker_name: hb_worker.clone(),
                        timestamp: Utc::now(),
                        pid: std::process::id(),
                    };
                    let _ = fs_ops::write_heartbeat(&hb_dirs, &hb_job_id, &hb);
                    // Sleep in 1s increments so we can check the stop flag
                    for _ in 0..30 {
                        if job_done_clone.load(Ordering::SeqCst) {
                            break;
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            });

            // Resolve output base directory from content type
            let output_base = match job.content_type {
                ContentType::Movie => config.movie_dir.as_deref().ok_or("movie_dir not configured"),
                ContentType::Episode => {
                    config.show_dir.as_deref().ok_or("show_dir not configured")
                }
            };

            let result = match output_base {
                Ok(base) => {
                    // Create output parent dirs before transcoding
                    let output_path =
                        std::path::Path::new(base).join(&job.relative_output_path);
                    if let Some(parent) = output_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    println!("Transcoding to {}...", output_path.display());
                    let transcode_job = job.to_transcode_job(&dirs.root, base);
                    transcode_job.run().map_err(|e| e.to_string())
                }
                Err(e) => Err(e.to_string()),
            };

            // Stop heartbeat thread
            job_done.store(true, Ordering::SeqCst);
            let _ = hb_handle.join();

            match result {
                Ok(()) => {
                    fs_ops::complete_job(&dirs, &job.id)?;
                    println!("Completed job {}", job.id);

                    if config.distributed.cleanup_raw_media {
                        let _ = fs_ops::maybe_cleanup_media(&dirs, &job.media_file);
                    }
                }
                Err(e) => {
                    eprintln!("Job {} failed: {}", job.id, e);
                    fs_ops::fail_job(&dirs, &mut job, &e)?;
                    if job.attempt < job.max_retries {
                        println!(
                            "Requeued job {} (attempt {}/{})",
                            job.id, job.attempt, job.max_retries
                        );
                    } else {
                        println!(
                            "Job {} moved to failed after {} attempts",
                            job.id, job.attempt
                        );
                    }
                }
            }
        }

        // Sleep between poll cycles, checking shutdown flag each second
        for _ in 0..config.distributed.poll_interval_secs {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }

    println!("Worker stopped.");
    Ok(())
}

fn recover_stale_jobs(dirs: &SharedDirs, config: &Config) {
    let active = fs_ops::list_jobs_in(&dirs.active);
    for mut job in active {
        if fs_ops::is_heartbeat_stale(dirs, &job.id, config.distributed.stale_lock_timeout_secs) {
            println!("Recovering stale job {} (no heartbeat)", job.id);
            job.attempt += 1;
            if job.attempt >= job.max_retries {
                job.error_message =
                    Some("Stale heartbeat - max retries exceeded".to_string());
                let _ = fs_ops::write_job_atomic(&dirs.failed, &job);
                let active_path = dirs.active.join(format!("{}.job.json", job.id));
                let _ = std::fs::remove_file(&active_path);
                fs_ops::remove_heartbeat(dirs, &job.id);
                println!(
                    "Job {} moved to failed after {} attempts",
                    job.id, job.attempt
                );
            } else {
                let _ = fs_ops::requeue_job(dirs, &job);
                println!(
                    "Requeued stale job {} (attempt {}/{})",
                    job.id, job.attempt, job.max_retries
                );
            }
        }
    }
}
