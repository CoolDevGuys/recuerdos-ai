//! `ProjectRegistry` over the V11 `projects` and `project_aliases` tables.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::applies_to::ProjectRef;
use crate::memories::domain::project_registry::ProjectRegistry;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::ProjectId;
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::Utc;
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
}
