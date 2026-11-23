//! Runs jobs on a fixed number of std threads that share one queue. Results come back to the
//! calling thread as they finish, so it can print and count without locks. Once `stop` returns
//! true no new job starts; jobs already running finish (or fail) on their own.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

pub fn run<J, R>(
    jobs: &[J],
    threads: usize,
    stop: &(dyn Fn() -> bool + Sync),
    work: &(dyn Fn(&J) -> R + Sync),
    mut finished: impl FnMut(usize, R),
) where
    J: Sync,
    R: Send,
{
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        for _ in 0..threads.clamp(1, jobs.len().max(1)) {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || {
                while !stop() {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(job) = jobs.get(index) else { break };
                    if sender.send((index, work(job))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, result) in receiver {
            finished(index, result);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn runs_every_job_once_in_parallel() {
        let jobs: Vec<u64> = (0..100).collect();
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let mut results = vec![None; jobs.len()];
        run(
            &jobs,
            4,
            &|| false,
            &|job| {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                thread::sleep(std::time::Duration::from_millis(2));
                running.fetch_sub(1, Ordering::SeqCst);
                job * 2
            },
            |index, result| results[index] = Some(result),
        );
        assert!(results.iter().enumerate().all(|(i, r)| *r == Some(i as u64 * 2)));
        assert!(most.load(Ordering::SeqCst) > 1 && most.load(Ordering::SeqCst) <= 4);
    }

    #[test]
    fn stops_starting_jobs_once_asked() {
        let jobs: Vec<u32> = (0..50).collect();
        let stop = AtomicBool::new(false);
        let mut done = 0;
        run(
            &jobs,
            1,
            &|| stop.load(Ordering::SeqCst),
            &|job| {
                if *job == 9 {
                    stop.store(true, Ordering::SeqCst);
                }
            },
            |_, ()| done += 1,
        );
        assert_eq!(done, 10);
    }

    #[test]
    fn handles_no_jobs() {
        let mut calls = 0;
        run(&Vec::<u32>::new(), 4, &|| false, &|_| (), |_, ()| calls += 1);
        assert_eq!(calls, 0);
    }
}
