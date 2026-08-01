pub mod pagination;
pub mod repository;
pub mod sources;

pub use pagination::{PageSource, Paginator};
// `RepoResult` is part of this module's public error-handling surface (used
// by every `GraphRepository` method signature); re-exported for callers that
// want to name it without reaching into `data::repository` directly.
#[allow(unused_imports)]
pub use repository::{CoreRepository, GraphRepository, RepoResult};
