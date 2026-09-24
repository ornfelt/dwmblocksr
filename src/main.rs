/* See LICENSE file for copyright and license details.
 *
 * dwmblocksr - modular status bar for dwm, a Rust port of dwmblocks.
 *
 * To understand everything else, start reading Dwmblocks::statusloop() in
 * dwmblocks.rs.
 */
mod config;
mod dwmblocks;

use std::os::unix::ffi::OsStrExt;

use x11::xlib::{XCloseDisplay, XStoreName};

use crate::dwmblocks::{signal, termhandler, Dwmblocks, WriteStatus};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    /* Rust ignores SIGPIPE, and the block commands would inherit that; restore
     * the default like a C program has it */
    // SAFETY: setting a signal's disposition to SIG_DFL has no preconditions.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let mut delimiter: Option<Vec<u8>> = None; //delim from the config, or the -d argument
    let mut writestatus = WriteStatus::Setroot;
    let mut i = 1;
    while i < args.len() {
        //Handle command line arguments
        if args[i] == "-d" && i + 1 < args.len() {
            i += 1;
            delimiter = Some(args[i].as_bytes().to_vec());
        } else if args[i] == "-p" {
            writestatus = WriteStatus::Pstdout;
        } else if args[i] == "-v" {
            eprintln!("dwmblocksr-{}", VERSION);
            std::process::exit(1);
        }
        i += 1;
    }
    let config = config::load();
    let delimiter = delimiter.unwrap_or_else(|| config.delim.as_bytes().to_vec());
    let mut d = Dwmblocks::new(config, delimiter, writestatus);
    if !d.setupx() {
        std::process::exit(1);
    }
    d.setupsignals();
    signal(libc::SIGTERM, termhandler);
    signal(libc::SIGINT, termhandler);
    d.statusloop();
    //clear the status so dwm doesn't keep showing stale blocks (e.g. a clock that
    //stopped), an empty status makes dwm fall back to its "dwm-<version>" text
    // SAFETY: dpy is the display opened in setupx(); nothing uses it afterwards.
    unsafe {
        if d.writestatus == WriteStatus::Setroot {
            XStoreName(d.dpy, d.root, c"".as_ptr());
        }
        XCloseDisplay(d.dpy);
    }
}
