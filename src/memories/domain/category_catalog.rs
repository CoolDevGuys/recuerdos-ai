//! Each user's own taxonomy: their custom categories, and the labels they
//! gave any category (ai-specs/web-ui-plan.md §4.2, migration V10).
//!
//! The eight built-ins are always valid and need no row; the catalog holds
//! what a user changed. Which memories are filed where is not the
//! catalog's business — moving members between categories is a memory
//! edit, done through the repository so it is audited and re-embedded like
//! every other edit (`application/category_curator.rs`).

use super::category::{Category, DEFAULT_CATEGORIES, MAX_CATEGORY_LABEL_LEN};
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};

/// One category as this user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub category: Category,
    /// The user's display label; `None` for the default wording.
    pub label: Option<String>,
}

impl CatalogEntry {
    /// The words to show: the user's label, else the default.
    pub fn display_label(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.category.label())
    }
}

pub trait CategoryCatalog: Send + Sync {
    /// The rows this user has: every custom category, and the built-ins
    /// they relabelled. Custom ones sorted by id.
    fn entries(&self, context: &UserContext) -> Result<Vec<CatalogEntry>>;

    /// Adds a custom category. `Conflict` when it already exists.
    fn create(&self, context: &UserContext, entry: &CatalogEntry) -> Result<()>;

    /// Sets (or, with `None`, clears) a category's display label. Works for
    /// built-ins, which gain a row the first time; `NotFound` for a custom
    /// id the user does not have.
    fn set_label(
        &self,
        context: &UserContext,
        category: &Category,
        label: Option<&str>,
    ) -> Result<()>;

    /// Removes a custom category's row. Its members must have been moved
    /// first; the catalog does not look. `NotFound` when absent.
    fn remove(&self, context: &UserContext, category: &Category) -> Result<()>;

    /// Every category this user may file a memory under: the built-ins in
    /// taxonomy order, then their custom ones, with display labels.
    fn all(&self, context: &UserContext) -> Result<Vec<CatalogEntry>> {
        let entries = self.entries(context)?;
        let mut all: Vec<CatalogEntry> = DEFAULT_CATEGORIES
            .iter()
            .map(|category| CatalogEntry {
                category: category.clone(),
                label: entries
                    .iter()
                    .find(|e| &e.category == category)
                    .and_then(|e| e.label.clone()),
            })
            .collect();
        all.extend(entries.into_iter().filter(|e| !e.category.is_built_in()));
        Ok(all)
    }

    /// This user's custom category ids — what
    /// [`Category::parse_with_extras`] accepts besides the built-ins.
    fn custom_ids(&self, context: &UserContext) -> Result<Vec<String>> {
        Ok(self
            .entries(context)?
            .into_iter()
            .filter(|e| !e.category.is_built_in())
            .map(|e| e.category.as_str().to_string())
            .collect())
    }

    /// Parses a category a caller named, against this user's taxonomy.
    fn resolve(&self, context: &UserContext, raw: &str) -> Result<Category> {
        Category::parse_with_extras(raw, &self.custom_ids(context)?)
    }
}

/// A label as a user typed it: trimmed; empty means "back to the default".
pub fn normalise_label(raw: Option<&str>) -> Result<Option<String>> {
    let Some(label) = raw.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    if label.chars().count() > MAX_CATEGORY_LABEL_LEN {
        return Err(RaError::Validation(format!(
            "a category label is at most {MAX_CATEGORY_LABEL_LEN} characters"
        )));
    }
    if label.chars().any(char::is_control) {
        return Err(RaError::Validation(
            "a category label cannot contain control characters".to_string(),
        ));
    }
    Ok(Some(label.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_label_means_the_default() {
        assert_eq!(normalise_label(Some("   ")).unwrap(), None);
        assert_eq!(normalise_label(None).unwrap(), None);
        assert_eq!(
            normalise_label(Some("  Sleep log ")).unwrap().as_deref(),
            Some("Sleep log")
        );
    }

    #[test]
    fn an_overlong_or_control_label_is_refused() {
        assert!(normalise_label(Some(&"x".repeat(65))).is_err());
        assert!(normalise_label(Some("a\nb")).is_err());
    }

    #[test]
    fn the_display_label_falls_back_to_the_default_wording() {
        let entry = CatalogEntry {
            category: Category::Decision,
            label: None,
        };
        assert_eq!(entry.display_label(), "Decision");
        let entry = CatalogEntry {
            label: Some("Choices".to_string()),
            ..entry
        };
        assert_eq!(entry.display_label(), "Choices");
    }
}
