//! Following an item as it streams: reading what it holds, and waiting at its end until more is
//! written or it is whole.
//!
//! New data is waited for on the open file: by inotify on Linux, and by kqueue on macOS, since
//! FSEvents tells of a file's new data only once its writer closes it. The end is waited for on
//! the lock, since the writer's release changes no data.

use std::{
  fs,
  io::{self, Read},
  path::Path,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  thread,
};

use crate::at;

/// Follows an item where it lies: reads what it holds, and blocks at its end until more is written
/// or it is whole, so that the end of reading is the end of the item. It waits by filesystem
/// notifications and by the lock, never by polling. Follow an item after moving it, as a taker
/// does, so that it stays where it is followed.
pub fn follow(path: &Path) -> io::Result<Follow> {
  let file = fs::File::open(path).map_err(|e| at(e, path))?;
  let done = Arc::new(AtomicBool::new(false));
  let lock = fs::File::open(path).map_err(|e| at(e, path))?;
  match lock.try_lock_shared() {
    Ok(()) => {
      done.store(true, Ordering::Release);
      return Ok(Follow {
        file,
        done,
        changes: None,
      });
    }
    Err(fs::TryLockError::WouldBlock) => {}
    Err(fs::TryLockError::Error(e)) => return Err(at(e, path)),
  }
  let (changes, end) = changes::watch(&file, path).map_err(|e| at(e, path))?;
  // Where the follower is dropped first, the thread ends with the writer.
  thread::spawn({
    let done = done.clone();
    move || {
      let _ = lock.lock_shared();
      done.store(true, Ordering::Release);
      end.tell();
    }
  });
  Ok(Follow {
    file,
    done,
    changes: Some(changes),
  })
}

/// An item followed until it is whole (`follow`).
pub struct Follow {
  file: fs::File,
  done: Arc<AtomicBool>,
  changes: Option<changes::Changes>,
}

impl Read for Follow {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    loop {
      // Tested before the read: the writer wrote everything before it let go.
      let done = self.done.load(Ordering::Acquire);
      let n = self.file.read(buf)?;
      if n > 0 || done || buf.is_empty() {
        return Ok(n);
      }
      let Some(changes) = &self.changes else {
        return Ok(0);
      };
      changes.wait()?;
    }
  }
}

#[cfg(not(target_os = "macos"))]
mod changes {
  use std::{fs, io, path::Path, sync::mpsc};

  use notify::{RecursiveMode, Watcher};

  /// The changes to a followed file: new data, or its end.
  pub struct Changes {
    _watcher: notify::RecommendedWatcher,
    rx: mpsc::Receiver<()>,
  }

  /// Tells the follower that its item is whole.
  pub struct End(mpsc::Sender<()>);

  pub fn watch(_file: &fs::File, path: &Path) -> io::Result<(Changes, End)> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher({
      let tx = tx.clone();
      move |_| {
        let _ = tx.send(());
      }
    })
    .map_err(io::Error::other)?;
    watcher
      .watch(path, RecursiveMode::NonRecursive)
      .map_err(io::Error::other)?;
    Ok((
      Changes {
        _watcher: watcher,
        rx,
      },
      End(tx),
    ))
  }

  impl Changes {
    /// Blocks until the file changes or ends.
    pub fn wait(&self) -> io::Result<()> {
      self.rx.recv().map_err(io::Error::other)
    }
  }

  impl End {
    pub fn tell(self) {
      let _ = self.0.send(());
    }
  }
}

#[cfg(target_os = "macos")]
mod changes {
  use std::{
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    sync::Arc,
  };

  /// The changes to a followed file, as one kqueue: a write to the open file, which it watches
  /// wherever the file is moved, or its end, an event of its own.
  pub struct Changes(Arc<OwnedFd>);

  /// Tells the follower that its item is whole.
  pub struct End(Arc<OwnedFd>);

  fn event(ident: usize, filter: i16, flags: u16, fflags: u32) -> libc::kevent {
    libc::kevent {
      ident,
      filter,
      flags,
      fflags,
      data: 0,
      udata: std::ptr::null_mut(),
    }
  }

  /// Applies the changes to the kqueue, or waits for one event where there are none.
  fn kevent(kq: &OwnedFd, changes: &[libc::kevent]) -> io::Result<()> {
    // SAFETY: zeroes are a valid kevent to receive into.
    let mut out: libc::kevent = unsafe { std::mem::zeroed() };
    let (wanted, timeout) = match changes {
      [] => (1, std::ptr::null()),
      _ => (
        0,
        &libc::timespec {
          tv_sec: 0,
          tv_nsec: 0,
        } as *const _,
      ),
    };
    loop {
      // SAFETY: the changes given, and room for at most one event.
      let n = unsafe {
        libc::kevent(
          kq.as_raw_fd(),
          changes.as_ptr(),
          changes.len() as i32,
          &mut out,
          wanted,
          timeout,
        )
      };
      if n >= 0 {
        return Ok(());
      }
      let error = io::Error::last_os_error();
      if error.kind() != io::ErrorKind::Interrupted {
        return Err(error);
      }
    }
  }

  pub fn watch(file: &fs::File, _path: &Path) -> io::Result<(Changes, End)> {
    // SAFETY: kqueue returns a new descriptor or -1.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
      return Err(io::Error::last_os_error());
    }
    // SAFETY: a new descriptor, owned here alone.
    let kq = Arc::new(unsafe { OwnedFd::from_raw_fd(kq) });
    // The file's filter goes as its descriptor closes, with the follower.
    kevent(
      &kq,
      &[
        event(
          file.as_raw_fd() as usize,
          libc::EVFILT_VNODE,
          libc::EV_ADD | libc::EV_CLEAR,
          libc::NOTE_WRITE | libc::NOTE_EXTEND,
        ),
        event(0, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0),
      ],
    )?;
    Ok((Changes(kq.clone()), End(kq)))
  }

  impl Changes {
    /// Blocks until the file changes or ends.
    pub fn wait(&self) -> io::Result<()> {
      kevent(&self.0, &[])
    }
  }

  impl End {
    pub fn tell(self) {
      let _ = kevent(
        &self.0,
        &[event(0, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER)],
      );
    }
  }
}
