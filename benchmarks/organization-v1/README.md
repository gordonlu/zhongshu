# Organization benchmark v1

This suite evaluates staffing before any model is called. It is deliberately
separate from `review-v1`: review is one specialized pipeline, while this suite
tests whether Zhongshu selects only the roles needed for cross-domain work.

Roles and capabilities are open string identifiers loaded from configuration;
they are not a closed development-role enum. The development fixture expects
backend + frontend + writer, while the finance fixture defines management
accountant + treasury reviewer at runtime. Built-in development constructors
are convenience templates only. Unrelated employees are not automatically
dispatched; independent verification must be an explicit requirement.

Current evidence level: deterministic staffing plus scripted read-only
execution. The finance flow also verifies a bounded sequential handoff from the
management accountant to the treasury reviewer. User-direct assignment is
covered separately and cannot bypass the target employee's declared role or
capabilities. Do not run this suite against live providers; mutation-capable
organization execution is deliberately blocked until it uses file claims and
parent patch review.
