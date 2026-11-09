// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An owned finite Windows worker tree. A worker waits for its first input until
//! both normal completion supervision and independent cancellation are bound.
//! This product component neither invokes nor replaces an agent execution gate.

use std::process::{ChildStdin, ChildStdout, Command, ExitStatus};
use std::sync::{
    Arc, Condvar, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use cosca::{Job as CompletionJob, containment::TreeDrain};
use process_wrap::std::{ChildWrapper, CommandWrap, CommandWrapper, CreationFlags, JobObject};
use win32job::{ExtendedLimitInfo, Job, PriorityClass};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

type Result<T> = std::result::Result<T, String>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Default)]
struct Signal {
    state: Mutex<(bool, bool)>, // cancelled, completion observed
    changed: Condvar,
}

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    signals: Mutex<Vec<Weak<Signal>>>,
}

#[derive(Clone, Default)]
pub struct Cancellation(Arc<CancellationState>);

impl Cancellation {
    pub fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let mut signals = lock(&self.0.signals);
        signals.retain(|signal| {
            let Some(signal) = signal.upgrade() else { return false };
            lock(&signal.state).0 = true;
            signal.changed.notify_all();
            true
        });
    }

    fn register(&self) -> Arc<Signal> {
        let signal = Arc::new(Signal::default());
        let mut signals = lock(&self.0.signals);
        signals.retain(|signal| signal.strong_count() != 0);
        signals.push(Arc::downgrade(&signal));
        if self.cancelled() {
            lock(&signal.state).0 = true;
        }
        signal
    }
}

pub struct ManagedProcess {
    child: Box<dyn ChildWrapper>,
    // The launcher supplies suspended assignment/resumption, not completion:
    // process-wrap 10.0.1's job wait mistakes the first notification for exit.
    // This independent, retained job waits on actual member process handles.
    completion_job: CompletionJob,
    // An independently closable job allows cancellation while completion waits
    // retain their observation handle. None of these is an MCP host.
    cancellation_job: Arc<Mutex<Option<Job>>>,
    signal: Arc<Signal>,
    watcher: Option<JoinHandle<()>>,
    completed: bool,
}

#[derive(Debug)]
struct BindCompletion(Arc<Mutex<Option<CompletionJob>>>);

impl CommandWrapper for BindCompletion {
    fn post_spawn(&mut self, _: &mut Command, child: &mut std::process::Child, _: &CommandWrap) -> std::io::Result<()> {
        use std::os::windows::io::AsHandle;
        // Every post_spawn runs after JobObject's CREATE_SUSPENDED pre_spawn
        // and before its wrap_child resumes the process. Bind before any code.
        match CompletionJob::assign(child.as_handle()) {
            Ok(job) => {
                *lock(&self.0) = Some(job);
                Ok(())
            }
            Err(why) => {
                let killed = child.kill();
                let exited = child.wait();
                Err(std::io::Error::other(format!(
                    "content completion binding failed: {why}; termination requested: {}; exit observed: {}",
                    killed.is_ok(),
                    exited.is_ok()
                )))
            }
        }
    }
}

impl ManagedProcess {
    /// The caller must use the content protocol: the child cannot import a
    /// document, render, OCR or spawn another worker before its first stdin line.
    pub fn spawn(command: Command, cancellation: &Cancellation) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        if cancellation.cancelled() {
            return Err("content extraction cancelled before launch".into());
        }
        let mut limits = ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close().limit_priority_class(PriorityClass::BelowNormal);
        let job = Job::create_with_limit_info(&limits).map_err(|why| why.to_string())?;
        let executable = command.get_program().to_string_lossy().into_owned();
        let completion = Arc::new(Mutex::new(None));
        let mut command = CommandWrap::from(command);
        command.wrap(CreationFlags(CREATE_NO_WINDOW)).wrap(BindCompletion(completion.clone())).wrap(JobObject);
        let mut child = command.spawn().map_err(|why| why.to_string())?;
        let completion_job = lock(&completion).take().ok_or("the suspended content worker has no completion binding")?;
        let pid = child.id();
        let assigned = child
            .process_handle()
            .ok_or_else(|| "the native worker has no owned process handle".to_owned())
            .and_then(|handle| job.assign_process(handle.as_raw_handle() as isize).map_err(|why| why.to_string()));
        if let Err(why) = assigned {
            let _ = child.start_kill();
            let exit = wait_tree(child.as_mut(), &completion_job);
            return Err(format!("content worker {executable}, PID {pid}, cancellation binding failed: {why}; exit observed: {}", exit.is_ok()));
        }
        let cancellation_job = Arc::new(Mutex::new(Some(job)));
        let signal = cancellation.register();
        let watched_signal = signal.clone();
        let watched_job = cancellation_job.clone();
        let watcher = std::thread::Builder::new().name("craft-content-cancellation".into()).spawn(move || {
            let state = lock(&watched_signal.state);
            let state = watched_signal
                .changed
                .wait_while(state, |(cancelled, complete)| !*cancelled && !*complete)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.0 && !state.1 {
                // Closing exactly this job terminates only its owned process
                // tree. The owner still waits for actual exit before completion.
                lock(&watched_job).take();
            }
        });
        match watcher {
            Ok(watcher) => Ok(Self { child, completion_job, cancellation_job, signal, watcher: Some(watcher), completed: false }),
            Err(why) => {
                lock(&cancellation_job).take();
                let exit = wait_tree(child.as_mut(), &completion_job);
                Err(format!("content worker {executable}, PID {pid}, cancellation thread could not start: {why}; exit observed: {}", exit.is_ok()))
            }
        }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin().take()
    }

    pub fn stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout().take()
    }

    pub fn abort(&mut self) {
        lock(&self.cancellation_job).take();
    }

    pub fn wait(&mut self) -> Result<ExitStatus> {
        let result = wait_tree(self.child.as_mut(), &self.completion_job);
        if result.is_ok() {
            self.completed = true;
            lock(&self.signal.state).1 = true;
            self.signal.changed.notify_all();
            if let Some(watcher) = self.watcher.take() {
                watcher.join().map_err(|_| "the content cancellation watcher panicked".to_owned())?;
            }
            lock(&self.cancellation_job).take();
        }
        result
    }
}

fn wait_tree(child: &mut dyn ChildWrapper, job: &CompletionJob) -> Result<ExitStatus> {
    // Only JobObject installs a ChildWrapper in this launch chain. Use its
    // documented direct inner child, not its faulty job wait or unsafe mutable
    // traversal API. Refuse a different chain instead of assuming its invariants.
    let inner = child.inner_mut();
    if !(inner as &dyn std::any::Any).is::<std::process::Child>() {
        return Err("the content worker lost its direct owned native child".into());
    }
    let status = inner.wait().map_err(|why| why.to_string())?;
    match job.wait_tree().map_err(|why| why.to_string())? {
        TreeDrain::AllMembersExited => Ok(status),
        _ => Err("the content worker tree has no confirmed complete exit".into()),
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if !self.completed {
            self.abort();
            let _ = wait_tree(self.child.as_mut(), &self.completion_job);
        }
        lock(&self.signal.state).1 = true;
        self.signal.changed.notify_all();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}
