//! The taxonomy a pipeline offers the model and holds it to, read per call
//! from the user's own catalog (V10): a category created in the web UI is
//! valid for that user's very next ingest.

use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category_catalog::CategoryCatalog;
use crate::shared::error::Result;
use crate::understanding::domain::taxonomy::{Taxonomy, TaxonomySource};
use std::collections::HashMap;
use std::sync::Arc;

/// The taxonomy each user curated, read from their catalog.
pub struct CatalogTaxonomy {
    catalog: Arc<dyn CategoryCatalog>,
}

impl CatalogTaxonomy {
    pub fn new(catalog: Arc<dyn CategoryCatalog>) -> Self {
        Self { catalog }
    }
}

impl TaxonomySource for CatalogTaxonomy {
    fn for_user(&self, context: &UserContext) -> Result<Taxonomy> {
        let mut extras = Vec::new();
        let mut labels = HashMap::new();
        for entry in self.catalog.entries(context)? {
            if entry.category.is_built_in() {
                // A built-in's label is display-only and stays out of the
                // prompt: the model files by meaning, and the descriptions
                // below are the meaning.
                continue;
            }
            let id = entry.category.as_str().to_string();
            if let Some(label) = entry.label {
                labels.insert(id.clone(), label);
            }
            extras.push(id);
        }
        Ok(Taxonomy::for_custom(extras, labels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memories::application::test_doubles::Fixture;

    #[test]
    fn a_users_catalog_reaches_the_prompt_with_its_labels() {
        let fixture = Fixture::new();
        fixture
            .category_curator(&[])
            .create(&fixture.alex, "health.sleep", Some("Sleep log"))
            .unwrap();
        let source = CatalogTaxonomy::new(fixture.catalog(&[]));

        let alex = source.for_user(&fixture.alex).unwrap();
        let sam = source.for_user(&fixture.sam).unwrap();

        assert!(alex.resolve("health.sleep").exact);
        assert!(alex.describe().contains("health.sleep (Sleep log)"));
        assert!(
            !sam.resolve("health.sleep").exact,
            "taxonomies are per user"
        );
    }
}
