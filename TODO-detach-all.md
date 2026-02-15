# TODO: Add `detach-all` and `detach <session>` CLI commands

## Motivation

The pomodoro break timer (`pomodoro.fish`) needs to detach all zmx sessions
before starting a break. Currently it does this by manually sending the
5-byte `DetachAll` IPC message (`\x04\x00\x00\x00\x00`) to each socket via
`socat`. This is fragile and couples external tools to zmx's wire protocol.

## Proposed CLI

```
zmx detach-all              # detach all clients from all sessions
zmx detach <session>        # detach all clients from a specific session
```

These commands should iterate the zmx socket directory and send the
appropriate IPC messages, handling socket discovery and error cases
internally.

## Benefits

- Decouples external tools from zmx's IPC wire format
- Enables scripting without needing `socat` + raw byte sequences
- Natural complement to existing `zmx attach` command
