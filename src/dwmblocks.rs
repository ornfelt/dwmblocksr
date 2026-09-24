/* See LICENSE file for copyright and license details.
 *
 * dwmblocksr runs a command for every block of the status bar, puts their
 * first output lines together and sets the result as the name of the root
 * window, which dwm draws as its status text. Every block is updated every
 * `interval` seconds and when its signal SIGRTMIN+`signal` arrives.
 *
 * A block with a signal starts with that signal as a byte (< ' '), so dwm
 * can tell which block was clicked. dwm then sends the same signal with the
 * mouse button as sigqueue()'s value, and the block's command is run with
 * BLOCK_BUTTON=<button> in its environment.
 *
 * Signals are only written to a pipe by the signal handlers; statusloop()
 * poll()s that pipe, plus the output of every running command in async mode.
 *
 * Every function of dwmblocks.c is a method of `Dwmblocks`, in the same order.
 */
use std::ffi::{c_char, c_int, c_void, CString};
use std::io::{self, Write};
use std::mem;
use std::os::unix::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use x11::xlib::{Display, Window, XDefaultScreen, XFlush, XOpenDisplay, XRootWindow, XStoreName};

use crate::config::{Block, Config};

pub const CMDLENGTH: usize = 100;

/* a queued signal: the block signal and the clicked button, 0 for an update */
#[repr(C)]
#[derive(Clone, Copy)]
struct SigEvent {
    signal: u32,
    button: c_int,
}

/* the handlers only have these: the write end of sigpipe, statusContinue and
 * SIGRTMIN (a libc call, so it is read once in setupsignals()) */
static SIGPIPE_FD: AtomicI32 = AtomicI32::new(-1);
static STATUS_CONTINUE: AtomicBool = AtomicBool::new(true);
static SIGPLUS: AtomicI32 = AtomicI32::new(0);

/// The text of one block, C's `char[CMDLENGTH]` without the NUL.
#[derive(Clone, Copy)]
pub struct BlockStatus {
    buf: [u8; CMDLENGTH],
    len: usize,
}

impl BlockStatus {
    const EMPTY: BlockStatus = BlockStatus { buf: [0; CMDLENGTH], len: 0 };

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len.min(CMDLENGTH)]
    }
}

/// The running command of a block, in async mode.
#[derive(Clone, Copy)]
struct BlockCmd {
    fd: c_int,    //read end of the running command's output, -1 if not running
    rerun: bool,  //the block was signalled again while its command was running
    len: usize,
    out: [u8; CMDLENGTH],
}

/// Where the status goes: the root window name, or stdout with -p.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WriteStatus {
    Setroot,
    Pstdout,
}

/// Every global of dwmblocks.c.
pub struct Dwmblocks {
    pub dpy: *mut Display,
    screen: c_int,
    pub root: Window,
    blocks: Vec<Block>,
    cmds: Vec<CString>, /* each block's command as the `sh -c` argument, built before any fork() */
    r#async: bool,
    statusbar: Vec<BlockStatus>,
    statusstr: [Vec<u8>; 2],
    sigpipe: [c_int; 2],
    delimiter: Vec<u8>,
    delimlen: usize,
    pub writestatus: WriteStatus,
    blockcmds: Vec<BlockCmd>,
}

