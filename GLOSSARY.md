---
type: glossary
created: 2026-10-21T00:15:26Z
updated: 2026-10-21T00:36:50Z
agent: accounts/fireworks/models/deepseek-v4p1-flash-1017
session: 01a15ba2-183a-7158-a4c7-5e136b2b5972
---

# Roger-Wilco

Durable mail, presence, and work claims between agent sessions on one owner's hosts.

## kernel

Frozen with KERNEL.md.

**Agent session**:
A process that keeps a durable record of what it has received and speaks the adapter contract; the core never asks which harness runs it.
_Example_: a live interactive session and a standalone process with no harness.
_Not_: a shell, a terminal pane, a host.
_Confirmed_: 2026-10-21, Jang, at init

**Mail**:
A body of text sent to one agent session or to every session; opaque integers travel inside a reference written in that text, never in a separate metadata field.
_Example_: a body `start [[0:build:3]] now` carries the label `build` with integers `0` and `3`.
_Not_: a wake, a receipt, a tool call.
_Confirmed_: 2026-10-21, Jang, at init

**Ack**:
The act of naming, to the core, the durable record in which a recipient stored a delivered body.
_Example_: a recipient answering with the identifier of the record it wrote and made durable.
_Not_: a model's reply, an open, a read without a record.
_Confirmed_: 2026-10-21, Jang, at init

**Address**:
The routing identity of an agent session: a UUID that a label never changes.
_Example_: a session keeps its UUID when its label goes from `AGENT` to `BUILDER`.
_Not_: a label, a display prefix, a call sign.
_Confirmed_: 2026-10-21, Jang, at init

**Receipt**:
The record the core keeps once a recipient acks delivery, naming that recipient's own durable record.
_Example_: a receipt naming record `r-1` after the recipient acked it.
_Not_: a notice that mail was sent, a lease.
_Confirmed_: 2026-10-21, Jang, at init

**Store**:
The durable record an agent session keeps of the mail it has received, written and made durable before that session acks.
_Example_: the append-only record a tracer writes, then names when it acks.
_Not_: the coordinator's state, an inbox, a harness transcript.
_Confirmed_: 2026-10-21, Jang, at init

**Core**:
The coordinator, host daemon, client, and adapter: every part that must not know a harness.
_Example_: the binaries installed on both hosts.
_Not_: a shim.
_Confirmed_: 2026-10-21, Jang, at init

**Coordinator**:
The single central process that holds durable state for the fleet and connects to each host daemon.
_Example_: the process owning the mail, session, and claim store that sessions on both hosts reach.
_Not_: a host daemon, a shim.
_Confirmed_: 2026-10-21, Jang, at init

**Host daemon**:
The one process per host that connects outbound to the coordinator and keeps the local inbox for the sessions on that host.
_Example_: the agent-jail process that keeps the local inbox for the sessions on agent-jail.
_Not_: the coordinator, a shim.
_Confirmed_: 2026-10-21, Jang, at init

**Harness**:
The program that runs an agent session and owns its model loop, transcript, and interface.
_Example_: an interactive coding agent, a batch agent, a plain command-line agent.
_Not_: a shim, the adapter.
_Confirmed_: 2026-10-21, Jang, at init

**Shim**:
The mechanical, per-harness program that reports what its harness observes, forwards tool calls, and renders results; it never acknowledges a delivery.
_Example_: the shim that forwards its harness's observations and tool calls to the adapter.
_Not_: the adapter, the core.
_Confirmed_: 2026-10-21, Jang, at init

**Tracer**:
A standalone process with no harness, used as the second session in the independence proof.
_Example_: a process that registers, reads mail, writes a durable record, acks, and prints the body.
_Not_: a fixture that fakes the core.
_Confirmed_: 2026-10-21, Jang, at init

**Board**:
The readable list of registered sessions and active claims, open to every session.
_Example_: the presence view a sender reads to find an address.
_Not_: a mailbox.
_Confirmed_: 2026-10-21, Jang, at init

**Claim**:
Exclusive ownership of one key by one session, renewed by that session alone, and freed by release or TTL expiry.
_Example_: one session holding `[[0:build:0]]` for 900 seconds.
_Not_: a lock on a host, a task assignment.
_Confirmed_: 2026-10-21, Jang, at init

**Wake**:
A request, made when mail is sent, for the receiving session to attend to its mailbox; a broadcast makes no wake request.
_Example_: a forceful wake starting a turn in an idle live session.
_Not_: a message type, a receipt.
_Confirmed_: 2026-10-21, Jang, at init

**Reference**:
A label with an opaque integer on each side, written `[[<integer>:<label>:<integer>]]`; it names a subject and carries nothing the core reads.
_Example_: `[[153:Meteor Shower:7]]` and `[[0:FLEET:0]]`.
_Not_: an address, a claim key with reserved meaning.
_Confirmed_: 2026-10-21, Jang, at init

**Opaque integer**:
A number attached to a label or a session, stored and returned verbatim and never interpreted.
_Example_: the `153` and `7` in `[[153:Meteor Shower:7]]`.
_Not_: a rank, a type, a status.
_Confirmed_: 2026-10-21, Jang, at init

**Broadcast**:
One mail body addressed to every registered session; it carries no wake request.
_Example_: `build finished` sent to all sessions on both hosts.
_Not_: mail addressed to a list of sessions.
_Confirmed_: 2026-10-21, Jang, at init

## invariants

Frozen with INVARIANTS.md.

**Committed event**:
A change to stored state that is durable and that triggers notification, rather than being found by a fixed-interval scan.
_Example_: a mail insert that wakes the local delivery path.
_Not_: a timer tick.
_Confirmed_: 2026-10-21, Jang, at init

**Cleanup sweep**:
A pass that removes expired state, which correctness may not depend on.
_Example_: the pass that drops mail acked seven days ago.
_Not_: the delivery path.
_Confirmed_: 2026-10-21, Jang, at init
