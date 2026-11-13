//! `ProjectRegistry` over the V11 `projects` and `project_aliases` tables.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::ProjectRef;
use crate::memories::domain::project_registry::{ProjectRecord, ProjectRegistry};
use crate::shared::error::{RaError, Result};
use crate::shared::ids::ProjectId;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

pub struct SqliteProjectRegistry {
    database: Arc<SqliteDatabase>,
}

impl SqliteProjectRegistry {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

fn project_ref(id: String, name: String) -> Result<ProjectRef> {
    Ok(ProjectRef {
        id: ProjectId::from_str(&id)
            .map_err(|e| RaError::Internal(format!("stored project id {id:?}: {e}")))?,
        name,
    })
}

fn timestamp(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|e| RaError::Internal(format!("stored project timestamp {raw:?}: {e}")))
}

fn failed(context: &'static str) -> impl Fn(rusqlite::Error) -> RaError {
    move |error| map_sqlite_error(error, context)
}

impl ProjectRegistry for SqliteProjectRegistry {
    fn resolve(&self, context: &UserContext, identifier: &str) -> Result<Option<ProjectRef>> {
        self.database.with_connection(|connection| {
            // A name wins over an alias: an alias is a guess about where the
            // caller is, a name is what the user calls the project.
            let found: Option<(String, String)> = optional(connection.query_row(
                "SELECT id, name FROM projects WHERE user_id = ?1 AND name = ?2
                 UNION ALL
                 SELECT p.id, p.name FROM project_aliases a
                   JOIN projects p ON p.id = a.project_id
                  WHERE a.user_id = ?1 AND a.alias = ?2
                 LIMIT 1",
                rusqlite::params![context.user_id().to_string(), identifier],
                |row| Ok(Ok((row.get(0)?, row.get(1)?))),
            ))?;
            found.map(|(id, name)| project_ref(id, name)).transpose()
        })
    }