fn errno() -> c_int {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// glibc's signal(): install `handler`, restarting interrupted system calls.
pub fn signal(sig: c_int, handler: extern "C" fn(c_int)) {
    // SAFETY: sigaction with a zeroed, then fully set, struct; the handler is
    // a plain extern "C" fn that only does async-signal-safe things.
    unsafe {
        let mut sa: libc::sigaction = mem::zeroed();
        sa.sa_sigaction = handler as libc::sighandler_t;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(sig, &sa, ptr::null_mut());
    }
}

impl Dwmblocks {
    /// The globals before main() runs, with `delimiter` (delim from the config,
    /// or the -d argument) cut to delimLen bytes as main() does.
    pub fn new(config: Config, mut delimiter: Vec<u8>, writestatus: WriteStatus) -> Self {
        let n = config.blocks.len();
        let cmds = config.blocks.iter().map(|b| CString::new(b.command.as_str()).unwrap_or_default()).collect();
        let mut delimlen = config.delimlen.min(delimiter.len());
        delimiter.truncate(delimlen);
        delimlen += 1; /* counts the NUL, like delimiter[delimLen++] = '\0' */
        Dwmblocks {
            dpy: ptr::null_mut(),
            screen: 0,
            root: 0,
            blocks: config.blocks,
            cmds,
            r#async: config.r#async,
            statusbar: vec![BlockStatus::EMPTY; n],
            /* STATUSLENGTH, so the Vecs never grow */
            statusstr: [Vec::with_capacity(n * CMDLENGTH + 1), Vec::with_capacity(n * CMDLENGTH + 1)],
            sigpipe: [-1, -1],
            delimiter,
            delimlen,
            writestatus,
            blockcmds: vec![BlockCmd { fd: -1, rerun: false, len: 0, out: [0; CMDLENGTH] }; n],
        }
    }

    //builds the status of a block from the output of its command
    pub fn setblockstatus(&self, block: &Block, cmdout: &[u8]) -> BlockStatus {
        let mut tempstatus = BlockStatus::EMPTY;
        let t = &mut tempstatus.buf;
        let mut start = 0;
        //mark the block with its signal so dwm can tell which block was clicked,
        //not when printing to stdout (-p) where the raw bytes would end up in the output
        if block.signal != 0 && self.writestatus != WriteStatus::Pstdout {
            t[start] = block.signal as u8;
            start += 1;
        }
        let icon = block.icon.as_bytes();
        let n = icon.len().min(CMDLENGTH - 1 - start);
        t[start..start + n].copy_from_slice(&icon[..n]);
        let mut i = start + n;
        //only use the first line of the output, as much of it as fits
        let limit = CMDLENGTH.saturating_sub(self.delimlen + 1);
        for &c in cmdout {
            if c == 0 || c == b'\n' || i >= limit {
                break;
            }
            t[i] = c;
            i += 1;
        }
        //drop a UTF-8 character that was cut in half because the output was too long
        let mut j = i;
        while j > start && t[j - 1] & 0xC0 == 0x80 {
            j -= 1;
        }
        if j > start {
            let lead = t[j - 1];
            let charlen = if lead >= 0xF0 {
                4
            } else if lead >= 0xE0 {
                3
            } else if lead >= 0xC0 {
                2
            } else {
                1
            };
            if i - (j - 1) < charlen {
                i = j - 1;
            }
        }
        //leave the block out if block and command output are both empty
        if i == start {
            i = 0;
        } else if !self.delimiter.is_empty() {
            let n = self.delimiter.len().min(self.delimlen).min(CMDLENGTH - 1 - i);
            t[i..i + n].copy_from_slice(&self.delimiter[..n]);
            i += n;
        }
        tempstatus.len = i;
        tempstatus
    }

    /// Fork `/bin/sh -c <command of block i>` with its stdout on a pipe;
    /// the pid and the read end (close-on-exec) of the pipe.
    fn startcmd(&self, i: usize) -> Option<(libc::pid_t, c_int)> {
        let cmd = self.cmds.get(i)?;
        let argv: [*const c_char; 4] = [c"sh".as_ptr(), c"-c".as_ptr(), cmd.as_ptr(), ptr::null()];
        let mut fds = [0; 2];
        // SAFETY: pipe() fills the two-int array. Between fork() and exec the
        // child only makes async-signal-safe calls (dup2, close, execv, _exit)
        // on argv, whose strings were all built before the fork.
        unsafe {
            if libc::pipe(fds.as_mut_ptr()) == -1 {
                return None;
            }
            let pid = libc::fork();
            if pid == -1 {
                libc::close(fds[0]);
                libc::close(fds[1]);
                return None;
            }
            if pid == 0 {
                libc::dup2(fds[1], libc::STDOUT_FILENO);
                libc::close(fds[0]);
                libc::close(fds[1]);
                libc::execv(c"/bin/sh".as_ptr(), argv.as_ptr());
                libc::_exit(127);
            }
            libc::close(fds[1]);
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            Some((pid, fds[0]))
        }
    }

    /// getcmd() of the ASYNC or the non-ASYNC build.
    pub fn getcmd(&mut self, i: usize) {
        if self.r#async {
            self.getcmd_async(i);
        } else {
            self.getcmd_sync(i);
        }
    }

    //starts the command of a block in the background, readcmd updates the block
    //once the command is done
    fn getcmd_async(&mut self, i: usize) {
        let Some(cmd) = self.blockcmds.get_mut(i) else {
            return;
        };
        if cmd.fd != -1 {
            cmd.rerun = true;
            return;
        }
        let Some((_, fd)) = self.startcmd(i) else {
            return;
        };
        // SAFETY: fd is the pipe's read end that startcmd() just opened.
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) };
        let cmd = &mut self.blockcmds[i];
        cmd.fd = fd;
        cmd.rerun = false;
        cmd.len = 0;
    }

    //reads the output of the running command of block i and updates the block
    //when the command is done
    pub fn readcmd(&mut self, i: usize) {
        let Some(cmd) = self.blockcmds.get_mut(i) else {
            return;
        };
        let mut buf = [0u8; 256];
        let mut n;

        //keep what fits, the rest is only read so the command can't block on a full pipe
        loop {
            // SAFETY: buf is a writable buffer of buf.len() bytes.
            n = unsafe { libc::read(cmd.fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            let keep = (n as usize).min(CMDLENGTH - 1 - cmd.len);
            cmd.out[cmd.len..cmd.len + keep].copy_from_slice(&buf[..keep]);
            cmd.len += keep;
        }
        if n == -1 {
            let e = errno();
            if e == libc::EAGAIN || e == libc::EINTR {
                return;
            }
        }
        // SAFETY: cmd.fd is the open read end of this command's pipe.
        unsafe { libc::close(cmd.fd) };
        cmd.fd = -1;
        let status = self.setblockstatus(&self.blocks[i], &self.blockcmds[i].out[..self.blockcmds[i].len]);
        self.statusbar[i] = status;
        if self.blockcmds[i].rerun {
            self.getcmd(i);
        }
    }

    //opens process *cmd and stores output in *output
    fn getcmd_sync(&mut self, i: usize) {
        //make sure status is same until output is ready
        let mut cmdout = [0u8; CMDLENGTH];
        let mut len = 0;
        /* popen() */
        let Some((pid, fd)) = self.startcmd(i) else {
            return;
        };
        /* fgets(): the first line, at most CMDLENGTH-1 bytes */
        while len < CMDLENGTH - 1 {
            // SAFETY: the pointer and length stay within cmdout[len..CMDLENGTH-1].
            let n = unsafe { libc::read(fd, cmdout[len..].as_mut_ptr() as *mut c_void, CMDLENGTH - 1 - len) };
            if n == -1 && errno() == libc::EINTR {
                continue;
            }
            if n <= 0 {
                break;
            }
            let n = n as usize;
            if let Some(nl) = cmdout[len..len + n].iter().position(|&c| c == b'\n') {
                len += nl + 1;
                break;
            }
            len += n;
        }
        /* pclose() */
        // SAFETY: fd is the read end startcmd() opened; pid is its child.
        unsafe {
            libc::close(fd);
            while libc::waitpid(pid, ptr::null_mut(), 0) == -1 && errno() == libc::EINTR {}
        }
        let status = self.setblockstatus(&self.blocks[i], &cmdout[..len]);
        self.statusbar[i] = status;
    }

    pub fn getcmds(&mut self, time: i32) {
        for i in 0..self.blocks.len() {
            //let a command that takes longer than its interval finish first
            if self.r#async && self.blockcmds[i].fd != -1 {
                continue;
            }
            let interval = self.blocks[i].interval;
            if (interval != 0 && (time as u32).is_multiple_of(interval)) || time == -1 {
                self.getcmd(i);
            }
        }
    }

    //runs the command of a clicked block in the background with BLOCK_BUTTON set,
    //then signals dwmblocksr to update the block from the command's normal output
    pub fn buttonhandler(&self, i: usize, button: c_int) {
        let Some(block) = self.blocks.get(i) else {
            return;
        };
        // SAFETY: getpid() has no preconditions.
        let (sig, pid) = (libc::SIGRTMIN() + block.signal as c_int, unsafe { libc::getpid() });
        let shcmd = format!("{}\nkill -{} {}", block.command, sig, pid);
        if shcmd.len() >= 1024 {
            return;
        }
        let Ok(shcmd) = CString::new(shcmd) else {
            return;
        };
        let Ok(btn) = CString::new(format!("BLOCK_BUTTON={}", button)) else {
            return;
        };
        /* setenv() is not async-signal-safe, so the command's environment
         * (ours with BLOCK_BUTTON set) is built before forking */
        let env: Vec<CString> = std::env::vars_os()
            .filter(|(k, _)| k != "BLOCK_BUTTON")
            .filter_map(|(k, v)| {
                let mut kv = k.as_bytes().to_vec();
                kv.push(b'=');
                kv.extend_from_slice(v.as_bytes());
                CString::new(kv).ok()
            })
            .collect();
        let mut envp: Vec<*const c_char> = env.iter().map(|s| s.as_ptr()).collect();
        envp.push(btn.as_ptr());
        envp.push(ptr::null());
        let argv: [*const c_char; 4] = [c"sh".as_ptr(), c"-c".as_ptr(), shcmd.as_ptr(), ptr::null()];

        //fork twice so the command is reparented to init and never left as a zombie
        // SAFETY: after fork() the children only make async-signal-safe calls
        // (fork, open, dup2, setsid, execve, _exit) on argv/envp, which point
        // to strings built above; the parent waits for its direct child only.
        unsafe {
            let child = libc::fork();
            if child == 0 {
                if libc::fork() == 0 {
                    let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
                    if devnull != -1 {
                        libc::dup2(devnull, libc::STDOUT_FILENO);
                    }
                    libc::setsid();
                    libc::execve(c"/bin/sh".as_ptr(), argv.as_ptr(), envp.as_ptr());
                    libc::_exit(127);
                }
                libc::_exit(0);
            }
            if child > 0 {
                while libc::waitpid(child, ptr::null_mut(), 0) == -1 && errno() == libc::EINTR {}
            }
        }
    }

    pub fn getsigcmds(&mut self, signal: u32) {
        for i in 0..self.blocks.len() {
            if self.blocks[i].signal == signal {
                self.getcmd(i);
            }
        }
    }

    pub fn setupsignals(&mut self) {
        //signals are queued on a pipe and handled in statusloop
        // SAFETY: pipe() fills the two-int array; fcntl on the fds it returned;
        // sigaction with zeroed, then fully set, structs whose handlers only
        // make async-signal-safe calls.
        unsafe {
            if libc::pipe(self.sigpipe.as_mut_ptr()) == -1 {
                eprintln!("dwmblocksr: pipe: {}", io::Error::last_os_error());
                std::process::exit(1);
            }
            for fd in self.sigpipe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
            }
            SIGPIPE_FD.store(self.sigpipe[1], Ordering::Relaxed);
            SIGPLUS.store(libc::SIGRTMIN(), Ordering::Relaxed);

            /* initialize all real time signals with dummy handler */
            for i in libc::SIGRTMIN()..=libc::SIGRTMAX() {
                signal(i, dummysighandler);
            }

            let mut sa: libc::sigaction = mem::zeroed();
            sa.sa_sigaction = sighandler as *const () as libc::sighandler_t;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            for block in &self.blocks {
                if block.signal > 0 {
                    libc::sigaction(libc::SIGRTMIN() + block.signal as c_int, &sa, ptr::null_mut());
                }
            }
        }
    }

    /// Put the blocks together in statusstr[0], the previous status moves to
    /// statusstr[1]; true if they differ.
    pub fn getstatus(&mut self) -> bool {
        let [str, last] = &mut self.statusstr;
        mem::swap(str, last);
        str.clear();
        for status in &self.statusbar {
            str.extend_from_slice(status.as_bytes());
        }
        if str.len() >= self.delimiter.len() {
            str.truncate(str.len() - self.delimiter.len());
        }
        str != last
    }

    pub fn setroot(&mut self) {
        if !self.getstatus() {
            //Only set root if text has changed.
            return;
        }
        self.statusstr[0].push(0);
        // SAFETY: dpy is the display opened in setupx() (main() exits if that
        // failed) and statusstr[0] is NUL-terminated.
        unsafe {
            XStoreName(self.dpy, self.root, self.statusstr[0].as_ptr() as *const c_char);
            XFlush(self.dpy);
        }
        self.statusstr[0].pop();
    }

    pub fn setupx(&mut self) -> bool {
        // SAFETY: XOpenDisplay(NULL) is always sound; the result is checked for
        // NULL before the screen and root window are read from it.
        unsafe {
            let dpy = XOpenDisplay(ptr::null());
            if dpy.is_null() {
                eprintln!("dwmblocksr: Failed to open display");
                return false;
            }
            self.dpy = dpy;
            self.screen = XDefaultScreen(dpy);
            self.root = XRootWindow(dpy, self.screen);
        }
        true
    }

    pub fn pstdout(&mut self) {
        if !self.getstatus() {
            //Only write out if text has changed.
            return;
        }
        let mut out = io::stdout().lock();
        let _ = out.write_all(&self.statusstr[0]).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush());
    }

    /// The `writestatus` function pointer: setroot, or pstdout with -p.
    pub fn writestatus(&mut self) {
        match self.writestatus {
            WriteStatus::Setroot => self.setroot(),
            WriteStatus::Pstdout => self.pstdout(),
        }
    }

    pub fn statusloop(&mut self) {
        //the signal pipe, plus the output of every running command in async mode
        let n = self.blocks.len();
        let mut pfds: Vec<libc::pollfd> = Vec::with_capacity(n + 1);
        let mut running: Vec<usize> = Vec::with_capacity(n + 1);
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        let mut next = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        let mut i: i32 = 0;
        let mut ev = SigEvent { signal: 0, button: 0 };

        for cmd in &mut self.blockcmds {
            cmd.fd = -1;
        }

        self.getcmds(-1);
        self.writestatus();
        // SAFETY: clock_gettime fills a valid timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut next) };
        next.tv_sec += 1;
        while STATUS_CONTINUE.load(Ordering::Relaxed) {
            if self.r#async {
                //reap finished commands
                // SAFETY: waitpid with a NULL status pointer is allowed.
                while unsafe { libc::waitpid(-1, ptr::null_mut(), libc::WNOHANG) } > 0 {}
            }
            // SAFETY: clock_gettime fills a valid timespec.
            unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
            let timeout = (next.tv_sec - now.tv_sec).saturating_mul(1000) + (next.tv_nsec - now.tv_nsec) / 1_000_000;
            if timeout <= 0 {
                i = i.wrapping_add(1);
                self.getcmds(i);
                self.writestatus();
                // SAFETY: clock_gettime fills a valid timespec.
                unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut next) };
                next.tv_sec += 1;
                continue;
            }
            //wait until the next second, handling signals and output as they come in
            pfds.clear();
            running.clear();
            pfds.push(libc::pollfd { fd: self.sigpipe[0], events: libc::POLLIN, revents: 0 });
            running.push(0);
            if self.r#async {
                for (j, cmd) in self.blockcmds.iter().enumerate() {
                    if cmd.fd != -1 {
                        pfds.push(libc::pollfd { fd: cmd.fd, events: libc::POLLIN, revents: 0 });
                        running.push(j);
                    }
                }
            }
            let timeout = timeout.min(c_int::MAX as libc::time_t) as c_int;
            // SAFETY: pfds holds pfds.len() initialised pollfd structs.
            if unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout) } <= 0 {
                continue;
            }
            if pfds[0].revents != 0 {
                // SAFETY: ev is a plain repr(C) struct of two ints that any
                // bytes are valid for, and the read is at most its size.
                while unsafe { libc::read(self.sigpipe[0], &mut ev as *mut SigEvent as *mut c_void, mem::size_of::<SigEvent>()) }
                    == mem::size_of::<SigEvent>() as isize
                {
                    if ev.signal == 0 {
                        continue; //wakeup from termhandler
                    }
                    if ev.button != 0 {
                        for j in 0..n {
                            if self.blocks[j].signal == ev.signal {
                                self.buttonhandler(j, ev.button);
                            }
                        }
                    } else {
                        self.getsigcmds(ev.signal);
                    }
                }
            }
            for (pfd, &j) in pfds.iter().zip(&running).skip(1) {
                if pfd.revents != 0 {
                    self.readcmd(j);
                }
            }
            self.writestatus();
        }
    }
}

