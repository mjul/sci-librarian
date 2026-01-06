use crate::clients::{DropboxClient, LlmClient};
use crate::models::{FileStatus, Job, JobResult, RemotePath, Rules, WorkDirectory};
use crate::storage::Storage;
use anyhow::Result;
use colored::*;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Pipeline {
    storage: Arc<Storage>,
    dropbox: Arc<dyn DropboxClient>,
    llm: Arc<dyn LlmClient>,
    multi_progress: MultiProgress,
    work_dir: WorkDirectory,
    rules: Arc<Rules>,
}

impl Pipeline {
    pub fn new(
        storage: Arc<Storage>,
        dropbox: Arc<dyn DropboxClient>,
        llm: Arc<dyn LlmClient>,
        work_dir: WorkDirectory,
        rules: Arc<Rules>,
    ) -> Self {
        Self {
            storage,
            dropbox,
            llm,
            multi_progress: MultiProgress::new(),
            work_dir,
            rules,
        }
    }

    pub async fn run_batch(&self, batch_size: i64, num_workers: usize) -> Result<()> {
        let pending = self.storage.get_pending_files(batch_size).await?;
        if pending.is_empty() {
            println!("{}", "No pending files to process.".yellow());
            return Ok(());
        }

        let (job_tx, job_rx) = mpsc::channel(batch_size as usize);
        let (result_tx, mut result_rx) = mpsc::channel(batch_size as usize);

        // 1. Scanner: Push jobs to queue
        for file in pending {
            let job = Job {
                id: file.dropbox_id,
                file_name: file.file_name,
                path: RemotePath("".to_string()), // We might need the path from DB if we store it
            };
            job_tx.send(job).await?;
        }
        drop(job_tx);

        // 2. Workers: Spawn worker threads
        let mut worker_handles = Vec::new();
        let job_rx = Arc::new(tokio::sync::Mutex::new(job_rx));

        for i in 0..num_workers {
            let job_rx = Arc::clone(&job_rx);
            let result_tx = result_tx.clone();
            let dropbox = Arc::clone(&self.dropbox);
            let llm = Arc::clone(&self.llm);
            let work_dir = self.work_dir.clone();
            let rules = Arc::clone(&self.rules);

            let pb = self.multi_progress.add(ProgressBar::new_spinner());
            pb.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.green} [{elapsed_precise}] {msg}")?,
            );
            pb.set_message(format!("Worker {}", i));

            let handle = tokio::spawn(async move {
                while let Some(job) = {
                    let mut rx = job_rx.lock().await;
                    rx.recv().await
                } {
                    let display_name = job.file_name.as_deref().unwrap_or("unknown");
                    pb.set_message(format!("Processing {} ({})", display_name, job.id.0));
                    let result =
                        process_file(job, &*dropbox, &*llm, &work_dir, &rules)
                            .await;
                    let _ = result_tx.send(result).await;
                }
                pb.finish_with_message(format!("Worker {} idle", i));
            });
            worker_handles.push(handle);
        }
        drop(result_tx);

        // 3. Collector: Listen for results and update DB/UI
        let main_pb = self.multi_progress.add(ProgressBar::new(batch_size as u64));
        main_pb.set_style(
            ProgressStyle::default_bar().template(
                "{span:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}",
            )?,
        );
        main_pb.set_message("Overall Progress");

        while let Some(result) = result_rx.recv().await {
            match result {
                JobResult::Success {
                    id,
                    file_name,
                    meta: _,
                    target_paths: _,
                } => {
                    // Update DB with metadata and status
                    // For now, just update status
                    self.storage
                        .update_status(&id, FileStatus::Processed)
                        .await?;
                    let display_name = file_name.as_deref().unwrap_or("unknown");
                    main_pb.println(format!("{} Processed {} ({})", "✔".green(), display_name, id.0));
                }
                JobResult::Failure { id, file_name, error } => {
                    self.storage.update_status(&id, FileStatus::Error).await?;
                    let display_name = file_name.as_deref().unwrap_or("unknown");
                    main_pb.println(format!("{} Failed {} ({}): {}", "✘".red(), display_name, id.0, error));
                }
            }
            main_pb.inc(1);
        }

        for handle in worker_handles {
            let _ = handle.await;
        }

        main_pb.finish_with_message("Batch complete");

        Ok(())
    }
}

async fn process_file(
    job: Job,
    dropbox: &dyn DropboxClient,
    llm: &dyn LlmClient,
    work_dir: &WorkDirectory,
    rules: &Rules,
) -> JobResult {
    // 1. Download
    let content = match dropbox.download_file(&job.id).await {
        Ok(c) => c,
        Err(e) => {
            return JobResult::Failure {
                id: job.id,
                file_name: job.file_name,
                error: e.to_string(),
            };
        }
    };

    // 2. Save to local raw directory
    let sanitized_id = job.id.0.replace([':', '/', '\\', ' '], "_");
    let local_path = work_dir.0.join("raw").join(format!("{}.pdf", sanitized_id));
    if let Err(e) = fs::write(&local_path, &content) {
        return JobResult::Failure {
            id: job.id,
            file_name: job.file_name,
            error: format!("Failed to save local copy: {}", e),
        };
    }

    // 3. Extract Text (lopdf)
    let text = match extract_text(&content) {
        Ok(t) => t,
        Err(e) => {
            return JobResult::Failure {
                id: job.id,
                file_name: job.file_name,
                error: e.to_string(),
            };
        }
    };

    // 4. LLM Analysis
    let (meta, targets) = match llm.query_llm(&text, &rules).await {
        Ok(r) => r,
        Err(e) => {
            return JobResult::Failure {
                id: job.id,
                file_name: job.file_name,
                error: e.to_string(),
            };
        }
    };

    // 5. Upload
    for target in &targets {
        if let Err(e) = dropbox.upload_file(target, content.clone()).await {
            return JobResult::Failure {
                id: job.id,
                file_name: job.file_name,
                error: e.to_string(),
            };
        }
        let sidecar_path = RemotePath(format!("{}.md", target.0));
        let sidecar_content = format!(
            "# {}\n\nAuthors: {}\n\nSummary: {}\n\nAbstract: {}",
            meta.title,
            meta.authors.join(", "),
            meta.summary.0,
            meta.abstract_text
        );
        if let Err(e) = dropbox
            .upload_file(&sidecar_path, sidecar_content.into_bytes())
            .await
        {
            return JobResult::Failure {
                id: job.id,
                file_name: job.file_name,
                error: e.to_string(),
            };
        }
    }

    JobResult::Success {
        id: job.id,
        file_name: job.file_name,
        meta,
        target_paths: targets,
    }
}

fn extract_text(content: &[u8]) -> Result<String> {
    let doc = lopdf::Document::load_mem(content)?;
    let mut text = String::new();

    // Extract from first 5 pages as per PRD
    let pages = doc.get_pages();
    let max_pages = std::cmp::min(pages.len(), 5);

    for i in 1..=max_pages {
        if let Ok(page_text) = doc.extract_text(&[i as u32]) {
            text.push_str(&page_text);
            text.push('\n');
        }
    }

    if text.trim().is_empty() {
        return Err(anyhow::anyhow!("No text extracted from PDF"));
    }

    Ok(text)
}
