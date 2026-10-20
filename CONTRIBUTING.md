# Contributing

Rust 1.91+ and Node 22+ are required. Before opening a pull request, run the
same checks CI runs:

```sh
cargo fmt --check
python3 scripts/check-vendor.py
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
bash scripts/check-js.sh
npm run check:types --prefix clients/js
npm run build:browser --prefix clients/js
```

Rules that keep the relay correct:

- No socket I/O may be performed while holding a registry lock (a session
  shard, the channel index, a channel, the listener set or a departure queue).
  Submit output after the guard drops, or queue it for the owning worker. The
  lock order is listed in [docs/architecture.md](docs/architecture.md).
- Every new configuration option in `src/config.rs` or `src/origin.rs` must be
  added to the reference table in [docs/operations.md](docs/operations.md)
  with its environment variable, default, valid range and meaning.
- A behavior change that users can observe goes in `CHANGELOG.md` under
  "Unreleased".
- Measurement reports go under `docs/reports/` with the date in the text. They
  describe the code as it was on that date and are not updated afterwards.
  Keep raw artifacts outside the repository and say so in the report.
- Define a project term before using it, or link [docs/glossary.md](docs/glossary.md).