/* this signal handler should do nothing */
extern "C" fn dummysighandler(_signum: c_int) {}

extern "C" fn sighandler(signum: c_int, si: *mut libc::siginfo_t, _ucontext: *mut c_void) {
    //running the commands here isn't async-signal-safe, so just queue the signal.
    //dwm sends the clicked mouse button with sigqueue, a plain kill means update.
    // SAFETY: only async-signal-safe calls: errno is saved and restored, and
    // the kernel's siginfo is read, sival_int being the first 4 bytes of the
    // sigval union; write() goes to the non-blocking signal pipe.
    unsafe {
        let errnop = libc::__errno_location();
        let olderrno = *errnop;
        let mut button = 0;
        if !si.is_null() && (*si).si_code == libc::SI_QUEUE {
            let value = (*si).si_value();
            button = ptr::read_unaligned(&value as *const libc::sigval as *const c_int);
        }
        let ev = SigEvent { signal: signum.wrapping_sub(SIGPLUS.load(Ordering::Relaxed)) as u32, button };
        libc::write(SIGPIPE_FD.load(Ordering::Relaxed), &ev as *const SigEvent as *const c_void, mem::size_of::<SigEvent>());
        *errnop = olderrno;
    }
}

pub extern "C" fn termhandler(_signum: c_int) {
    //also wake up statusloop, in case the signal came just before it started waiting
    // SAFETY: errno is saved and restored; write() to the non-blocking signal
    // pipe is async-signal-safe.
    unsafe {
        let errnop = libc::__errno_location();
        let olderrno = *errnop;
        let ev = SigEvent { signal: 0, button: 0 };
        STATUS_CONTINUE.store(false, Ordering::Relaxed);
        libc::write(SIGPIPE_FD.load(Ordering::Relaxed), &ev as *const SigEvent as *const c_void, mem::size_of::<SigEvent>());
        *errnop = olderrno;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dwmblocks(blocks: Vec<Block>, delim: &str, writestatus: WriteStatus) -> Dwmblocks {
        let config = Config { blocks, ..Config::default() };
        Dwmblocks::new(config, delim.as_bytes().to_vec(), writestatus)
    }

    fn block(icon: &str, signal: u32) -> Block {
        Block { icon: icon.to_string(), command: String::new(), interval: 0, signal }
    }

    #[test]
    fn signal_byte() {
        let d = dwmblocks(vec![], " ", WriteStatus::Setroot);
        let s = d.setblockstatus(&block("^2^ ", 5), b"abc\nsecond line");
        assert_eq!(s.as_bytes(), b"\x05^2^ abc ");
        let s = d.setblockstatus(&block("^2^ ", 0), b"abc");
        assert_eq!(s.as_bytes(), b"^2^ abc ");
    }

    #[test]
    fn pstdout_has_no_signal_byte() {
        let d = dwmblocks(vec![], " ", WriteStatus::Pstdout);
        assert_eq!(d.setblockstatus(&block("^2^ ", 5), b"abc\n").as_bytes(), b"^2^ abc ");
    }

    #[test]
    fn utf8_cut() {
        let d = dwmblocks(vec![], " ", WriteStatus::Setroot);
        /* 97 = CMDLENGTH - delimLen(2) - 1 bytes fit: 96 x and the first byte of é */
        let mut out = vec![b'x'; 96];
        out.extend_from_slice("é".as_bytes());
        let mut want = vec![b'x'; 96];
        want.push(b' ');
        assert_eq!(d.setblockstatus(&block("", 0), &out).as_bytes(), &want[..]);
        /* a 3-byte glyph cut after 2 bytes */
        let mut out = vec![b'x'; 95];
        out.extend_from_slice("\u{f017}".as_bytes());
        let mut want = vec![b'x'; 95];
        want.push(b' ');
        assert_eq!(d.setblockstatus(&block("", 0), &out).as_bytes(), &want[..]);
        /* a whole one is kept */
        let mut out = vec![b'x'; 94];
        out.extend_from_slice("\u{f017}".as_bytes());
        let mut want = out.clone();
        want.push(b' ');
        assert_eq!(d.setblockstatus(&block("", 0), &out).as_bytes(), &want[..]);
    }

    #[test]
    fn status2d_codes_pass_through() {
        let d = dwmblocks(vec![], " ", WriteStatus::Setroot);
        let s = d.setblockstatus(&block("", 6), b"^c#ff0000^^B^\xef\x80\x97^N^ 42%\n");
        assert_eq!(s.as_bytes(), b"\x06^c#ff0000^^B^\xef\x80\x97^N^ 42% ");
    }

    #[test]
    fn empty_block() {
        let d = dwmblocks(vec![], " ", WriteStatus::Setroot);
        assert_eq!(d.setblockstatus(&block("", 5), b"").as_bytes(), b"");
        assert_eq!(d.setblockstatus(&block("", 5), b"\nsecond").as_bytes(), b"");
        assert_eq!(d.setblockstatus(&block("", 0), b"").as_bytes(), b"");
        assert_eq!(d.setblockstatus(&block("^4^", 0), b"").as_bytes(), b"^4^ ");
        /* output stops at a NUL, like the C string */
        assert_eq!(d.setblockstatus(&block("", 0), b"ab\0cd").as_bytes(), b"ab ");
    }

    #[test]
    fn delimiter() {
        let d = dwmblocks(vec![], "", WriteStatus::Setroot);
        assert_eq!(d.setblockstatus(&block("", 0), b"abc").as_bytes(), b"abc");
        /* cut to delimLen (5) bytes */
        let d = dwmblocks(vec![], " | ab | ", WriteStatus::Setroot);
        assert_eq!(d.setblockstatus(&block("", 0), b"abc").as_bytes(), b"abc | ab");
        /* the output limit shrinks with the delimiter: 100 - 6 - 1 = 93 bytes */
        let out = vec![b'x'; 200];
        assert_eq!(d.setblockstatus(&block("", 0), &out).as_bytes().len(), 93 + 5);
    }

    #[test]
    fn getstatus() {
        let mut d = dwmblocks(vec![block("", 1), block("", 2), block("", 3)], " ", WriteStatus::Pstdout);
        assert!(!d.getstatus()); /* "" is what was there */
        d.statusbar[0] = d.setblockstatus(&block("", 0), b"a");
        d.statusbar[2] = d.setblockstatus(&block("", 0), b"c");
        assert!(d.getstatus());
        assert_eq!(d.statusstr[0], b"a c"); /* trailing delimiter removed */
        assert!(!d.getstatus());
        assert_eq!(d.statusstr[0], b"a c");
        d.statusbar[1] = d.setblockstatus(&block("", 0), b"b");
        assert!(d.getstatus());
        assert_eq!(d.statusstr[0], b"a b c");
        /* no delimiter: nothing removed */
        let mut d = dwmblocks(vec![block("", 1)], "", WriteStatus::Pstdout);
        d.statusbar[0] = d.setblockstatus(&block("", 0), b"ab");
        assert!(d.getstatus());
        assert_eq!(d.statusstr[0], b"ab");
    }
}
