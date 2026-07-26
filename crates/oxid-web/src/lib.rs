//! Library half of the front end.
//!
//! Same split as `oxid-api`, and for the same reason (see `docs/DECISOES.md`,
//! stage 1): in a pure binary, `unreachable_pub` rejects `pub` while
//! `redundant_pub_crate` rejects `pub(crate)`, and the two cannot both be
//! satisfied. A library gives the items a genuine external path.

pub mod api;
pub mod app;
pub mod storage;
