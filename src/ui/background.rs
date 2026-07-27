//! Running one piece of work off the UI thread.
//!
//! Every slow thing this program does — reading a repository, creating a
//! worktree, removing one — takes long enough to drop frames if it ran where
//! the window is drawn. Each becomes a [`Task`]: started once, polled without
//! blocking, delivering exactly one result.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

/// The state of a task each time it is asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll<T> {
    /// Still working, or already collected.
    Pending,
    Ready(T),
    /// The worker died without producing anything. Reported rather than
    /// waited on, so a panicked thread cannot leave the window spinning
    /// forever.
    Lost,
}

pub struct Task<T> {
    receiver: Receiver<T>,
    finished: bool,
}

impl<T> Task<T> {
    /// Takes the result if it is ready. Never blocks, and yields a result at
    /// most once.
    pub fn poll(&mut self) -> Poll<T> {
        if self.finished {
            return Poll::Pending;
        }

        match self.receiver.try_recv() {
            Ok(value) => {
                self.finished = true;
                Poll::Ready(value)
            }
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                Poll::Lost
            }
        }
    }

    pub fn is_running(&self) -> bool {
        !self.finished
    }
}

/// Runs `work` on a new thread and wakes the window when it finishes.
///
/// The repaint request is what makes this usable: without it the result would
/// sit in the channel until some unrelated input happened to cause a frame.
pub fn spawn<T, F>(work: F, ctx: egui::Context) -> Task<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        // A failed send means the window is gone; there is nobody to tell.
        if sender.send(work()).is_ok() {
            ctx.request_repaint();
        }
    });

    Task {
        receiver,
        finished: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task fed by hand, so the polling rules can be tested without threads
    /// or timing.
    fn task<T>() -> (mpsc::Sender<T>, Task<T>) {
        let (sender, receiver) = mpsc::channel();
        (
            sender,
            Task {
                receiver,
                finished: false,
            },
        )
    }

    #[test]
    fn work_still_running_reports_pending() {
        let (_sender, mut task) = task::<u8>();

        assert_eq!(task.poll(), Poll::Pending);
        assert!(task.is_running());
    }

    #[test]
    fn a_finished_task_yields_its_result_once() {
        let (sender, mut task) = task();
        sender.send(7).expect("send");

        assert_eq!(task.poll(), Poll::Ready(7));
        assert!(!task.is_running());
        // Asking again must not resurrect the result or panic.
        assert_eq!(task.poll(), Poll::Pending);
    }

    #[test]
    fn a_worker_that_dies_without_a_result_is_reported_as_lost() {
        let (sender, mut task) = task::<u8>();
        drop(sender);

        assert_eq!(task.poll(), Poll::Lost);
        assert!(!task.is_running());
        assert_eq!(task.poll(), Poll::Pending);
    }

    #[test]
    fn a_result_sent_before_the_worker_ended_is_still_delivered() {
        let (sender, mut task) = task();
        sender.send(1).expect("send");
        drop(sender);

        assert_eq!(task.poll(), Poll::Ready(1));
    }
}
