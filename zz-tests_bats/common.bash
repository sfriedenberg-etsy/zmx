#! /bin/bash -e
#
# zz-tests_bats/common.bash — load from every .bats file's setup():
#
#   setup() {
#     load "$(dirname "$BATS_TEST_FILE")/common.bash"
#     setup_test_home
#   }

if [[ -z $BATS_TEST_TMPDIR ]]; then
  echo 'common.bash loaded before $BATS_TEST_TMPDIR set. aborting.' >&2
  exit 1
fi

pushd "$BATS_TEST_TMPDIR" >/dev/null || exit 1

bats_load_library bats-support
bats_load_library bats-assert
bats_load_library bats-emo
bats_load_library bats-island

require_bin ZMX_BIN zmx

# Wraps zmx invocation with a per-call timeout. BATS_TEST_TIMEOUT caps
# the whole @test block; this caps a single invocation. Bump only if a
# specific call legitimately needs more.
run_zmx() {
  local bin="${ZMX_BIN:-zmx}"
  run timeout --preserve-status 2s "$bin" "$@"
}

# --- raw IPC helpers for integration tests ------------------------------------
#
# A zmx client is just a socket connection that speaks the binary protocol, so
# tests can drive "fake" clients with socat (no PTY allocation). The wire format
# is an 8-byte Header { tag:u8, len:u32 LE, 3 pad bytes } followed by `len`
# payload bytes (see src/ipc.rs).

# Render a u16 as two little-endian \xNN printf escapes.
zmx_le16() { printf '\\x%02x\\x%02x' "$(( $1 & 255 ))" "$(( ($1 >> 8) & 255 ))"; }

# Print (to stdout) a size message with tag $1 carrying Resize{rows=$2, cols=$3}:
# an 8-byte Header { tag, len=4 } + 4-byte Resize payload.
zmx_size_msg() {
  printf '%b' "$(printf '\\x%02x' "$1")\\x04\\x00\\x00\\x00\\x00\\x00\\x00$(zmx_le16 "$2")$(zmx_le16 "$3")"
}

# Init (tag 0x07) and Resize (tag 0x02) messages carrying rows/cols.
zmx_init_msg()   { zmx_size_msg 7 "$1" "$2"; }
zmx_resize_msg() { zmx_size_msg 2 "$1" "$2"; }

# Print (to stdout) an Input message (tag 0x00) carrying the bytes of $1 (run
# through printf %b so escapes like \n work). The payload must not contain NUL
# bytes (bash strings can't hold them), which is fine for typed input.
zmx_input_msg() {
  local data len
  data="$(printf '%b' "$1")"
  len=${#data}
  printf '%b' "\\x00$(printf '\\x%02x\\x%02x\\x%02x\\x%02x' "$(( len & 255 ))" "$(( (len >> 8) & 255 ))" "$(( (len >> 16) & 255 ))" "$(( (len >> 24) & 255 ))")\\x00\\x00\\x00"
  printf '%s' "$data"
}

# Print (to stdout) a no-payload control message with tag $1 — just the 8-byte
# Header { tag, len=0 }. E.g. Detach (0x03), DetachAll (0x04), Kill (0x05).
zmx_ctrl_msg() {
  printf '%b' "$(printf '\\x%02x' "$1")\\x00\\x00\\x00\\x00\\x00\\x00\\x00"
}
