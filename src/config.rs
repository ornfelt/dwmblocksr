/* See LICENSE file for copyright and license details. */
//! Configuration.
//!
//! The compiled-in defaults ([`Config::default`]) are dwmblocks' blocks.def.h
//! plus the `delim`/`delimLen` values and the `ASYNC` build flag. At startup
//! `$XDG_CONFIG_HOME/dwmblocksr/config.toml` (usually
//! `~/.config/dwmblocksr/config.toml`) is read on top of them; every key in it
//! is optional and falls back to the default. A config file that cannot be
//! parsed is reported on stderr and ignored so that the status bar still
//! starts.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::dwmblocks::CMDLENGTH;

/// One block of the status bar, like dwmblocks' `Block`.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub icon: String,
    pub command: String,
    pub interval: u32,
    pub signal: u32,
    /// Some(true) keeps the block only with a battery, Some(false) only
    /// without one (compile.sh's sb-battery/sb-internet swap); None always.
    pub battery: Option<bool>,
}

#[derive(Debug)]
pub struct Config {
    pub blocks: Vec<Block>,
    /// sets delimiter between status commands. "" means no delimiter.
    pub delim: String,
    /// the most bytes of the delimiter that are used (`delimLen`)
    pub delimlen: usize,
    /// true runs the block commands in parallel, so a slow command (e.g. one
    /// waiting on the network) can't hold up the other blocks, signals and
    /// clicks; false runs them one at a time like upstream dwmblocks (`ASYNC`)
    pub r#async: bool,
}

fn block(icon: &str, command: &str, interval: u32, signal: u32) -> Block {
    Block { icon: icon.to_string(), command: command.to_string(), interval, signal, battery: None }
}

/// A block used only with (`present`) or without a battery.
fn ifbattery(present: bool, b: Block) -> Block {
    Block { battery: Some(present), ..b }
}

/// compile.sh's test: is there a /sys/class/power_supply/BAT?* entry?
pub fn hasbattery() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    dir.flatten().any(|e| e.file_name().as_encoded_bytes().strip_prefix(b"BAT").is_some_and(|rest| !rest.is_empty()))
}

impl Default for Config {
    /// blocks.def.h, with compile.sh's sb-battery/sb-internet swap as a
    /// pair of battery blocks
    fn default() -> Self {
        Config {
            blocks: vec![
                /*     Icon                  Command                                           Update Interval  Update Signal */
                /* block("^c1^",             "~/.local/bin/my_scripts/spotify_dwmblocks.sh", 5,               12), */
                block("",                    "~/.local/bin/my_scripts/spotify_dwmblocks.sh", 5,               12),
                block("",                    "~/.local/bin/statusbar/sb-claude",             30,              6),
                /* net down/up, memory and cpu, shown/hidden with mod-ctrl-p (sb-sysinfo toggle), a click shows details */
                block("",                    "~/.local/bin/statusbar/sb-sysinfo net",        2,               13),
                block("",                    "~/.local/bin/statusbar/sb-sysinfo mem",        2,               14),
                block("",                    "~/.local/bin/statusbar/sb-sysinfo cpu",        2,               15),
                block("^2^\u{f0c2}  ",       "~/.local/bin/statusbar/weather",               1800,            5),
                block("^3^ \u{f2c8} ",       "~/.local/bin/statusbar/cputemp",               5,               4),
                block("^4^ ",                "~/.local/bin/statusbar/sb-volume",             0,               10),
                /* without a battery, sb-internet takes sb-battery's place */
                ifbattery(false, block("^5^ ", "~/.local/bin/statusbar/sb-internet",         5,               3)),
                ifbattery(true, block("^5^ ",  "~/.local/bin/statusbar/sb-battery",          5,               3)),
                block("^6^ \u{f017} ",       "~/.local/bin/statusbar/sb-clock",              5,               1),
            ],
            delim: " ".to_string(),
            delimlen: 5,
            r#async: true,
        }
    }
}

/* ---------------------------------------------------------------------- */
/* config file loading                                                    */

/// `$XDG_CONFIG_HOME/dwmblocksr/config.toml`, or `$HOME/.config/dwmblocksr/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("dwmblocksr").join("config.toml"))
}

impl Config {
    /// Drop the blocks that are not for this machine: `battery = true`
    /// blocks without a battery, `battery = false` blocks with one.
    pub fn selectblocks(&mut self, battery: bool) {
        self.blocks.retain(|b| b.battery.is_none_or(|want| want == battery));
    }
}

/// Load the configuration: the config file if it exists and is valid, the
/// built-in defaults otherwise, with the blocks for this machine.
pub fn load() -> Config {
    let mut config = loadfile();
    config.selectblocks(hasbattery());
    config
}

fn loadfile() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    if !path.is_file() {
        return Config::default();
    }
    let result = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| parse(&text).map_err(String::from));
    match result {
        Ok(config) => config,
        Err(err) => {
            eprintln!("dwmblocksr: {}: {}", path.display(), err);
            eprintln!("dwmblocksr: using the built-in default configuration");
            Config::default()
        }
    }
}

#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<ConfigError> for String {
    fn from(e: ConfigError) -> String {
        e.0
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(msg.into()))
}

/* raw TOML shape */

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    blocks: Option<Vec<RawBlock>>,
    delim: Option<String>,
    delimlen: Option<usize>,
    r#async: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlock {
    #[serde(default)]
    icon: String,
    command: String,
    #[serde(default)]
    interval: u32,
    #[serde(default)]
    signal: u32,
    battery: Option<bool>,
}

