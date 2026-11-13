# agent4agent (e4a)

Durable agent engine for Hatchery: give it an API key or an inference endpoint,
get an agent with tools, sessions, forks, checkpointed tasks and cost accounting
that survives a process crash. Written in Rust. To gate4agent (g4a) it is one
more provider, spoken to over ACP on stdio. It also runs as an advisor and in
other review/automation loops.

## Origin and attribution

e4a is a Rust port of **pi-durable 1.5.2**
([`@earendil-works/pi-durable`](https://github.com/earendil-works/pi/tree/v1.5.2/packages/durable),
tag `v1.5.2` = `b4afdddf44e059c67ccf5097e5317d913521b470`) by Earendil Works —
Mario Zechner and Armin Ronacher — released under the MIT License. We are
grateful to its authors; the design (journal before display, checkpointed tasks,
forks, usage ledger) is theirs. The upstream license text is kept in
[`LICENSES/pi-MIT.txt`](LICENSES/pi-MIT.txt); see [`NOTICE`](NOTICE).

This is a vendor fork: 1.5.2 is the fixed base, we maintain it ourselves and take
ideas from upstream selectively. The 1.5.0 E0 fixtures stay; durable had no
breaking changes after 1.5.0.

## Where the code lives

Working code lives in the Hatchery **federated git trees** (tree `agent4agent`).
This private repository is an **independent origin for checkpoints**: snapshots
are pushed here after each stage, never force-pushed.

Plan: `hatchery-websession-docs/docs/plans/engine4agent.md`.

## Store

Document deltas are Chord operations (`set`, `splice`, and the other `Op` variants
in `e4a-types`). Those operations are not RFC 6902, so `e4a-types/src/apply.rs`
applies them directly. The `json-patch` crate is the wrong tool for that and is
not a dependency.

`SqliteStorage` is `!Sync` (`RefCell`). The session layer (Э2) will keep one
writer thread and send commits to it on a channel — the plan's single writer —
rather than a `Mutex` held across a task. One handle owns the SQLite file; a
second `open` of the same path fails.

## License

MIT, see [`LICENSE`](LICENSE). Third-party notices in [`NOTICE`](NOTICE).
