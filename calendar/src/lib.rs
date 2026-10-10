//! NYSE trading calendar: holiday rules computed for any year, unioned with the
//! overrides seed (`config/nyse_calendar_overrides.toml`) and the runtime overrides.
//!
//! Specification: `.claude/skills/09-lock-state-machine/SKILL.md` §4 and constitution §1
//! rule 6. Must reproduce `.claude/skills/09-lock-state-machine/fixtures/nyse_calendar_cases.json`
//! exactly, like the Julia oracle does.
//!
//! Phase I1, workstream W2. Not implemented yet.
