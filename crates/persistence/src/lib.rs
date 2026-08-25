#![forbid(unsafe_code)]

//! Extractor-owned `PostgreSQL` pool and current schema.

mod records;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use records::{
    ArtifactKind, ArtifactRecord, CandidateRecord, FetchRecord, MediaArchiveRecord,
    delete_expired_media, has_unexpired_media_for_video, record_artifact, record_candidate,
    record_fetch, reserve_media_archive, unexpired_media_bytes,
};

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor as _, PgPool};

const SCHEMA: &str = include_str!("../../../schema.sql");
const SCHEMA_LOCK: i64 = 0x7261_7461_736b_7202;

/// A persistence failure safe to expose only to process diagnostics.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PersistenceError {
    /// The database connection could not be established.
    #[error("the database connection could not be established")]
    Connect(#[source] sqlx::Error),
    /// The current schema could not be applied.
    #[error("the database schema could not be applied")]
    Schema(#[source] sqlx::Error),
    /// An extractor-owned query failed.
    #[error("an extractor database query failed")]
    Query(#[source] sqlx::Error),
    /// A numeric record field cannot fit its database representation.
    #[error("an extractor record exceeds its supported range")]
    ValueOverflow,
    /// An artifact reference is not owned by this extractor or uses an unsupported digest.
    #[error("an extractor artifact reference is invalid")]
    InvalidArtifact,
    /// The run does not belong to the supplied owner.
    #[error("the extraction run does not belong to the owner")]
    OwnerMismatch,
}

/// The one finite `PostgreSQL` pool owned by the extractor process.
#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Connects one finite pool.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::Connect`] when `PostgreSQL` is unavailable.
    pub async fn connect(
        url: &str,
        max_connections: u32,
        acquire_timeout: std::time::Duration,
    ) -> Result<Self, PersistenceError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(acquire_timeout)
            .connect(url)
            .await
            .map_err(PersistenceError::Connect)?;
        Ok(Self { pool })
    }

    /// Applies the current schema once.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::Schema`] when `PostgreSQL` refuses the schema.
    pub async fn apply_schema(&self) -> Result<(), PersistenceError> {
        let mut transaction = self.pool.begin().await.map_err(PersistenceError::Schema)?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *transaction)
            .await
            .map_err(PersistenceError::Schema)?;
        let present: Option<String> =
            sqlx::query_scalar("select to_regnamespace('extractor')::text")
                .fetch_one(&mut *transaction)
                .await
                .map_err(PersistenceError::Schema)?;
        if present.is_none() {
            transaction
                .execute(SCHEMA)
                .await
                .map_err(PersistenceError::Schema)?;
        }
        transaction.commit().await.map_err(PersistenceError::Schema)
    }

    /// Returns the owned pool to data-access crates.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Closes the pool after admitted work is joined.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    #[cfg(feature = "test-support")]
    pub(crate) const fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}
