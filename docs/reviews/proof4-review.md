# Review: proof 4 "Claims are exclusive and expire" (commits e254e8d..8c2436b)

- **Reviewed:** commits df2535c and 8c2436b (vector), now on main.
- **Line numbers:** taken at main `2e98352`. They differ from the original slice where proof 6 or `f559e15` moved code.
- **Judged against:** KERNEL.md proof 4, INVARIANTS.md (1, 2, 3, 4 and the anti-goals), GLOSSARY.md (Claim, Board) and AGENTS.md § Coding standards.
- **Method:** the `review` skill's three axes (fitness, intent-fit, syntactical) ran as separate agents, followed by a direct check of replay, clock use, races and the acceptance script. The review was read-only.

## Verdict: fix first (proof 4 is already on main, so the fixes go to main)

No finding blocks on the kernel or the invariants. KERNEL.md, INVARIANTS.md and GLOSSARY.md are unchanged by the slice, `scripts/check-harness-free` passes, and nothing enters the workflow-engine anti-goal.

`scripts/acceptance claims` cannot exit 0 while any of the four sentences in proof 4 is false:
- **"A second owner is refused":** the script requires exit 3 exactly and a `REFUSED op=… holder=` line. An error exits 1, so it cannot pass as a refusal.
- **"Expiry frees the key":** the board reads expiry on the coordinator's clock, and the timing assertion runs on the coordinator's own host.
- **"The board read from both hosts shows one holder":** the two reads go to two separate host daemons (`a.sock` locally, `b.sock` over ssh).

Findings 1 to 4 should still be fixed before the next slice builds on claims; the `/roger` board is next in line. Finding 1 is a partial match against the glossary's definition of Claim. Findings 2 and 3 are test weaknesses that do not make the current check pass falsely, but make it less tight than it reads. Finding 4 makes the process test flaky across sibling worktrees.

### Questions asked, answered
- **Does replay after a coordinator restart restore claims correctly? Yes.**
  - Each claim event stores an absolute `expires_ms`.
  - `claim()` decides, appends with an fsync (`src/durlog.rs:61-67`), applies, then answers, all under one `State` mutex guard (`src/coordinator.rs:335-341`, `:376-427`).
  - Replay (`src/coordinator.rs:121-139`) applies grants and releases in log order, so a deadline that passed while the coordinator was down reads as expired.
  - The process test `claim_survives_a_coordinator_restart` (`tests/process.rs:981`) covers this.
- **Is only the coordinator's clock used? Yes.**
  - The deadline is `now + ttl_ms`, with `now` from `now_ms()` at `src/coordinator.rs:37-43`, set in `src/claims.rs:100`.
  - Clients send only a TTL.
  - Wall-clock caveat: finding 7.
- **Can acquire or renew race across two hosts? No.**
  - Every decision is serialized by the coordinator's `State` mutex.
  - Host daemons relay `claim` and `board` requests with no cache (`src/daemon.rs:486`).
  - A retried acquire or renew by the holder after a lost reply is idempotent (`src/claims.rs:81`).
- **Is the `cross_host` → `two_host_stack` refactor safe? Yes.**
  - It is a pure move (`scripts/acceptance:56`, used at `:97` and `:396`).
  - Proof 1's assertions are byte-identical, and `bin` and `coord` are now script-level globals that are set before dispatch.
  - Not rerun here, because it needs tailscale and `agent-jail`. The vector's PASS for `cross-host` is unverified.

## Findings (most severe first)

### 1. Medium: a Claim owner is any string, not a registered session
- **Where:** `src/coordinator.rs:386` reads `let owner = net::field(v, "owner")?.to_string();` and `decide` runs without checking `st.sessions`. The acceptance run never registers `$a` or `$b` (`scripts/acceptance:394-424`), and neither does `tests/process.rs:915-960`.
- **Why:** GLOSSARY Claim says "Exclusive ownership of one key by one session, renewed by that session alone." At present the holders are not sessions, and any client can renew as `--owner $a`.
- **Fix:**
  - In `claim()`, refuse with `reason: "owner <x> is not a registered session"` when `!st.sessions.contains_key(&owner)`.
  - Register both sessions in `claims()` and in the process tests before claiming, and add a refusal case for an unregistered owner.
  - Binding the owner to the requesting connection, to stop impersonation, is a separate design decision: `send`'s `from` field trusts the caller the same way today.

