# Refactoring Rationale: Swappable Terminal Backend Abstraction

## Motivation

The ghostty-vt dependency was refactored behind an abstraction layer for three key reasons:

1. **Dependency Flexibility**: The full Ghostty terminal emulator (ghostty-vt) is a substantial dependency. For lightweight deployments or environments where Ghostty isn't available, an alternative backend (libvterm) provides the same core functionality with a smaller footprint.

2. **Build-time Selection**: Users can choose at compile time which backend to use via `-Dbackend=ghostty|libvterm`, ensuring only the selected backend's dependencies are linked.

3. **Testing & Portability**: The abstraction enables easier testing of session persistence logic independent of any specific terminal emulator implementation.

## Architecture Overview

The refactoring introduces compile-time (comptime) polymorphism:

```
src/terminal.zig          - Generic Terminal(Impl) and VtStream(Impl) wrappers
src/backends/mod.zig      - Backend registry, exports DefaultTerminal based on build option
src/backends/ghostty.zig  - GhosttyBackend implementation
src/backends/libvterm.zig - LibvtermBackend implementation (alternative)
```

The `Terminal(comptime Impl: type)` pattern provides zero-cost abstraction - no vtables or runtime dispatch overhead.

---

## Code Style: Alignment with Upstream Ghostty

### Conventions Followed

| Convention | Status | Notes |
|------------|--------|-------|
| PascalCase for types | Yes | `GhosttyBackend`, `StreamImpl`, `Terminal`, `Cursor` |
| snake_case for functions/variables | Yes | `nextSlice`, `serializeState`, `pending_wrap` |
| Doc comments with `///` | Yes | All public types and functions documented |
| Error handling with try/catch | Yes | Consistent use throughout |
| Deferred cleanup with `defer` | Yes | `defer builder.deinit()` pattern |
| Explicit allocator passing | Yes | Allocators passed as function parameters |

### Conventions NOT Fully Followed

| Convention | Issue | Location |
|------------|-------|----------|
| `@branchHint` for fast paths | Missing | No branch hints in hot paths |
| Assertions for integrity | Missing | No `assertIntegrity()` or similar defensive checks |
| Inline "why" comments | Partial | Some functions lack rationale comments |
| Error context in panics | Missing | No `@panic()` for unrecoverable states |
| Packed struct annotations | N/A | Not applicable to current structs |

---

## Differences from Ghostty Patterns

1. **No `ReadonlyStream` concept**: The `VtStream` wrapper is mutable. Ghostty's `vtStream()` returns a readonly variant for safety.

2. **Simplified formatter options**: `GhosttyBackend.serialize()` hardcodes certain formatter flags (e.g., `palette = false`, `tabstops = false`) rather than exposing them as parameters.

3. **Error suppression in serialize functions**: Errors are logged and `null` returned instead of propagating - this differs from Ghostty's typical error handling where errors bubble up.

4. **Missing screen selection**: The serialization always uses `.all` screen content without allowing callers to select primary vs alternate screen.

---

## TODO: Items to Address for Upstream Consistency

Before submitting upstream, consider addressing these items to better align with Ghostty's codebase style:

### High Priority

- [ ] **Add defensive assertions**: Include integrity checks like `assert(self.impl != undefined)` where appropriate
- [ ] **Use `@branchHint`**: Add branch hints for unlikely error paths (e.g., allocation failures in `serialize`)
- [ ] **Consider readonly stream pattern**: Evaluate if `VtStream` should return a readonly view like Ghostty's `vtStream()`
- [ ] **Propagate errors instead of returning null**: Change `serialize*` functions to return `!?[]const u8` and let callers decide how to handle failures

### Medium Priority

- [ ] **Expose formatter options**: Consider making palette, tabstops, screen selection configurable rather than hardcoded
- [ ] **Add "why" comments**: Document rationale for key decisions (e.g., why `palette = false` in serialization)
- [ ] **Review terminal state completeness**: Ensure all relevant terminal modes are captured in `serializeState()`

### Low Priority / Style

- [ ] **Run through Ghostty's linting**: If Ghostty has custom zig-fmt rules, ensure compliance
- [ ] **Match file organization**: Check if Ghostty prefers nested types defined inline vs separate constants
- [ ] **Review test coverage**: Ghostty emphasizes manual testing for input; consider adding unit tests for serialization edge cases

---

## Build Integration Notes

The current build.zig uses:
- `b.option(Backend, "backend", ...)` for backend selection
- `b.lazyDependency("ghostty", ...)` to avoid downloading Ghostty when building libvterm variant
- Conditional `linkSystemLibrary("vterm", ...)` for the libvterm backend

This pattern should work well with Ghostty's build system if this abstraction is ever upstreamed to Ghostty itself.

---

## Summary

The refactoring successfully isolates ghostty-vt behind a clean abstraction using Zig's comptime polymorphism. The code generally follows Ghostty's naming conventions and patterns, but deviates in a few areas:

**Strengths:**
- Zero-cost abstraction with compile-time backend selection
- Clean separation of backend implementation from daemon logic
- Consistent naming (PascalCase types, snake_case functions)
- Proper allocator passing and deferred cleanup

**Areas for improvement before upstream:**
1. Error handling (return errors vs suppress with null)
2. Defensive assertions
3. Branch hints for unlikely paths
4. More detailed "why" comments
5. Consider readonly stream pattern
