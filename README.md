# engine4agent (e4a)

Durable agent engine for Hatchery: give it an API key or an inference endpoint,
get an agent with tools, sessions, forks, checkpointed tasks and cost accounting
that survives a process crash. Written in Rust. To gate4agent (g4a) it is one
more provider, spoken to over ACP on stdio. It also runs as an advisor and in
other review/automation loops.

## Origin and attribution

e4a is a Rust port of **pi-durable 1.5.0**
([`@earendil-works/pi-durable`](https://github.com/earendil-works/pi/tree/v1.5.0/packages/durable),
tag `v1.5.0` = `f9a26365568127461c47a07a37dcb42c140a3ac5`) by Earendil Works —
Mario Zechner and Armin Ronacher — released under the MIT License. We are
grateful to its authors; the design (journal before display, checkpointed tasks,
forks, usage ledger) is theirs. The upstream license text is kept in
[`LICENSES/pi-MIT.txt`](LICENSES/pi-MIT.txt); see [`NOTICE`](NOTICE).

This is a vendor fork: 1.5.0 is the fixed base, we maintain it ourselves and take
ideas from upstream selectively.

## Where the code lives

Working code lives in the Hatchery **federated git trees** (tree `engine4agent`).
This private repository is an **independent origin for checkpoints**: snapshots
are pushed here after each stage, never force-pushed.

Plan: `hatchery-websession-docs/docs/plans/engine4agent.md`.

## License

MIT, see [`LICENSE`](LICENSE). Third-party notices in [`NOTICE`](NOTICE).
