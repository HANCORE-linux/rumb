PREFIX ?= /usr/local

all:
	cargo build --release --locked --manifest-path tui/Cargo.toml --target-dir tui/target

install:
	install -Dm755 tui/target/release/rumb "$(DESTDIR)$(PREFIX)/bin/rumb"
	install -Dm755 bin/rumb-scan "$(DESTDIR)$(PREFIX)/bin/rumb-scan"
	install -Dm644 man/rumb.1 "$(DESTDIR)$(PREFIX)/share/man/man1/rumb.1"
	install -Dm644 tui/assets/OFL.txt "$(DESTDIR)$(PREFIX)/share/licenses/rumb/OFL-JetBrainsMono.txt"

uninstall:
	rm -f "$(DESTDIR)$(PREFIX)/bin/rumb" "$(DESTDIR)$(PREFIX)/bin/rumb-scan" "$(DESTDIR)$(PREFIX)/share/man/man1/rumb.1"
	rm -f "$(DESTDIR)$(PREFIX)/share/licenses/rumb/OFL-JetBrainsMono.txt"

.PHONY: all install uninstall
