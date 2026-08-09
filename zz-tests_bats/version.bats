# bats file_tags=version

setup() {
  load "$(dirname "$BATS_TEST_FILE")/common.bash"
  setup_test_home
}

@test "version subcommand prints zmx component" {
  run_zmx version
  assert_success
  assert_line --partial "COMPONENT"
  assert_line --partial "VERSION"
  assert_line --partial "REV"
  assert_line --partial "zmx"
}

@test "v alias prints version" {
  run_zmx v
  assert_success
  assert_line --partial "zmx"
}

@test "--version alias prints version" {
  run_zmx --version
  assert_success
  assert_line --partial "zmx"
}

@test "version reports the built-in vt backend" {
  run_zmx version
  assert_success
  assert_line --partial "zmx-vt"
}