### 2. Medium: the renew-past-first-deadline check has a narrow margin that is not asserted
- **Where:**
  - `scripts/acceptance:398` takes `t0` before the acquire. `:409` sleeps until `t0+4500` and treats that as past the first deadline, but the first deadline is the acquire's coordinator time plus 4000.
  - `tests/process.rs:921` (`start` before the acquire) and `:944` (sleep to 1700 ms against a 1500 ms TTL) repeat the pattern with a 200 ms margin.
- **Effect:** if the acquire takes more than 500 ms (200 ms in the test), the "renewal still holds" step runs before the original deadline and proves nothing about renewal. It still passes.
- **Fix:**
  - Use the deadline the coordinator reports (`holder_fields` already returns a remaining time), or take `t1` after `CLAIMED` returns.
  - Sleep until `t1 + ttl + margin`, and assert `now >= t1 + ttl` before the refused acquire.

### 3. Medium: a failed board read counts as "key free"
- **Where:** `scripts/acceptance:392` is `key_free_on_board() { ! local_roger board | grep -qF "key=$CLAIM_KEY"; }`.
- **Effect:** if `board` errors, the key looks free. The later `>= tr+8000` check and `$b`'s acquire stop a false PASS today, but the helper does not check what it claims to.
- **Fix:** `key_free_on_board() { local out; out="$(local_roger board)" || return 1; ! grep -qF "key=$CLAIM_KEY" <<<"$out"; }`. Assert the same on `remote_roger board` after expiry.

### 4. Medium: process tests collide across worktrees through a shared temp directory
- **Where:** `tests/process.rs:28` builds `temp_dir().join(format!("roger-process-{tag}"))`, and `Rig::new` runs `remove_dir_all` on it.
- **Effect:** sibling worktrees (`rw-proof-5-wake`, `rw-roger-command`, …) that run the same test delete each other's directory and sockets. One of ten runs failed at the `start.elapsed() >= 3000` assertion (`:954` at main) while another run was active.
- **Fix:** keep the per-tag, survives-until-next-run intent stated in the comment at `:25-27`, but scope the name to the worktree, e.g. `roger-process-{hash(env!("CARGO_MANIFEST_DIR"))}-{tag}`.

### 5. Low: the invariant 2 check cannot see a claim-expiry regression
- **Where:** `scripts/check-no-poll`, added in `f559e15`, is a source grep for sleeps, timeouts and intervals.
- **Why:** INVARIANTS.md says the check "fails when an event trigger is removed." For claims, the trigger is the deadline comparison in `Claims::live` and `Claims::active` (`src/claims.rs:70`, `:133-139`). If someone deletes that comparison, no-poll still passes; only `acceptance claims` and the unit tests catch it.
- **Fix:** add a claims case to `scripts/acceptance no-poll`, or to its mutation list, that removes the deadline filter and expects failure.

### 6. Low: replay drops malformed claim events silently, and a failed fsync can resurrect a grant
- **Where:**
  - `src/coordinator.rs:121-138` decodes with `if let (Ok(key), Ok(owner), Some(expires_ms))` and ignores anything that does not match.
  - `src/coordinator.rs:414` uses `st.log.append(..)?`. If `write_all` succeeds and `sync_all` fails, the client gets an error and memory is not updated, but the line may survive and replay as a grant after a restart.
- **Fix:**
  - Return an `InvalidData` error naming the line when a `claim` or `release` event lacks its fields, so start-up fails loudly.
  - On an append error, stop the coordinator (or poison `DurLog`) instead of continuing with memory that has diverged from disk. This matches invariant 1's durability stance.