/// Parse a config file on top of the defaults, rejecting values that the C
/// code would overflow a buffer or send a wrong signal with.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
    let mut config = Config::default();

    if let Some(delim) = raw.delim {
        if delim.contains('\0') {
            return err("delim: must not contain a NUL character");
        }
        config.delim = delim;
    }
    if let Some(delimlen) = raw.delimlen {
        config.delimlen = delimlen;
    }
    if let Some(r#async) = raw.r#async {
        config.r#async = r#async;
    }
    if let Some(blocks) = raw.blocks {
        config.blocks = blocks
            .into_iter()
            .map(|b| Block { icon: b.icon, command: b.command, interval: b.interval, signal: b.signal, battery: b.battery })
            .collect();
    }

    /* the signal byte, the icon and the delimiter with its NUL must fit in a
     * block's CMDLENGTH bytes */
    if config.delimlen + 3 > CMDLENGTH {
        return err(format!("delimlen: must be at most {}", CMDLENGTH - 3));
    }
    let sigmax = libc::SIGRTMAX() - libc::SIGRTMIN();
    /* a block's signal is also the byte dwm reads to tell the blocks apart,
     * so it has to be a control character (< ' ') */
    let sigmax = sigmax.clamp(0, 31) as u32;
    for (i, b) in config.blocks.iter().enumerate() {
        if b.icon.contains('\0') || b.command.contains('\0') {
            return err(format!("blocks[{}]: icon and command must not contain a NUL character", i));
        }
        if 1 + b.icon.len() + config.delimlen + 1 > CMDLENGTH {
            return err(format!("blocks[{}]: icon is {} bytes, at most {} fit with delimlen {}", i, b.icon.len(), CMDLENGTH - 2 - config.delimlen, config.delimlen));
        }
        if b.signal > sigmax {
            return err(format!("blocks[{}]: signal {} is out of range (0..={})", i, b.signal, sigmax));
        }
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_TOML: &str = include_str!("../config/config.toml");

    #[test]
    fn empty_file_is_default() {
        let c = parse("").unwrap();
        let d = Config::default();
        assert_eq!(c.blocks, d.blocks);
        assert_eq!((c.delim, c.delimlen, c.r#async), (d.delim, d.delimlen, d.r#async));
    }

    /// The shipped config/config.toml must be exactly blocks.def.h.
    #[test]
    fn shipped_config_matches_defaults() {
        let c = parse(DEFAULT_TOML).expect("config/config.toml parses");
        let d = Config::default();
        assert_eq!(c.blocks, d.blocks);
        assert_eq!(c.delim, d.delim);
        assert_eq!(c.delimlen, d.delimlen);
        assert_eq!(c.r#async, d.r#async);
    }

    /// The icons are the Nerd Font glyphs of blocks.h, byte for byte.
    #[test]
    fn icons_match_blocks_h() {
        let d = Config::default();
        assert_eq!(d.blocks[5].icon.as_bytes(), b"^2^\xef\x83\x82  ");
        assert_eq!(d.blocks[6].icon.as_bytes(), b"^3^ \xef\x8b\x88 ");
        assert_eq!(d.blocks[10].icon.as_bytes(), b"^6^ \xef\x80\x97 ");
    }

    /// compile.sh: sb-battery with a battery, sb-internet without one.
    #[test]
    fn battery_blocks() {
        let command = |battery| {
            let mut c = Config::default();
            c.selectblocks(battery);
            assert_eq!(c.blocks.len(), 10);
            c.blocks.iter().map(|b| b.command.clone()).collect::<Vec<_>>()
        };
        assert!(command(true).iter().any(|c| c.ends_with("sb-battery")));
        assert!(!command(true).iter().any(|c| c.ends_with("sb-internet")));
        assert!(command(false).iter().any(|c| c.ends_with("sb-internet")));
        assert!(!command(false).iter().any(|c| c.ends_with("sb-battery")));
        let mut c = parse("blocks = [ { command = \"a\" }, { command = \"b\", battery = false } ]").unwrap();
        c.selectblocks(true);
        assert_eq!(c.blocks, vec![block("", "a", 0, 0)]);
    }

    #[test]
    fn partial_config() {
        let c = parse("async = false\ndelim = \" | \"\nblocks = [ { command = \"date\", interval = 1 } ]").unwrap();
        assert!(!c.r#async);
        assert_eq!(c.delim, " | ");
        assert_eq!(c.delimlen, 5);
        assert_eq!(c.blocks, vec![block("", "date", 1, 0)]);
    }

    #[test]
    fn rejects_bad_values() {
        assert!(parse("nonsense = 1").is_err());
        assert!(parse("blocks = [ { icon = \"x\" } ]").is_err()); /* no command */
        assert!(parse("blocks = [ { command = \"x\", bogus = 1 } ]").is_err());
        assert!(parse("blocks = [ { command = \"x\", signal = 32 } ]").is_err());
        assert!(parse("blocks = [ { command = \"x\", interval = -1 } ]").is_err());
        assert!(parse("blocks = [ { command = \"x\\u0000\" } ]").is_err());
        assert!(parse("delim = \"\\u0000\"").is_err());
        assert!(parse("delimlen = 98").is_err());
        let long = "x".repeat(CMDLENGTH);
        assert!(parse(&format!("blocks = [ {{ command = \"x\", icon = \"{}\" }} ]", long)).is_err());
        let fits = "x".repeat(CMDLENGTH - 2 - 5);
        assert!(parse(&format!("blocks = [ {{ command = \"x\", icon = \"{}\" }} ]", fits)).is_ok());
    }
}
