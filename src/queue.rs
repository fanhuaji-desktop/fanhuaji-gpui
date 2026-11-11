//! The file queue and its batch state, ported from the Tauri app's `queue.ts`. The batch
//! flags only change through `begin`/`stop`/`end`, so how a stopped batch and a
//! single-file Reconvert interact is decided here.

use fanhuaji_core::PreviewResult;
use std::path::PathBuf;

pub type FileId = u64;

#[derive(Debug, PartialEq)]
pub enum Status {
    Pending,
    Converting(Option<(usize, usize)>),
    Done(String),
    Warning(String),
    Failed(String),
}

pub struct FileItem {
    pub id: FileId,
    pub path: PathBuf,
    pub status: Status,
    /// What the finished conversion wrote, and with which converter.
    pub saved: Option<(PreviewResult, &'static str)>,
}

impl FileItem {
    pub fn is_finished(&self) -> bool {
        matches!(self.status, Status::Done(_) | Status::Warning(_))
    }
}

#[derive(Default)]
pub struct Queue {
    pub files: Vec<FileItem>,
    next_id: FileId,
    converting: bool,
    /// Set by Stop; the batch loop exits before starting its next file.
    stop_requested: bool,
    /// The last batch was stopped.
    stopped: bool,
    was_stopped: bool,
}

impl Queue {
    pub fn converting(&self) -> bool {
        self.converting
    }
    pub fn stop_requested(&self) -> bool {
        self.stop_requested
    }
    #[allow(dead_code)] // shortcut: read by the auto-convert setting, not ported yet.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Add a file unless the same path is already queued. Returns whether it was added.
    pub fn add(&mut self, path: PathBuf) -> bool {
        if self.files.iter().any(|f| f.path == path) {
            return false;
        }
        self.next_id += 1;
        self.files.push(FileItem {
            id: self.next_id,
            path,
            status: Status::Pending,
            saved: None,
        });
        true
    }

    pub fn get(&self, id: FileId) -> Option<&FileItem> {
        self.files.iter().find(|f| f.id == id)
    }

    pub fn get_mut(&mut self, id: FileId) -> Option<&mut FileItem> {
        self.files.iter_mut().find(|f| f.id == id)
    }

    /// Start a batch over the pending files, or only `only` (Reconvert), and return their
    /// ids; None when a batch is already running or nothing is pending. A single-file
    /// batch leaves a stopped queue stopped.
    pub fn begin(&mut self, only: Option<FileId>) -> Option<Vec<FileId>> {
        if self.converting {
            return None;
        }
        let pending: Vec<FileId> = self
            .files
            .iter()
            .filter(|f| f.status == Status::Pending && only.is_none_or(|id| f.id == id))
            .map(|f| f.id)
            .collect();
        if pending.is_empty() {
            return None;
        }
        self.was_stopped = self.stopped;
        self.converting = true;
        if only.is_none() {
            self.stopped = false;
        }
        Some(pending)
    }

    pub fn stop(&mut self) {
        self.stop_requested = true;
    }

    pub fn end(&mut self, only: Option<FileId>) {
        self.converting = false;
        self.stopped = self.stop_requested || (only.is_some() && self.was_stopped);
        self.stop_requested = false;
    }

    /// Files added mid-batch still wait; a stopped batch leaves them pending on purpose.
    #[allow(dead_code)] // shortcut: read by the auto-convert setting, not ported yet.
    pub fn should_resume(&self, auto_convert: bool) -> bool {
        !self.stopped && auto_convert && self.files.iter().any(|f| f.status == Status::Pending)
    }

    /// Failed files go back to pending for Retry.
    pub fn reset_errors(&mut self) {
        for f in &mut self.files {
            if matches!(f.status, Status::Failed(_)) {
                f.status = Status::Pending;
            }
        }
    }

    /// Drop finished files, keeping pending, running and failed ones.
    pub fn remove_completed(&mut self) {
        self.files.retain(|f| !f.is_finished());
    }

    /// Remove one file; the file being converted stays.
    pub fn remove(&mut self, id: FileId) {
        self.files
            .retain(|f| f.id != id || matches!(f.status, Status::Converting(_)));
    }
}

#[cfg(test)]
mod tests {
    //! `queue.test.ts` plus the `resetErrors` / `removeCompleted` / `removeFile` cases.
    use super::*;

