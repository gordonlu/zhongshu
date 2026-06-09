# AGENTS.md

## Core Principle

Correctness over appearance.

Do not optimize for looking finished. Optimize for verified behavior, minimal changes, and explicit assumptions.

---

## Before Coding

* Do not assume. If requirements are ambiguous, ask.
* State important assumptions explicitly.
* Prefer the simplest solution that satisfies the request.
* Do not add features, abstractions, or configurability that were not requested.

---

## Editing Existing Code

* Read relevant files before editing.
* Never invent file contents, APIs, or project structure.
* Make the smallest change that solves the problem.
* Match existing code style and architecture.
* Do not refactor unrelated code.
* Do not fix unrelated issues unless asked.

Every changed line should be traceable to the user's request.

---

## Signature and API Changes

Before changing:

* Function signatures
* Public APIs
* Database schemas
* Shared types

Check all callers and usages first.

Do not update a definition without updating its consumers.

---

## Verification

Do not treat compilation, green tests, or successful demos as proof of correctness.

When relevant, verify:

* Failure paths
* Restart/recovery behavior
* State consistency
* Persistence behavior
* Concurrency implications

If something was not verified, say so explicitly.

Never claim correctness you did not verify.

---

## Error Handling

* Do not hide failures.
* Do not swallow errors silently.
* Do not implement fake retries, fake recovery, or fake persistence.
* Prefer observable failures over invisible degradation.

---

## Code Hygiene

Remove code made unused by your changes:

* Imports
* Variables
* Functions

Do not remove unrelated dead code.

---

## Execution Workflow

For non-trivial tasks:

1. Understand the request
2. Inspect relevant code
3. Make minimal changes
4. Verify behavior
5. Report what was verified and what was not

Stop when the requested problem is solved.
