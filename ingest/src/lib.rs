//! Ingestion library: the common infrastructure every source worker uses
//! (`01-data-ingestion/SKILL.md` §4, `BACKFILL.md` §3-§4, `RETRY-POLICY.md`).

pub mod eodhd;
pub mod identity;
pub mod infra;
pub mod ishares;