    fn queue(statuses: &[Status]) -> (Queue, Vec<FileId>) {
        let mut q = Queue::default();
        for (i, _) in statuses.iter().enumerate() {
            q.add(PathBuf::from(format!("/tmp/{i}.srt")));
        }
        let ids: Vec<FileId> = q.files.iter().map(|f| f.id).collect();
        for (f, s) in q.files.iter_mut().zip(statuses) {
            f.status = match s {
                Status::Pending => Status::Pending,
                Status::Converting(p) => Status::Converting(*p),
                Status::Done(x) => Status::Done(x.clone()),
                Status::Warning(x) => Status::Warning(x.clone()),
                Status::Failed(x) => Status::Failed(x.clone()),
            };
        }
        (q, ids)
    }

    fn done() -> Status {
        Status::Done("out".into())
    }

    #[test]
    fn runs_the_pending_files_and_refuses_a_second_batch_meanwhile() {
        let (mut q, ids) = queue(&[Status::Pending, done(), Status::Pending]);
        assert_eq!(q.begin(None), Some(vec![ids[0], ids[2]]));
        assert!(q.converting());
        assert_eq!(q.begin(None), None);
        q.end(None);
        assert!(!q.converting());
    }

    #[test]
    fn returns_none_when_nothing_is_pending() {
        let (mut q, _) = queue(&[done()]);
        assert_eq!(q.begin(None), None);
        assert!(!q.converting());
    }

    #[test]
    fn does_not_resume_a_stopped_batch() {
        let (mut q, _) = queue(&[Status::Pending, Status::Pending]);
        q.begin(None);
        q.stop();
        q.end(None);
        assert!(q.stopped());
        assert!(!q.stop_requested());
        assert!(!q.should_resume(true));
    }

    #[test]
    fn keeps_a_stopped_queue_stopped_after_reconverting_one_file() {
        let (mut q, ids) = queue(&[Status::Pending, Status::Pending]);
        q.begin(None);
        q.stop();
        q.end(None);
        assert_eq!(q.begin(Some(ids[0])), Some(vec![ids[0]]));
        q.end(Some(ids[0]));
        assert!(q.stopped());
        assert!(!q.should_resume(true));
    }

    #[test]
    fn picks_up_files_added_mid_batch_when_auto_convert_is_on() {
        let (mut q, ids) = queue(&[Status::Pending]);
        q.begin(None);
        q.get_mut(ids[0]).unwrap().status = done();
        q.add(PathBuf::from("/tmp/b.srt"));
        q.end(None);
        assert!(q.should_resume(true));
        assert!(!q.should_resume(false));
    }

    #[test]
    fn a_full_batch_clears_the_stopped_state() {
        let (mut q, _) = queue(&[Status::Pending, Status::Pending]);
        q.begin(None);
        q.stop();
        q.end(None);
        q.begin(None);
        q.end(None);
        assert!(!q.stopped());
    }

    #[test]
    fn add_skips_a_path_already_queued_and_ids_stay_unique() {
        let mut q = Queue::default();
        assert!(q.add("/tmp/a.srt".into()));
        assert!(!q.add("/tmp/a.srt".into()));
        assert!(q.add("/tmp/b.srt".into()));
        assert_ne!(q.files[0].id, q.files[1].id);
        let gone = q.files[0].id;
        q.remove(gone);
        q.add("/tmp/c.srt".into());
        assert!(q.files.iter().all(|f| f.id != gone), "ids are never reused");
    }

    #[test]
    fn reset_errors_turns_failed_files_back_to_pending() {
        let (mut q, _) = queue(&[Status::Failed("X".into()), done(), Status::Pending]);
        q.reset_errors();
        let s: Vec<&Status> = q.files.iter().map(|f| &f.status).collect();
        assert_eq!(s, [&Status::Pending, &done(), &Status::Pending]);
    }

    #[test]
    fn remove_completed_keeps_pending_running_and_failed_files() {
        let (mut q, ids) = queue(&[
            Status::Pending,
            done(),
            Status::Warning("w".into()),
            Status::Converting(None),
            Status::Failed("X".into()),
        ]);
        q.remove_completed();
        let left: Vec<FileId> = q.files.iter().map(|f| f.id).collect();
        assert_eq!(left, [ids[0], ids[3], ids[4]]);
    }

    #[test]
    fn remove_keeps_the_file_being_converted() {
        let (mut q, ids) = queue(&[Status::Converting(None), Status::Pending]);
        q.remove(ids[0]);
        q.remove(ids[1]);
        let left: Vec<FileId> = q.files.iter().map(|f| f.id).collect();
        assert_eq!(left, [ids[0]]);
    }
}