### 7. Low: deadlines use the wall clock
- **Where:** `src/coordinator.rs:37-43` uses `SystemTime::now()`.
- **Effect:** a forward clock jump (NTP step, suspend and resume) frees held keys early, and a backward jump extends them. Absolute deadlines must survive a restart, so a monotonic clock alone will not do.
- **Fix:** document the assumption on `now_ms()` and in the README's claims section. Optionally clamp `now` to never go backwards within one process.

### 8. Low: acquire and renew behaviour nobody asked for
- **Where:** `src/claims.rs:81`. `Op::Acquire => live.is_none() || mine` lets the holder re-acquire, which silently resets its deadline, and a renew can shorten it.
- **Fix:** either document both, as retry-safe acquire and renew-sets-deadline, in the `decide` doc comment and the README, or refuse acquire-by-holder and renews that shorten the deadline. Documenting fits the retry-idempotence argument better.

### 9. Low: the Board lists claims in key order (invariant 3, borderline)
- **Where:** `src/claims.rs:64` (`BTreeMap`) and `:132` ("in key order").
- **Why:** a key such as `[[153:Meteor Shower:7]]` therefore sorts by its leading integer's text. That is close to "no code … ranks … them".
- **Fix:** keep insertion or grant order (for example an index map, or a sequence number on `Held`), or state that the order is unspecified and drop "in key order".

### 10. Low (AGENTS.md § Coding standards "a failure names what was expected and what was seen"): two error strings say neither
- **Where:** `src/session.rs:256` `"no answer"` and `:260` `.unwrap_or("unexplained")`.
- **Fix:** `format!("expected one reply line from {}, saw EOF", socket.display())` and `format!("expected a reason in the error reply, saw {}", json::stringify(&reply))`.
- (The other standards finding from the original slice, the stale usage line, is fixed on main at `src/main.rs:98`.)

### 11. Smell (judgement call): Data Clumps between `Change::Grant` and `Held`
- **Where:** `src/claims.rs:49`, `:98-108`, `:115-125`, and `src/coordinator.rs:126-130`, `:400-406`.
- **Fix:** make it `Change::Grant(Held)`. `decide` then returns `holder: Some(held.clone())`, and `apply` becomes `insert(key, held.clone())`.

### 12. Smell: the op travels as a string (Primitive Obsession)
- **Where:** `src/main.rs:69` (`if cmd == "claim" { "acquire" } else { cmd }`) and `src/session.rs:300-301` (matches on `"acquire"` and `"renew"`).
- **Fix:** parse to `claims::Op` once in `main` with `Op::parse`, pass `&Op` into `session::claim`, and match on the enum.

### 13. Smell: Duplicated Code in the board helpers
- **Where:** `tests/process.rs:1001-1006` rebuilds the `board` command that `fn board` at `:893` already runs.
- **Fix:** extract `board_output(rig, sock) -> Output`. `board` asserts success on it, and `wait_until_board_reaches_both` polls `.status.success()`.

### 14. Smell: Shotgun Surgery between claim event encode and decode
- **Where:** decode at `src/coordinator.rs:121-138`, encode at `:400-413`.
- **Fix:** add `Change::to_event(key)` and `Change::from_event(&Value)` next to `Change` in `src/claims.rs`. This pairs well with findings 6 and 11.

## Per-axis summary
- **Fitness:** 1 open breach of the coding standards (finding 10) and 4 smells (11 to 14). Clippy with `-D warnings` is clean, and `cargo test` passed 10 unit and 12 process tests at 8c2436b.
- **Intent-fit:** 1 partial example (finding 1) and weak or unrequested behaviour (findings 2, 3 and 8). The worst is finding 1.
- **Syntactical:** frozen files unchanged, no invariant violated, and `check-harness-free` passes. The worst findings are 5 (no-poll does not cover claim expiry) and 9 (key-ordered Board).

## Not run
- `scripts/acceptance claims` and `scripts/acceptance cross-host`. Both need tailscale, `agent-jail` and a release build in `$ROOT/target`, and other writers were active in the worktree.
