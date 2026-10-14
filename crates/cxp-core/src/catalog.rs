//! The experience catalog (story S8): `CatalogExt` on `otto_tenant::Tx` --
//! experiences, applications, pages, and components, each optionally owned by
//! a platform team (`team_id`) -- with its model structs.
//!
//! The hierarchy is Experience -> Application -> Page -> Component. A child's
//! parent is optional, but a parent owns its children: deleting it deletes
//! them. Rows elsewhere (tickets, feedback, ...) that pointed at a deleted
//! entity keep existing with the reference nulled by the database.
//!
//! `team_id` is an opaque platform team id. This module only stores it; the
//! API layer verifies it at the platform before calling in. On update the team
//! is a [`TeamChange`]: leave it, clear it, or set it.

use chrono::{DateTime, Utc};
use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::future::Future;
use uuid::Uuid;

use crate::error::{on_foreign_key, Error, Result};
use crate::paging;

/// What an update does to an entity's `team_id`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TeamChange {
    /// Leave the team as it is (the field was absent).
    #[default]
    Keep,
    /// Remove the team (the field was an explicit null).
    Clear,
    /// Own the entity by this platform team.
    Set(Uuid),
}

impl TeamChange {
    /// Build from a deserialized `Option<Option<Uuid>>`: absent is `None`,
    /// explicit null `Some(None)`, an id `Some(Some(id))`.
    pub fn from_nested(v: Option<Option<Uuid>>) -> Self {
        match v {
            None => TeamChange::Keep,
            Some(None) => TeamChange::Clear,
            Some(Some(id)) => TeamChange::Set(id),
        }
    }

    /// `(change_team, new_value)` as bound into `SET team_id = CASE WHEN $a THEN $b ELSE team_id END`.
    fn binds(self) -> (bool, Option<Uuid>) {
        match self {
            TeamChange::Keep => (false, None),
            TeamChange::Clear => (true, None),
            TeamChange::Set(id) => (true, Some(id)),
        }
    }
}