    fn names(&self, context: &UserContext) -> Result<Vec<String>> {
        self.database.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT name FROM projects WHERE user_id = ?1 ORDER BY name")
                .map_err(|e| map_sqlite_error(e, "reading projects"))?;
            statement
                .query_map([context.user_id().to_string()], |row| row.get(0))
                .map_err(|e| map_sqlite_error(e, "reading projects"))?
                .collect::<std::result::Result<Vec<String>, _>>()
                .map_err(|e| map_sqlite_error(e, "reading projects"))
        })
    }

    fn create(&self, context: &UserContext, name: &str, aliases: &[String]) -> Result<ProjectRef> {
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(|e| map_sqlite_error(e, "could not begin creating a project"))?;
            let id = ProjectId::new();
            let now = Utc::now().to_rfc3339();
            transaction
                .execute(
                    "INSERT INTO projects (id, user_id, name, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    rusqlite::params![id.to_string(), context.user_id().to_string(), name, now],
                )
                .map_err(|e| map_sqlite_error(e, &format!("project {name:?} already exists")))?;
            for alias in aliases {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO project_aliases (user_id, alias, project_id, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![context.user_id().to_string(), alias, id.to_string(), now],
                    )
                    .map_err(|e| map_sqlite_error(e, "project alias conflict"))?;
            }
            transaction
                .commit()
                .map_err(|e| map_sqlite_error(e, "could not commit a new project"))?;
            Ok(ProjectRef {
                id,
                name: name.to_string(),
            })
        })
    }

    fn add_alias(&self, context: &UserContext, project: &ProjectRef, alias: &str) -> Result<()> {
        self.database.with_connection(|connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO project_aliases (user_id, alias, project_id, created_at)
                     SELECT ?1, ?2, id, ?4 FROM projects WHERE id = ?3 AND user_id = ?1",
                    rusqlite::params![
                        context.user_id().to_string(),
                        alias,
                        project.id.to_string(),
                        Utc::now().to_rfc3339()
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "project alias conflict"))?;
            Ok(())
        })
    }

    fn list(&self, context: &UserContext) -> Result<Vec<ProjectRecord>> {
        let user = context.user_id().to_string();
        self.database.with_connection(|connection| {
            let mut aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let mut statement = connection
                .prepare("SELECT project_id, alias FROM project_aliases WHERE user_id = ?1 ORDER BY alias")
                .map_err(failed("reading project aliases"))?;
            let rows = statement
                .query_map([&user], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(failed("reading project aliases"))?;
            for row in rows {
                let (project, alias) = row.map_err(failed("reading project aliases"))?;
                aliases.entry(project).or_default().push(alias);
            }

            let mut statement = connection
                .prepare("SELECT id, name, created_at FROM projects WHERE user_id = ?1 ORDER BY name")
                .map_err(failed("reading projects"))?;
            let rows = statement
                .query_map([&user], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(failed("reading projects"))?;
            let mut records = Vec::new();
            for row in rows {
                let (id, name, created_at) = row.map_err(failed("reading projects"))?;
                records.push(ProjectRecord {
                    aliases: aliases.remove(&id).unwrap_or_default(),
                    project: project_ref(id, name)?,
                    created_at: timestamp(&created_at)?,
                });
            }
            Ok(records)
        })
    }

    fn find(&self, context: &UserContext, id: ProjectId) -> Result<Option<ProjectRef>> {
        self.database.with_connection(|connection| {
            let found: Option<(String, String)> = optional(connection.query_row(
                "SELECT id, name FROM projects WHERE user_id = ?1 AND id = ?2",
                rusqlite::params![context.user_id().to_string(), id.to_string()],
                |row| Ok(Ok((row.get(0)?, row.get(1)?))),
            ))?;
            found.map(|(id, name)| project_ref(id, name)).transpose()
        })
    }

    fn rename(
        &self,
        context: &UserContext,
        project: &ProjectRef,
        new_name: &str,
        aliases: &[String],
    ) -> Result<ProjectRef> {
        let user = context.user_id().to_string();
        let id = project.id.to_string();
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(failed("could not begin renaming a project"))?;
            let now = Utc::now().to_rfc3339();
            let changed = transaction
                .execute(
                    "UPDATE projects SET name = ?3, updated_at = ?4 WHERE user_id = ?1 AND id = ?2",
                    rusqlite::params![user, id, new_name, now],
                )
                .map_err(|e| {
                    map_sqlite_error(e, &format!("another project is already called {new_name:?}"))
                })?;
            if changed == 0 {
                return Err(RaError::NotFound(format!("project {}", project.name)));
            }
            if project.name != new_name {
                // The old name was this project's until now, so it wins over
                // any alias of another project that it shadowed.
                transaction
                    .execute(
                        "INSERT INTO project_aliases (user_id, alias, project_id, created_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (user_id, alias) DO UPDATE SET project_id = excluded.project_id",
                        rusqlite::params![user, project.name, id, now],
                    )
                    .map_err(failed("project alias conflict"))?;
            }
            for alias in aliases {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO project_aliases (user_id, alias, project_id, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![user, alias, id, now],
                    )
                    .map_err(failed("project alias conflict"))?;
            }
            // A name is found as a name; an alias spelling it is dead weight,
            // and one pointing at another project could only mislead.
            transaction
                .execute(
                    "DELETE FROM project_aliases WHERE user_id = ?1 AND alias = ?2",
                    rusqlite::params![user, new_name],
                )
                .map_err(failed("project alias conflict"))?;
            transaction
                .commit()
                .map_err(failed("could not commit a project rename"))?;
            Ok(ProjectRef {
                id: project.id,
                name: new_name.to_string(),
            })
        })
    }

    fn absorb(&self, context: &UserContext, from: &ProjectRef, into: &ProjectRef) -> Result<()> {
        let user = context.user_id().to_string();
        let (from_id, into_id) = (from.id.to_string(), into.id.to_string());
        self.database.with_connection(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(failed("could not begin merging projects"))?;
            let now = Utc::now().to_rfc3339();
            transaction
                .execute(
                    "UPDATE memories SET project_id = ?3 WHERE user_id = ?1 AND project_id = ?2",
                    rusqlite::params![user, from_id, into_id],
                )
                .map_err(failed("moving a project's memories"))?;
            transaction
                .execute(
                    "UPDATE project_aliases SET project_id = ?3 WHERE user_id = ?1 AND project_id = ?2",
                    rusqlite::params![user, from_id, into_id],
                )
                .map_err(failed("moving a project's aliases"))?;
            transaction
                .execute(
                    "INSERT INTO project_aliases (user_id, alias, project_id, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (user_id, alias) DO UPDATE SET project_id = excluded.project_id",
                    rusqlite::params![user, from.name, into_id, now],
                )
                .map_err(failed("project alias conflict"))?;
            transaction
                .execute(
                    "DELETE FROM project_aliases WHERE user_id = ?1 AND alias = ?2",
                    rusqlite::params![user, into.name],
                )
                .map_err(failed("project alias conflict"))?;
            let deleted = transaction
                .execute(
                    "DELETE FROM projects WHERE user_id = ?1 AND id = ?2",
                    rusqlite::params![user, from_id],
                )
                .map_err(failed("deleting a merged project"))?;
            if deleted == 0 {
                return Err(RaError::NotFound(format!("project {}", from.name)));
            }
            transaction
                .commit()
                .map_err(failed("could not commit a project merge"))
        })
    }
}
