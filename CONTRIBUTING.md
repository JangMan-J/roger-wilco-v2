# Contributing

Read [AGENTS.md](AGENTS.md), KERNEL.md, INVARIANTS.md and GLOSSARY.md before changing code. Keep changes focused and add tests appropriate to their risk.

Run `scripts/check-all`; it must pass before landing. Run affected acceptance suites too; `scripts/acceptance --help` lists suites and required environment variables.

Frozen files and glossary namespaces change only through a fork the owner promotes; do not alter their pins to bypass `scripts/check-frozen`.

Vectors are append-only position reports in `VECTORS/<thread>/<UTC-YYYYMMDD-HHMMSS>.md`; read open threads and commit a new report for work moved.
