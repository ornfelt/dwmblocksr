# dwmblocksr - modular status bar for dwm (Rust port of dwmblocks)
# See LICENSE file for copyright and license details.

VERSION = 1.0.0

# paths
PREFIX = /usr/local
MANPREFIX = ${PREFIX}/share/man

CARGO = cargo
BIN = target/release/dwmblocksr
USERHOME = ${HOME}

# cargo/rustup toolchains are per user, so under `sudo make install` build
# (and find the config dir) as the invoking user, not as root
ifneq (${SUDO_USER},)
CARGO = sudo -u ${SUDO_USER} -H cargo
USERHOME = $(shell getent passwd ${SUDO_USER} | cut -d: -f6)
endif
CONFDIR = ${USERHOME}/.config/dwmblocksr

all: ${BIN}

${BIN}: Cargo.toml src/*.rs
	${CARGO} build --release

test:
	${CARGO} test

clean:
	${CARGO} clean

install: all
	mkdir -p ${DESTDIR}${PREFIX}/bin
	cp -f ${BIN} ${DESTDIR}${PREFIX}/bin/dwmblocksr
	chmod 755 ${DESTDIR}${PREFIX}/bin/dwmblocksr
	mkdir -p ${DESTDIR}${MANPREFIX}/man1
	sed "s/VERSION/${VERSION}/g" < dwmblocksr.1 > ${DESTDIR}${MANPREFIX}/man1/dwmblocksr.1
	chmod 644 ${DESTDIR}${MANPREFIX}/man1/dwmblocksr.1

# copy the default config to ~/.config/dwmblocksr unless one is already there;
# dwmblocksr itself picks sb-battery or sb-internet at startup (the blocks'
# battery key), which dwmblocks' compile.sh did at build time
install-config:
	mkdir -p ${CONFDIR}
	[ -e ${CONFDIR}/config.toml ] || cp config/config.toml ${CONFDIR}/config.toml

uninstall:
	rm -f ${DESTDIR}${PREFIX}/bin/dwmblocksr\
		${DESTDIR}${MANPREFIX}/man1/dwmblocksr.1

.PHONY: all test clean install install-config uninstall
