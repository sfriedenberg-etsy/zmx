# zmx is being rewritten in Rust as part of `posh`

zmx and mosh are being combined into a single Rust tool, **posh**, which
provides both local session persistence (zmx's role) and roaming remote
sessions over encrypted UDP (mosh's role).

The rewrite lives in the mosh fork (to be renamed `posh`):

- Repository: https://github.com/amarbel-llc/mosh
- Branch: `claude/posh-rust-rewrite-cqribr`

Layout there:

- `crates/posh-term` — standalone terminal emulation library (a from-scratch
  Rust rewrite of the ghostty-vt core, targeting kitty feature parity).
- `crates/posh` — the combined CLI: zmx's daemon-per-session architecture and
  Unix-socket IPC, plus mosh's encrypted datagram transport and state sync.

The Zig implementation in this repository remains as the reference for the
session-persistence behavior being ported.
