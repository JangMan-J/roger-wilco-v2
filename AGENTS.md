This project has a kernel: read KERNEL.md, INVARIANTS.md, GLOSSARY.md, and the open threads in VECTORS/ before changing anything.
KERNEL.md, INVARIANTS.md and the frozen `## kernel` and `## invariants` namespaces of GLOSSARY.md name no harness or model; provenance frontmatter (`agent:`, `session:`) may. `scripts/check-frozen` pins the frozen bytes (`scripts/frozen.sha256`). Harness specifics live in shims/ and their tests.

## Directives

- "just remember, this is a standalone project. meant to be as applicable to a call center as to agents. dont let the harness symantics take it over" (2026-10-22)
- "my vision is this to be a ruthlessly efficient foundation that any sort of adapter can be placed on top of. modular" (2026-10-22)
- "if there is a fundamental issue that cant be resolved in the original documents, the rules are simple. fork it-change-it-try again" (2026-10-22)
- "Wake (proof 5) is landed and is not a focus: do not extend, prototype or queue wake work unless Jang asks." (2026-10-22)
