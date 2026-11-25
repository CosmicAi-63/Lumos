//! Background jobs (finalize now; export/import later). In memory for M0.

use std::sync::Mutex;
use std::time::Duration;

use lumos_model::events::{self, Event};
use lumos_model::jobs::{Job, JobKind, JobStatus};
use lumos_model::{ToolError, ToolResult, new_id, now_rfc3339};
use serde_json::Value;
use tokio::sync::{broadcast, watch};

const KEEP: usize = 200;

pub struct Jobs {
    entries: Mutex<Vec<(Job, watch::Sender<Job>)>>,
    events: broadcast::Sender<Event>,
}

impl Jobs {
    pub fn new(events: broadcast::Sender<Event>) -> Self {
        Self { entries: Mutex::new(Vec::new()), events }
    }

    pub fn create(&self, kind: JobKind, recording_id: Option<String>) -> Job {
        let job = Job {
            id: new_id("job"),
            kind,
            status: JobStatus::Queued,
            recording_id,
            progress: 0.0,
            created_at: now_rfc3339(),
            started_at: None,
            finished_at: None,
            result: None,
            error: None,
        };
        let (tx, _) = watch::channel(job.clone());
        let mut entries = self.entries.lock().unwrap();
        entries.push((job.clone(), tx));
        // Drop the oldest finished jobs past the cap.
        while entries.len() > KEEP {
            match entries.iter().position(|(j, _)| j.status.is_terminal()) {
                Some(i) => {
                    entries.remove(i);
                }
                None => break,
            }
        }
        job
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let mut entries = self.entries.lock().unwrap();
        let (job, tx) = entries.iter_mut().find(|(j, _)| j.id == id)?;
        f(job);
        // `send` would discard the value when nobody is subscribed yet.
        tx.send_replace(job.clone());
        Some(job.clone())
    }

    pub fn start(&self, id: &str) {
        self.update(id, |j| {
            j.status = JobStatus::Running;
            j.started_at = Some(now_rfc3339());
        });
    }

    pub fn progress(&self, id: &str, progress: f64) {
        if let Some(job) = self.update(id, |j| j.progress = progress.clamp(0.0, 1.0)) {
            let _ = self.events.send(Event::new(events::JOB_PROGRESS, serde_json::json!({ "jobId": job.id, "progress": job.progress })));
        }
    }

    pub fn finish(&self, id: &str, result: ToolResult<Value>) {
        let job = self.update(id, |j| {
            j.finished_at = Some(now_rfc3339());
            match result {
                Ok(v) => {
                    j.status = JobStatus::Done;
                    j.progress = 1.0;
                    j.result = Some(v);
                }
                Err(e) => {
                    j.status = JobStatus::Failed;
                    j.error = Some(e);
                }
            }
        });
        if let Some(job) = job {
            let _ = self.events.send(Event::new(events::JOB_FINISHED, &job));
        }
    }

    pub fn get(&self, id: &str) -> ToolResult<Job> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|(j, _)| j.id == id)
            .map(|(j, _)| j.clone())
            .ok_or_else(|| ToolError::not_found(format!("no job '{id}'")))
    }

    pub fn list(&self, limit: usize) -> Vec<Job> {
        self.entries.lock().unwrap().iter().rev().take(limit).map(|(j, _)| j.clone()).collect()
    }

    /// Ids of unfinished jobs of one kind.
    pub fn active(&self, kind: JobKind) -> Vec<String> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(j, _)| j.kind == kind && !j.status.is_terminal())
            .map(|(j, _)| j.id.clone())
            .collect()
    }

    /// Wait until the job ends or `timeout` passes; either way return its current state.
    pub async fn wait(&self, id: &str, timeout: Duration) -> ToolResult<Job> {
        let mut rx = {
            let entries = self.entries.lock().unwrap();
            let (_, tx) = entries.iter().find(|(j, _)| j.id == id).ok_or_else(|| ToolError::not_found(format!("no job '{id}'")))?;
            tx.subscribe()
        };
        let _ = tokio::time::timeout(timeout, rx.wait_for(|j| j.status.is_terminal())).await;
        let job = rx.borrow().clone();
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_returns_early_and_on_timeout() {
        let (tx, _) = broadcast::channel(16);
        let jobs = std::sync::Arc::new(Jobs::new(tx));
        let job = jobs.create(JobKind::Finalize, None);
        jobs.start(&job.id);

        let timed_out = jobs.wait(&job.id, Duration::from_millis(20)).await.unwrap();
        assert_eq!(timed_out.status, JobStatus::Running);

        let j2 = jobs.clone();
        let id = job.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            j2.finish(&id, Ok(serde_json::json!({ "ok": true })));
        });
        let done = jobs.wait(&job.id, Duration::from_secs(5)).await.unwrap();
        assert_eq!(done.status, JobStatus::Done);
        assert!(jobs.active(JobKind::Finalize).is_empty());
    }
}
