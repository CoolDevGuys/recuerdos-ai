//! The category set as the model sees it, and how to get back to a real
//! [`Category`] from whatever it actually returns.
//!
//! Two jobs, and the second is the one that matters. A schema `enum`
//! constrains a good model and is politely ignored by a small local one,
//! so *something* has to decide what `"preferences"` or `"user_pref"`
//! means. Rejecting the candidate would throw away a memory over a label;
//! inventing a new category would fragment the taxonomy, which is the
//! failure this whole design exists to prevent (see `category.rs`). So
//! unknown names are pulled to the nearest real category, and the caller
//! is told it happened.

use crate::memories::domain::category::{Category, DEFAULT_CATEGORIES};

/// Where a name that is not a known category ends up.
///
/// `fact.project` because it is the least presumptuous: it asserts that
/// something is true of the user's work without claiming to know it is a
/// standing preference or a decision, which are the labels that change
/// how much weight a recall gives a line.
const FALLBACK: Category = Category::FactProject;

pub struct Taxonomy {
    /// Deployment-defined categories from
    /// `[understanding.taxonomy].extra_categories`.
    extras: Vec<String>,
}

/// The result of resolving a model-supplied category name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCategory {
    pub category: Category,
    /// False when the model named something outside the taxonomy and this
    /// is a best guess. Callers log it: a model that guesses constantly
    /// means the prompt or the taxonomy needs work, and that signal is
    /// invisible if the correction is silent.
    pub exact: bool,
}

impl Taxonomy {
    pub fn new(extras: Vec<String>) -> Self {
        Self {
            extras: extras
                .into_iter()
                .map(|extra| extra.trim().to_ascii_lowercase())
                .filter(|extra| !extra.is_empty())
                .collect(),
        }
    }

    /// Every category name, built-ins first.
    pub fn names(&self) -> Vec<String> {
        DEFAULT_CATEGORIES
            .iter()
            .map(|category| category.as_str().to_string())
            .chain(self.extras.iter().cloned())
            .collect()
    }

    pub fn extras(&self) -> &[String] {
        &self.extras
    }

    /// The taxonomy as prompt text: one line per category, with the
    /// guidance from `category.rs` attached.
    ///
    /// Descriptions rather than a bare list because the names alone are
    /// ambiguous — `experience` and `fact.project` both sound like "a
    /// thing that happened" until you are told one is an outcome and the
    /// other is a standing truth.
    pub fn describe(&self) -> String {
        let mut lines: Vec<String> = DEFAULT_CATEGORIES
            .iter()
            .map(|category| format!("- {}: {}", category.as_str(), describe(category)))
            .collect();

        lines.extend(self.extras.iter().map(|extra| {
            format!(
                "- {extra}: a category specific to this deployment; use it when it plainly fits"
            )
        }));

        lines.join("\n")
    }

    /// Resolves a model-supplied name to a real category.
    ///
    /// Exact match first, then a prefix match on the dotted family
    /// (`preference.tooling` → `preference.coding`), then the fallback.
    /// The prefix step is worth having because the families are where a
    /// model's guesses cluster: it reliably knows something is a
    /// preference and unreliably knows which kind.
    pub fn resolve(&self, raw: &str) -> ResolvedCategory {
        if let Ok(category) = Category::parse_with_extras(raw, &self.extras) {
            return ResolvedCategory {
                category,
                exact: true,
            };
        }

        let name = raw.trim().to_ascii_lowercase();
        let family = name.split('.').next().unwrap_or_default();

        let nearest = (!family.is_empty())
            .then(|| {
                DEFAULT_CATEGORIES
                    .iter()
                    .find(|category| {
                        category
                            .as_str()
                            .split('.')
                            .next()
                            .is_some_and(|known| known == family)
                    })
                    .cloned()
            })
            .flatten()
            .unwrap_or(FALLBACK);

        ResolvedCategory {
            category: nearest,
            exact: false,
        }
    }
}

fn describe(category: &Category) -> &'static str {
    match category {
        Category::PreferenceCoding => {
            "a standing preference about code — tooling, style, patterns \
             (\"always uses pnpm\", \"forbids default exports\")"
        }
        Category::PreferencePersonal => {
            "a standing preference about life or working style \
             (\"vegetarian\", \"no meetings before 10am\")"
        }
        Category::Decision => "a decision that was made, together with why",
        Category::FactProject => {
            "a durable truth about the project or its stack \
             (\"the backend runs on Hetzner\")"
        }
        Category::FactPerson => "a durable truth about a person, relationship or role",
        Category::Experience => {
            "something that happened and what was learned from it \
             (\"the pgvector migration failed because of the index size\")"
        }
        Category::Skill => "a procedure worth repeating, described so it can be followed again",
        Category::Reference => "a pointer outward — a URL, a ticket, a dashboard",
        Category::Custom(_) => "a deployment-defined category",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_resolves_to_itself() {
        let taxonomy = Taxonomy::new(vec![]);
        for category in DEFAULT_CATEGORIES {
            let resolved = taxonomy.resolve(category.as_str());
            assert_eq!(&resolved.category, category);
            assert!(resolved.exact, "{} was treated as a guess", category);
        }
    }

    #[test]
    fn a_configured_extra_resolves_exactly() {
        let taxonomy = Taxonomy::new(vec!["fact.homelab".to_string()]);
        let resolved = taxonomy.resolve("fact.homelab");

        assert_eq!(resolved.category.as_str(), "fact.homelab");
        assert!(resolved.exact);
    }

    #[test]
    fn an_unknown_name_lands_in_its_family_rather_than_being_thrown_away() {
        // The common model failure: right family, invented leaf. Losing
        // the memory over that would be a bad trade.
        let taxonomy = Taxonomy::new(vec![]);

        let resolved = taxonomy.resolve("preference.tooling");
        assert_eq!(resolved.category, Category::PreferenceCoding);
        assert!(!resolved.exact, "a guess must be reported as a guess");
    }

    #[test]
    fn a_name_with_no_recognisable_family_falls_back() {
        let taxonomy = Taxonomy::new(vec![]);
        let resolved = taxonomy.resolve("vibes");

        assert_eq!(resolved.category, Category::FactProject);
        assert!(!resolved.exact);
    }

    #[test]
    fn an_empty_name_falls_back_rather_than_panicking() {
        let taxonomy = Taxonomy::new(vec![]);
        assert_eq!(taxonomy.resolve("   ").category, Category::FactProject);
    }

    #[test]
    fn the_description_names_every_category_and_says_what_it_is_for() {
        // The names alone are ambiguous — a model told only "experience"
        // and "fact.project" cannot tell an outcome from a standing truth.
        let taxonomy = Taxonomy::new(vec!["fact.homelab".to_string()]);
        let described = taxonomy.describe();

        for name in taxonomy.names() {
            assert!(
                described.contains(&name),
                "{name} missing from:\n{described}"
            );
        }
        assert!(
            described.contains("pnpm"),
            "descriptions should be concrete"
        );
    }

    #[test]
    fn extras_are_normalised_so_config_casing_does_not_matter() {
        let taxonomy = Taxonomy::new(vec!["  Fact.Homelab  ".to_string(), "   ".to_string()]);

        assert_eq!(taxonomy.extras(), ["fact.homelab"]);
        assert!(taxonomy.resolve("fact.homelab").exact);
    }
}
