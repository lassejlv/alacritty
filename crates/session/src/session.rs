use std::io;
#[cfg(not(windows))]
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{EventListener, WindowSize};

use crate::event_loop::EventLoop;
use crate::events::{Msg, Notifier};
use crate::pty;
use crate::sync::FairMutex;

/// A terminal and its PTY worker. Dropping it requests worker shutdown.
pub struct Session<T> {
    pub terminal: Arc<FairMutex<Term<T>>>,
    pub notifier: Notifier,
    #[cfg(not(windows))]
    pub master_fd: RawFd,
    #[cfg(not(windows))]
    pub shell_pid: u32,
}

impl<T: EventListener + Send + 'static> Session<T> {
    /// Start a PTY worker for a configured terminal.
    pub fn new(
        terminal: Term<T>,
        options: &pty::Options,
        size: WindowSize,
        window_id: u64,
        listener: T,
        record: bool,
    ) -> io::Result<Self> {
        let terminal = Arc::new(FairMutex::new(terminal));
        let pty = pty::new(options, size, window_id)?;
        #[cfg(not(windows))]
        let master_fd = pty.file().as_raw_fd();
        #[cfg(not(windows))]
        let shell_pid = pty.child().id();
        let event_loop =
            EventLoop::new(Arc::clone(&terminal), listener, pty, options.drain_on_exit, record)?;
        let notifier = Notifier(event_loop.channel());
        // Preserve detached worker cleanup: its PTY closes when the worker exits.
        let _worker = event_loop.spawn();
        Ok(Self {
            terminal,
            notifier,
            #[cfg(not(windows))]
            master_fd,
            #[cfg(not(windows))]
            shell_pid,
        })
    }
}

impl<T> Drop for Session<T> {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}
