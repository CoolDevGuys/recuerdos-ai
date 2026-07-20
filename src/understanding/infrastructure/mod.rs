//! Adapters: the job queue, ingestion REST/MCP handlers, prompt files
//! included via `include_str!` — everything that touches the outside world.

pub mod http;
pub mod ingest_workers;
pub mod sqlite_job_queue;

#[cfg(test)]
mod ingest_worker_tests;
#[cfg(test)]
mod job_queue_tests;
