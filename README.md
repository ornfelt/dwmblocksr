# dwmblocksr - modular status bar for dwm

dwmblocksr is a port of [dwmblocks](https://github.com/torrinfail/dwmblocks)
to Rust, in the same way dwmr is a port of dwm: a
function-for-function replica with the same behaviour, including this fork's
additions (asynchronous commands, clickable blocks with `BLOCK_BUTTON`,
UTF-8-safe truncation and clearing the status on exit). The one difference is
that the blocks are read from a config file at startup instead of being
compiled in, so changing them only needs a restart.

It is a drop-in replacement for dwmblocks: same status text, same signal
bytes before the blocks, same click protocol (dwm sends SIGRTMIN+n with the
button as the `sigqueue` value), and the same `BLOCK_BUTTON` for the scripts.

## Requirements

- Rust (cargo) and a C toolchain for linking
- Xlib headers (Debian: `libx11-dev`, plus `pkg-config`)

## Installation

    make                    # cargo build --release
    sudo make install       # /usr/local/bin/dwmblocksr and the man page
    make install-config     # copy the default config to ~/.config/dwmblocksr/config.toml

Like dwmblocks' `compile.sh`, `make install-config` uses the `sb-internet`
block instead of `sb-battery` when there is no `/sys/class/power_supply/BAT*`.

## Running

Start it before dwm/dwmr, e.g. in `~/.xinitrc`:

    dwmblocksr &

`dwmblocksr -d <delim>` overrides the delimiter, `dwmblocksr -p` writes the
status to stdout instead of the root window name, `dwmblocksr -v` prints the
version.

## Configuration

dwmblocksr reads `$XDG_CONFIG_HOME/dwmblocksr/config.toml`, i.e.
`~/.config/dwmblocksr/config.toml`. The file mirrors blocks.def.h; every key
is optional and falls back to the built-in default. A file that fails to
parse is reported on stderr and ignored. See `config/config.toml` for the
commented default.

    blocks = [
        { icon = "^6^  ", command = "~/.local/bin/statusbar/sb-clock", interval = 5, signal = 1 },
    ]
    delim = " "       # "" for none
    delimlen = 5      # at most this many bytes of delim are used
    async = true      # false runs the commands one at a time (dwmblocks' ASYNC=0)

- Commands run with `/bin/sh -c`, so `~` and shell syntax work. Only the
  first line of the output is used, cut to fit in 100 bytes per block
  (signal byte, icon and delimiter included).
- `interval` is in seconds; 0 means the block only updates on its signal.
- `signal` n updates the block on SIGRTMIN+n (`pkill -RTMIN+n dwmblocksr`)
  and makes it clickable; 0 means no signal. It has to be at most 30.
- Status2d codes (`^2^`, `^c#rrggbb^`, `^B^`...`^N^`) in icons and output
  are passed to dwm unchanged.

## Scripts

A click runs the block's command with `BLOCK_BUTTON=<button>` and then
updates the block. Scripts that signal the status bar themselves have to use
the new process name, e.g. `pkill -RTMIN+5 -x 'dwmblocksr?'`, which matches
both dwmblocks and dwmblocksr.

## Development

`cargo test` runs the unit tests; see `.claude/skills/dwmblocksr-dev/SKILL.md`
for how the C code maps onto the Rust code and how to test on Xvfb.
