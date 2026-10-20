# Measurement and decision records

These files are dated measurement and decision records from the October 2026
Rust rewrite. They are kept for provenance. Each one describes the
implementation as it was on the date in its text, including its routing code,
test fixtures and measured numbers. They are not updated when the code changes.

The single global registry lock and the single global departure FIFO that these
reports discuss have since been replaced by per-channel locks and per-room
departure queues. For the current design, read [docs/architecture.md](../architecture.md).
