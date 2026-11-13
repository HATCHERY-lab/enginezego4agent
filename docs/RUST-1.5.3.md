# engine4agent spec 1.5.3

Our Rust port. Upstream published tag is still `v1.5.2`
(`b4afdddf44e059c67ccf5097e5317d913521b470`). `main` is unreleased.
This tree takes the durable fixes from those commits and the open issues
that belong to the engine. We do not wait for their tag.

## Taken from main after 1.5.2

- Suspendable work: signals, a tool may park, gates, hook tasks.
  The scheduler slice only reserves the `Parked` phase. The body lands
  after the four worktrees merge.
- `listDir` skips a name that vanishes between enumerate and stat
  (ENOENT). `e4a-env` does this.
- A waiter stores its error before the caller observes it. No dropped
  failure. `e4a-harness` scheduler.
- A task's conversation is its own `conversation_id`. Do not walk the
  owner chain.
- Code-mode namespace break is recorded, not ported. We do not ship the
  JS sandbox.

## Taken from open issues

- #12101 / PR #12134. OpenRouter chat completions send instruction
  messages as `system`. `developer` only when the caller sets it.
  OpenAI's own host is the exception; we are not that host.
- #12098. Store tool arguments after schema coercion, so a numeric field
  sent as `"20"` is replayed as a number.
- #11026. `delete_conversation` removes the conversation's rows. Root
  conversation 1 is refused. A later read is not found, not a tombstone
  that still returns history.
- #12090. Cancellation that waits on a signal belongs to the suspendable
  body, same slice as the park/gate work.
- #11446. A small extension entry point, after the kernel, not in this cut.

## Not our tree

Coding-agent catalog, Copilot model cache, context-window caps,
cache-warming extensions, sidebar. Those stay upstream.

## Slices in flight

| branch | crate | check dir |
|---|---|---|
| `slice/llm` | `e4a-llm` SOCKS OpenRouter, role `system` | `E:\nemo-build\cargo-target-e4a-llm` |
| `slice/shell` | `e4a-tools` bash and powershell. Arg coercion (#12098) is the next edit on this crate, not in the first check | `E:\nemo-build\cargo-target-e4a-tools` |
| `slice/env` | `e4a-env` local jail, ENOENT list | `E:\nemo-build\cargo-target-e4a-env` |
| `slice/sched` | `e4a-harness` queue, tool phases | `E:\nemo-build\cargo-target-e4a-harness` |
| `slice/session` | delete conversation, no owner walk | `E:\nemo-build\cargo-target-e4a-session` |

Each slice runs `cargo check` only. Merge is into `e1/store` when the
checks are green. No crates.io publish.
