---
name: dwmblocksr-dev
description: How to add features, port dwmblocks changes and fix bugs in dwmblocksr, the Rust port of dwmblocks. Use for any change to this repo.
---

# dwmblocksr development

## What this project is

dwmblocksr is a port of the user's dwmblocks fork (`~/.config/dwmblocks`,
C) to Rust. The executable is called `dwmblocksr`. It is a
function-for-function replica of dwmblocks.c: same behaviour, same logic,
same structure, same function names, and the same comment style, as far as
Rust allows. The fork adds to upstream dwmblocks: asynchronous commands
(`ASYNC`), click handling with `BLOCK_BUTTON` (the statuscmd protocol: dwm
sends SIGRTMIN+n with the button as the `sigqueue` value), UTF-8-safe
truncation, and clearing the root name on exit. The one addition over the C
code is that the blocks are read at startup from
`$XDG_CONFIG_HOME/dwmblocksr/config.toml` on top of compiled-in defaults
that equal blocks.def.h.

It must stay a drop-in replacement: the same status text, the same signal
byte before each block, the same click protocol and the same `BLOCK_BUTTON`
for the scripts in `~/.local/bin/statusbar` and `~/.local/bin/my_scripts`.

The priorities, in order (the same as dwmr's):

1. **Safety**: dwmblocksr must never crash. No panics on any input (command
   output, config file, signals): no `unwrap()`/`expect()` on runtime data,
   no slicing that can go out of bounds, no integer overflow in debug builds.
   Bad config values are rejected at load time with a message, and the
   defaults are used. Between `fork()` and exec only async-signal-safe calls
   (no allocation, no locks, no `println!`): build argv/envp `CString`s
   before forking. Signal handlers only `write()` to the self-pipe and
   save/restore errno; everything they read is a static atomic.
2. **Speed and memory efficiency**: fixed-size, reused buffers
   (`BlockStatus`, `BlockCmd.out`, `statusstr` preallocated to
   STATUSLENGTH, the poll arrays), no allocation per update. A click may
   allocate (it builds the environment for the command).
3. **Fidelity**: same logic and structure as dwmblocks.c, with Rust naming
   (snake_case, `bool` for flags, `Option` for NULL, byte slices for
   `char*` since command output need not be UTF-8). Function names stay
   dwmblocks' (`setblockstatus`, `getcmds`, `statusloop`), lower-cased
   (`setupX` is `setupx`).

## Where things are

| dwmblocks file      | dwmblocksr file       | notes |
|---------------------|-----------------------|-------|
| dwmblocks.c         | `src/dwmblocks.rs`    | `struct Dwmblocks` holds every global; every C function is a method (or, for signal handlers, a free `extern "C" fn`) in the same order |
| dwmblocks.c main()  | `src/main.rs`         | argument handling, setup, exit (clear root name, close display) |
| blocks.def.h        | `src/config.rs`       | `Config::default()` is blocks.def.h plus `delim`, `delimLen` and `ASYNC`; the rest is the TOML loader |
| blocks.h            | `config/config.toml`  | the shipped default config; must equal `Config::default()` (unit test `shipped_config_matches_defaults`) |
| Makefile, compile.sh | `Makefile`           | `make`, `sudo make install`, `make install-config` (does compile.sh's sb-battery → sb-internet swap without a battery) |
| (none)              | `dwmblocksr.1`        | man page |

Reference C source: `~/.config/dwmblocks/dwmblocks.c`.

## How C constructs are represented

- **Globals** (`statusbar`, `statusstr`, `delimiter`, `blockcmds`, `dpy`,
  ...) are fields of `Dwmblocks`. What the signal handlers need is static:
  `SIGPIPE_FD` (write end of `sigpipe`), `STATUS_CONTINUE`
  (`statusContinue`) and `SIGPLUS` (`SIGRTMIN`, read once in
  `setupsignals`, since `libc::SIGRTMIN()` is a libc call).
- **Compile-time switches** became config: `ASYNC` is `config.async`
  (`getcmd` dispatches to `getcmd_async`/`getcmd_sync`, the two C variants);
  `NO_X` and the OpenBSD paths are not ported.
- **`writestatus`** (function pointer `setroot`/`pstdout`) is the
  `WriteStatus` enum plus the `writestatus()` method.
- **`char[CMDLENGTH]` buffers** are `BlockStatus { buf, len }` and
  `BlockCmd.out`; C string ends (`'\0'`) are slice lengths, and a NUL in
  command output still ends the text like in C.
- **`setblockstatus(block, output, cmdout)`** returns the new
  `BlockStatus`; the caller stores it in `statusbar[i]` (the borrow checker
  would not allow `&mut self.statusbar[i]` next to `&self`).
- **`delimLen`** keeps C's meaning after main(): the used delimiter length
  plus 1 (the NUL), so the output limit `CMDLENGTH - delimLen - 1` is the
  same. `Dwmblocks::new` does main()'s cutting of the delimiter.
- **popen/fgets/pclose** (sync mode) are `startcmd` + a read loop up to the
  first newline or 99 bytes + close/waitpid.
- **setenv in the forked child** (buttonhandler) became an `envp` built
  before `fork()` and `execve`.
- **`signal()`** is a wrapper over `sigaction` with `SA_RESTART`, like
  glibc's. `main()` resets SIGPIPE to SIG_DFL, because Rust ignores it and
  the block commands would inherit that.
- **Unsafe**: every libc/Xlib call sits in an `unsafe` block with a
  `// SAFETY:` comment. Keep them small; do the Rust logic outside.

## Changing the config

If a blocks.def.h/Makefile setting is added, do all of:
- `Config` + `Config::default()` in `src/config.rs`,
- `RawConfig` and the conversion/validation in `parse()` (reject values the
  C code would overflow a buffer with: the signal byte, icon and delimiter
  must fit in CMDLENGTH; signals must be ≤ SIGRTMAX-SIGRTMIN and < 32),
- `config/config.toml` (same value, commented),
- `dwmblocksr.1` and `README.md`.
`~/.config/dwmblocksr/config.toml` is the user's copy; do not edit it unless
asked, but tell the user what to add.

## Verifying a change (always, before reporting done)

```sh
cargo build && cargo clippy --all-targets   # must be warning-free
cargo test                                   # config and setblockstatus/getstatus unit tests
cargo build --release                        # what `make install` installs
```

Functional test on a virtual X server (no root needed):

```sh
apt-get download xvfb && dpkg -x xvfb_*.deb xvfb   # in a scratch dir
./xvfb/usr/bin/Xvfb :98 -screen 0 800x600x24 -nolisten tcp >/dev/null 2>&1 &
DISPLAY=:98 XDG_CONFIG_HOME=$PWD/xdg ./target/release/dwmblocksr >log 2>&1 &
```

- Read the root name with a tiny C program (`XFetchName` on the root window,
  `fputs` to stdout, `cc ... -lX11`) and `od -c` it: signal bytes show as
  `\001`...; `xprop` escapes too much to compare bytes.
- Compare against the C program: build `~/.config/dwmblocks/dwmblocks.c`
  in a scratch dir with a `blocks.h` equal to your test config
  (`-DASYNC=1` and `-DASYNC=0`) and check the root names and `-p` output
  are identical (in async mode only the final `-p` line is deterministic).
- Update: `kill -$((34+n)) <pid>` (SIGRTMIN is 34 with glibc; dash's
  `kill -l RTMIN` does not work). Click: a small C program calling
  `sigqueue(pid, SIGRTMIN+n, (union sigval){.sival_int = button})`; the
  command should see `BLOCK_BUTTON` and the block update afterwards.
- SIGTERM/SIGINT must exit 0 and leave an empty root name; no zombie
  children (`ps --ppid <pid> -o stat=`).
- Run test processes with stdout/stderr redirected to files, kill by PID.

## Install and run

`make` builds, `sudo make install` installs `/usr/local/bin/dwmblocksr` and
the man page (cargo runs as `$SUDO_USER`), `make install-config` copies the
default config to `~/.config/dwmblocksr/` if none exists. dwmr's
`statusbar` config option names the program to signal on clicks.

## Committing

After a change passes the checks above, commit it (unless told otherwise)
with a short summary line and a body saying what and why. No
`Co-Authored-By` line. The user's `~/.config/dwmblocksr/config.toml` is
never part of a commit.