// ---------------------------------------------------------------- experience

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Experience {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const EXPERIENCE_COLS: &str = "id, name, description, team_id, metadata, created_at, updated_at";

/// Fields for a new experience. `metadata` defaults to `{}`.
#[derive(Debug, Clone, Default)]
pub struct NewExperience {
    pub name: String,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

/// Fields to change on an experience; `None` leaves a field as it is (so a text
/// field cannot be cleared). The parent never changes.
#[derive(Debug, Clone, Default)]
pub struct ExperienceUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub team: TeamChange,
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------- application

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Application {
    pub id: Uuid,
    pub experience_id: Option<Uuid>,
    pub name: String,
    pub platform: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const APPLICATION_COLS: &str =
    "id, experience_id, name, platform, description, team_id, metadata, created_at, updated_at";

/// Fields for a new application. `metadata` defaults to `{}`.
#[derive(Debug, Clone, Default)]
pub struct NewApplication {
    pub experience_id: Option<Uuid>,
    pub name: String,
    pub platform: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

/// Fields to change on an application; `None` leaves a field as it is (so a text
/// field cannot be cleared). The parent never changes.
#[derive(Debug, Clone, Default)]
pub struct ApplicationUpdate {
    pub name: Option<String>,
    pub platform: Option<String>,
    pub description: Option<String>,
    pub team: TeamChange,
    pub metadata: Option<serde_json::Value>,
}

/// Filters for listing applications.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApplicationFilter {
    pub experience_id: Option<Uuid>,
}

// ---------------------------------------------------------------- page

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: Uuid,
    pub application_id: Option<Uuid>,
    pub name: String,
    pub path: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const PAGE_COLS: &str =
    "id, application_id, name, path, description, team_id, metadata, created_at, updated_at";

/// Fields for a new page. `metadata` defaults to `{}`.
#[derive(Debug, Clone, Default)]
pub struct NewPage {
    pub application_id: Option<Uuid>,
    pub name: String,
    pub path: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

/// Fields to change on a page; `None` leaves a field as it is (so a text
/// field cannot be cleared). The parent never changes.
#[derive(Debug, Clone, Default)]
pub struct PageUpdate {
    pub name: Option<String>,
    pub path: Option<String>,
    pub description: Option<String>,
    pub team: TeamChange,
    pub metadata: Option<serde_json::Value>,
}

/// Filters for listing pages.
#[derive(Debug, Clone, Copy, Default)]
pub struct PageFilter {
    pub application_id: Option<Uuid>,
}

// ---------------------------------------------------------------- component

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: Uuid,
    pub page_id: Option<Uuid>,
    pub name: String,
    pub component_type: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COMPONENT_COLS: &str =
    "id, page_id, name, component_type, description, team_id, metadata, created_at, updated_at";

/// Fields for a new component. `metadata` defaults to `{}`.
#[derive(Debug, Clone, Default)]
pub struct NewComponent {
    pub page_id: Option<Uuid>,
    pub name: String,
    pub component_type: Option<String>,
    pub description: Option<String>,
    pub team_id: Option<Uuid>,
    pub metadata: Option<serde_json::Value>,
}

/// Fields to change on a component; `None` leaves a field as it is (so a text
/// field cannot be cleared). The parent never changes.
#[derive(Debug, Clone, Default)]
pub struct ComponentUpdate {
    pub name: Option<String>,
    pub component_type: Option<String>,
    pub description: Option<String>,
    pub team: TeamChange,
    pub metadata: Option<serde_json::Value>,
}

/// Filters for listing components.
#[derive(Debug, Clone, Copy, Default)]
pub struct ComponentFilter {
    pub page_id: Option<Uuid>,
}

// ---------------------------------------------------------------- trait

/// Catalog operations inside one organization's transaction.
pub trait CatalogExt {
    fn create_experience(
        &mut self,
        new: &NewExperience,
    ) -> impl Future<Output = Result<Experience>> + Send;
    fn get_experience(&mut self, id: Uuid) -> impl Future<Output = Result<Experience>> + Send;
    fn list_experiences(
        &mut self,
        page: paging::Page,
    ) -> impl Future<Output = Result<(Vec<Experience>, i64)>> + Send;
    fn update_experience(
        &mut self,
        id: Uuid,
        update: &ExperienceUpdate,
    ) -> impl Future<Output = Result<Experience>> + Send;
    fn delete_experience(&mut self, id: Uuid) -> impl Future<Output = Result<()>> + Send;
    fn create_application(
        &mut self,
        new: &NewApplication,
    ) -> impl Future<Output = Result<Application>> + Send;
    fn get_application(&mut self, id: Uuid) -> impl Future<Output = Result<Application>> + Send;
    fn list_applications(
        &mut self,
        filter: ApplicationFilter,
        page: paging::Page,
    ) -> impl Future<Output = Result<(Vec<Application>, i64)>> + Send;
    fn update_application(
        &mut self,
        id: Uuid,
        update: &ApplicationUpdate,
    ) -> impl Future<Output = Result<Application>> + Send;
    fn delete_application(&mut self, id: Uuid) -> impl Future<Output = Result<()>> + Send;
    fn create_page(&mut self, new: &NewPage) -> impl Future<Output = Result<Page>> + Send;
    fn get_page(&mut self, id: Uuid) -> impl Future<Output = Result<Page>> + Send;
    fn list_pages(
        &mut self,
        filter: PageFilter,
        page: paging::Page,
    ) -> impl Future<Output = Result<(Vec<Page>, i64)>> + Send;
    fn update_page(
        &mut self,
        id: Uuid,
        update: &PageUpdate,
    ) -> impl Future<Output = Result<Page>> + Send;
    fn delete_page(&mut self, id: Uuid) -> impl Future<Output = Result<()>> + Send;
    fn create_component(
        &mut self,
        new: &NewComponent,
    ) -> impl Future<Output = Result<Component>> + Send;
    fn get_component(&mut self, id: Uuid) -> impl Future<Output = Result<Component>> + Send;
    fn list_components(
        &mut self,
        filter: ComponentFilter,
        page: paging::Page,
    ) -> impl Future<Output = Result<(Vec<Component>, i64)>> + Send;
    fn update_component(
        &mut self,
        id: Uuid,
        update: &ComponentUpdate,
    ) -> impl Future<Output = Result<Component>> + Send;
    fn delete_component(&mut self, id: Uuid) -> impl Future<Output = Result<()>> + Send;
}

impl CatalogExt for Tx<'_> {
    async fn create_experience(&mut self, new: &NewExperience) -> Result<Experience> {
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO experiences (org_id, name, description, team_id, metadata, created_at, updated_at) VALUES ($1, $2, $3, $4, COALESCE($5, '{{}}'::jsonb), \
             clock_timestamp(), clock_timestamp()) \
             RETURNING {EXPERIENCE_COLS}"
        ))
        .bind(org)
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.team_id)
        .bind(&new.metadata)
        .fetch_one(self.conn())
        .await
        .map_err(Error::Database)
    }

    async fn get_experience(&mut self, id: Uuid) -> Result<Experience> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {EXPERIENCE_COLS} FROM experiences WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "experience" })
    }

    async fn list_experiences(&mut self, page: paging::Page) -> Result<(Vec<Experience>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {EXPERIENCE_COLS} FROM experiences WHERE org_id = $1 \
             ORDER BY created_at DESC, id LIMIT $2 OFFSET $3"
        ))
        .bind(org)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) = sqlx::query_as("SELECT count(*) FROM experiences WHERE org_id = $1")
            .bind(org)
            .fetch_one(self.conn())
            .await?;
        Ok((rows, total))
    }

    async fn update_experience(
        &mut self,
        id: Uuid,
        update: &ExperienceUpdate,
    ) -> Result<Experience> {
        let org = self.org();
        let (change, team) = update.team.binds();
        sqlx::query_as(&format!(
            "UPDATE experiences SET name = COALESCE($3, name), description = COALESCE($4, description), team_id = CASE WHEN $5 THEN $6 ELSE team_id END, metadata = COALESCE($7, metadata), \
             updated_at = clock_timestamp() WHERE org_id = $1 AND id = $2 RETURNING {EXPERIENCE_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&update.name)
        .bind(&update.description)
        .bind(change)
        .bind(team)
        .bind(&update.metadata)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "experience" })
    }

    /// Deletes the experience and, by cascade, everything beneath it.
    async fn delete_experience(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM experiences WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "experience" });
        }
        Ok(())
    }

    async fn create_application(&mut self, new: &NewApplication) -> Result<Application> {
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO applications (org_id, experience_id, name, platform, description, team_id, metadata, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '{{}}'::jsonb), \
             clock_timestamp(), clock_timestamp()) \
             RETURNING {APPLICATION_COLS}"
        ))
        .bind(org)
        .bind(new.experience_id)
        .bind(&new.name)
        .bind(&new.platform)
        .bind(&new.description)
        .bind(new.team_id)
        .bind(&new.metadata)
        .fetch_one(self.conn())
        .await
        .map_err(|e| on_foreign_key(e, || Error::NotFound { kind: "experience" }))
    }

    async fn get_application(&mut self, id: Uuid) -> Result<Application> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {APPLICATION_COLS} FROM applications WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound {
            kind: "application",
        })
    }

    async fn list_applications(
        &mut self,
        filter: ApplicationFilter,
        page: paging::Page,
    ) -> Result<(Vec<Application>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {APPLICATION_COLS} FROM applications \
             WHERE org_id = $1 AND ($2::uuid IS NULL OR experience_id = $2) \
             ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(filter.experience_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM applications WHERE org_id = $1 AND ($2::uuid IS NULL OR experience_id = $2)",
        )
        .bind(org)
        .bind(filter.experience_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn update_application(
        &mut self,
        id: Uuid,
        update: &ApplicationUpdate,
    ) -> Result<Application> {
        let org = self.org();
        let (change, team) = update.team.binds();
        sqlx::query_as(&format!(
            "UPDATE applications SET name = COALESCE($3, name), platform = COALESCE($4, platform), description = COALESCE($5, description), team_id = CASE WHEN $6 THEN $7 ELSE team_id END, metadata = COALESCE($8, metadata), \
             updated_at = clock_timestamp() WHERE org_id = $1 AND id = $2 RETURNING {APPLICATION_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&update.name)
        .bind(&update.platform)
        .bind(&update.description)
        .bind(change)
        .bind(team)
        .bind(&update.metadata)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "application" })
    }

    /// Deletes the application and, by cascade, everything beneath it.
    async fn delete_application(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM applications WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound {
                kind: "application",
            });
        }
        Ok(())
    }

    async fn create_page(&mut self, new: &NewPage) -> Result<Page> {
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO pages (org_id, application_id, name, path, description, team_id, metadata, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '{{}}'::jsonb), \
             clock_timestamp(), clock_timestamp()) \
             RETURNING {PAGE_COLS}"
        ))
        .bind(org)
        .bind(new.application_id)
        .bind(&new.name)
        .bind(&new.path)
        .bind(&new.description)
        .bind(new.team_id)
        .bind(&new.metadata)
        .fetch_one(self.conn())
        .await
        .map_err(|e| on_foreign_key(e, || Error::NotFound { kind: "application" }))
    }

    async fn get_page(&mut self, id: Uuid) -> Result<Page> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {PAGE_COLS} FROM pages WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "page" })
    }

    async fn list_pages(
        &mut self,
        filter: PageFilter,
        page: paging::Page,
    ) -> Result<(Vec<Page>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {PAGE_COLS} FROM pages \
             WHERE org_id = $1 AND ($2::uuid IS NULL OR application_id = $2) \
             ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(filter.application_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM pages WHERE org_id = $1 AND ($2::uuid IS NULL OR application_id = $2)",
        )
        .bind(org)
        .bind(filter.application_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn update_page(&mut self, id: Uuid, update: &PageUpdate) -> Result<Page> {
        let org = self.org();
        let (change, team) = update.team.binds();
        sqlx::query_as(&format!(
            "UPDATE pages SET name = COALESCE($3, name), path = COALESCE($4, path), description = COALESCE($5, description), team_id = CASE WHEN $6 THEN $7 ELSE team_id END, metadata = COALESCE($8, metadata), \
             updated_at = clock_timestamp() WHERE org_id = $1 AND id = $2 RETURNING {PAGE_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&update.name)
        .bind(&update.path)
        .bind(&update.description)
        .bind(change)
        .bind(team)
        .bind(&update.metadata)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "page" })
    }

    /// Deletes the page and, by cascade, everything beneath it.
    async fn delete_page(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM pages WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "page" });
        }
        Ok(())
    }

    async fn create_component(&mut self, new: &NewComponent) -> Result<Component> {
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO components (org_id, page_id, name, component_type, description, team_id, metadata, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, '{{}}'::jsonb), \
             clock_timestamp(), clock_timestamp()) \
             RETURNING {COMPONENT_COLS}"
        ))
        .bind(org)
        .bind(new.page_id)
        .bind(&new.name)
        .bind(&new.component_type)
        .bind(&new.description)
        .bind(new.team_id)
        .bind(&new.metadata)
        .fetch_one(self.conn())
        .await
        .map_err(|e| on_foreign_key(e, || Error::NotFound { kind: "page" }))
    }

    async fn get_component(&mut self, id: Uuid) -> Result<Component> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {COMPONENT_COLS} FROM components WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "component" })
    }

    async fn list_components(
        &mut self,
        filter: ComponentFilter,
        page: paging::Page,
    ) -> Result<(Vec<Component>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {COMPONENT_COLS} FROM components \
             WHERE org_id = $1 AND ($2::uuid IS NULL OR page_id = $2) \
             ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(filter.page_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let (total,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM components WHERE org_id = $1 AND ($2::uuid IS NULL OR page_id = $2)",
        )
        .bind(org)
        .bind(filter.page_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn update_component(&mut self, id: Uuid, update: &ComponentUpdate) -> Result<Component> {
        let org = self.org();
        let (change, team) = update.team.binds();
        sqlx::query_as(&format!(
            "UPDATE components SET name = COALESCE($3, name), component_type = COALESCE($4, component_type), description = COALESCE($5, description), team_id = CASE WHEN $6 THEN $7 ELSE team_id END, metadata = COALESCE($8, metadata), \
             updated_at = clock_timestamp() WHERE org_id = $1 AND id = $2 RETURNING {COMPONENT_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&update.name)
        .bind(&update.component_type)
        .bind(&update.description)
        .bind(change)
        .bind(team)
        .bind(&update.metadata)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "component" })
    }

    /// Deletes the component and, by cascade, everything beneath it.
    async fn delete_component(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM components WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "component" });
        }
        Ok(())
    }
}
