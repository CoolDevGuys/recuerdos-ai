//! The structure of a memory saved before structure existed
//! (ai-specs/memory-structure-plan.md §8).
//!
//! Every memory stored before V11 has only a category. This maps it to a
//! kind, a topic and a target, deterministically, and flags what it cannot
//! place for the user to review. It is the one copy of the mapping: the
//! startup backfill writes it, a row the backfill has not reached is read
//! through it, and until the save path takes a structure (S1) a new memory
//! is given one through it too.
//!
//! What it cannot know it does not guess. A project decision has no project
//! name on record, so it is filed as `general` and flagged; a coding
//! preference may really be one project's convention, so it is flagged too.

use super::applies_to::{AppliesTo, Target};
use super::category::Category;
use super::kind::Kind;
use super::memory_structure::{MemoryStructure, Origin};
use super::topic::Topic;

/// Tags that point at a topic, checked in the memory's own tag order. Only
/// unambiguous words: a tag missing from here costs a fallback and a review
/// flag, a wrong one costs a misfiled memory.
const TOPIC_TAGS: &[(&str, Topic)] = &[
    ("coding", Topic::Coding),
    ("code", Topic::Coding),
    ("typescript", Topic::Coding),
    ("javascript", Topic::Coding),
    ("python", Topic::Coding),
    ("rust", Topic::Coding),
    ("php", Topic::Coding),
    ("go", Topic::Coding),
    ("testing", Topic::Coding),
    ("tests", Topic::Coding),
    ("frontend", Topic::Coding),
    ("backend", Topic::Coding),
    ("architecture", Topic::Coding),
    ("api", Topic::Coding),
    ("git", Topic::Coding),
    ("database", Topic::Coding),
    ("sql", Topic::Coding),
    ("tooling", Topic::Coding),
    ("infra", Topic::Tech),
    ("infrastructure", Topic::Tech),
    ("server", Topic::Tech),
    ("servers", Topic::Tech),
    ("homelab", Topic::Tech),
    ("network", Topic::Tech),
    ("networking", Topic::Tech),
    ("docker", Topic::Tech),
    ("kubernetes", Topic::Tech),
    ("linux", Topic::Tech),
    ("nas", Topic::Tech),
    ("vps", Topic::Tech),
    ("devops", Topic::Tech),
    ("work", Topic::Work),
    ("career", Topic::Work),
    ("meetings", Topic::Work),
    ("family", Topic::Family),
    ("people", Topic::People),
    ("team", Topic::People),
    ("friends", Topic::People),
    ("health", Topic::Health),
    ("sleep", Topic::Health),
    ("fitness", Topic::Health),
    ("exercise", Topic::Health),
    ("medical", Topic::Health),
    ("sport", Topic::Lifestyle),
    ("hobby", Topic::Lifestyle),
    ("hobbies", Topic::Lifestyle),
    ("books", Topic::Lifestyle),
    ("home", Topic::Home),
    ("house", Topic::Home),
    ("finance", Topic::Finance),
    ("money", Topic::Finance),
    ("travel", Topic::Travel),
    ("food", Topic::Food),
    ("cooking", Topic::Food),
    ("diet", Topic::Food),
];

/// Old category words that name a topic under another id.
const TOPIC_ALIASES: &[(&str, Topic)] = &[
    ("personal", Topic::Lifestyle),
    ("project", Topic::Coding),
    ("person", Topic::People),
];

/// The derived structure: always valid, flagged when it may be wrong.
pub fn legacy_structure(category: &Category, tags: &[String]) -> MemoryStructure {
    let (kind, topic, applies_to, needs_review) = match category {
        // May be one project's convention rather than the user's standard:
        // that is the case this whole change exists for.
        Category::PreferenceCoding => (Kind::Preference, Topic::Coding, AppliesTo::Me, true),
        Category::PreferencePersonal => {
            let (topic, _) = topic_from_tags(tags, Topic::Lifestyle);
            (Kind::Preference, topic, AppliesTo::Me, false)
        }
        // The project they were about is not on record.
        Category::Decision => {
            let (topic, _) = topic_from_tags(tags, Topic::Work);
            (Kind::Decision, topic, AppliesTo::General, true)
        }
        Category::FactProject => {
            let (topic, _) = topic_from_tags(tags, Topic::Coding);
            (Kind::Fact, topic, AppliesTo::General, true)
        }
        Category::FactPerson => {
            let topic = if tags.iter().any(|tag| tag == "family") {
                Topic::Family
            } else {
                Topic::People
            };
            (Kind::Fact, topic, AppliesTo::General, false)
        }
        Category::Experience => {
            let (topic, guessed) = topic_from_tags(tags, Topic::Work);
            (Kind::Experience, topic, AppliesTo::General, guessed)
        }
        Category::Skill => {
            let (topic, guessed) = topic_from_tags(tags, Topic::Work);
            (Kind::Procedure, topic, AppliesTo::General, guessed)
        }
        Category::Reference => {
            let (topic, guessed) = topic_from_tags(tags, Topic::Work);
            (Kind::Reference, topic, AppliesTo::General, guessed)
        }
        Category::Custom(id) => {
            let (kind, topic) = custom_kind_and_topic(id, tags);
            // No project is on record, so `general` where the kind allows it
            // and `me` where it does not (a preference, a goal).
            let applies_to = if kind.allows(Target::General) {
                AppliesTo::General
            } else {
                AppliesTo::Me
            };
            (kind, topic, applies_to, true)
        }
    };

    MemoryStructure::new(kind, topic, applies_to, Origin::UserSaid, None)
        .expect("every legacy mapping is an allowed pair")
        .flagged_for_review(needs_review)
}

/// The topic named by the first tag that names one, else the fallback.
/// The flag says the fallback was used.
fn topic_from_tags(tags: &[String], fallback: Topic) -> (Topic, bool) {
    tags.iter()
        .find_map(|tag| {
            TOPIC_TAGS
                .iter()
                .find(|(word, _)| word == tag)
                .map(|(_, topic)| topic.clone())
        })
        .map_or((fallback, true), |topic| (topic, false))
}

/// A user's category (`health.sleep`, `fact.homelab`, `homelab`): its first
/// segment is a kind when it names one, and the next segment is the topic;
/// otherwise the memory is a fact and the first segment is the topic.
fn custom_kind_and_topic(id: &str, tags: &[String]) -> (Kind, Topic) {
    let mut segments = id.split('.').filter(|s| !s.is_empty());
    let head = segments.next().unwrap_or_default();

    let (kind, candidate) = match legacy_kind(head) {
        Some(kind) => (kind, segments.next()),
        None => (Kind::Fact, Some(head)),
    };
    // Kinds that cannot stand without a project become their nearest
    // project-free kind; the memory is flagged for review either way.
    let kind = match kind {
        Kind::Convention => Kind::Rule,
        other => other,
    };

    let topic = candidate
        .and_then(|word| {
            Topic::built_in(word)
                .or_else(|| {
                    TOPIC_ALIASES
                        .iter()
                        .find(|(alias, _)| *alias == word)
                        .map(|(_, topic)| topic.clone())
                })
                .or_else(|| Topic::custom(word).ok())
        })
        .unwrap_or_else(|| topic_from_tags(tags, Topic::Work).0);

    (kind, topic)
}

/// The old category a structured memory is filed under, for what still reads
/// categories (recall's category filter, the map, the enriched vector header)
/// until topics replace them (plan phases S4–S5). Lossy by design: the
/// structure is the record; this only keeps the old readers working.
pub fn category_for(kind: Kind, topic: &Topic) -> Category {
    let technical = matches!(topic, Topic::Coding | Topic::Tech);
    match kind {
        Kind::Preference | Kind::Rule | Kind::Convention => {
            if technical {
                Category::PreferenceCoding
            } else {
                Category::PreferencePersonal
            }
        }
        Kind::Decision => Category::Decision,
        Kind::Fact | Kind::Goal | Kind::Plan => match topic {
            Topic::People | Topic::Family => Category::FactPerson,
            _ => Category::FactProject,
        },
        Kind::Experience | Kind::Learning => Category::Experience,
        Kind::Procedure => Category::Skill,
        Kind::Reference => Category::Reference,
    }
}

fn legacy_kind(word: &str) -> Option<Kind> {
    match word {
        "skill" => Some(Kind::Procedure),
        other => other.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    fn mapped(category: Category, words: &[&str]) -> (Kind, Topic, Target, bool) {
        let structure = legacy_structure(&category, &tags(words));
        (
            structure.kind(),
            structure.topic().clone(),
            structure.applies_to().target(),
            structure.needs_review(),
        )
    }

    #[test]
    fn the_built_in_categories_map_as_the_plan_says() {
        // ai-specs/memory-structure-plan.md §8, row by row.
        assert_eq!(
            mapped(Category::PreferenceCoding, &[]),
            (Kind::Preference, Topic::Coding, Target::Me, true)
        );
        assert_eq!(
            mapped(Category::PreferencePersonal, &[]),
            (Kind::Preference, Topic::Lifestyle, Target::Me, false)
        );
        assert_eq!(
            mapped(Category::Decision, &[]),
            (Kind::Decision, Topic::Work, Target::General, true)
        );
        assert_eq!(
            mapped(Category::FactProject, &[]),
            (Kind::Fact, Topic::Coding, Target::General, true)
        );
        assert_eq!(
            mapped(Category::FactPerson, &[]),
            (Kind::Fact, Topic::People, Target::General, false)
        );
        assert_eq!(
            mapped(Category::Skill, &["docker"]),
            (Kind::Procedure, Topic::Tech, Target::General, false)
        );
        assert_eq!(
            mapped(Category::Reference, &["accessibility"]),
            (Kind::Reference, Topic::Work, Target::General, true),
            "no tag names a topic: fallback, flagged"
        );
    }

    #[test]
    fn the_first_tag_that_names_a_topic_wins() {
        assert_eq!(
            mapped(Category::Experience, &["quill", "database", "infra"]).1,
            Topic::Coding
        );
        assert_eq!(
            mapped(Category::Decision, &["infra", "database"]).1,
            Topic::Tech
        );
        assert_eq!(
            mapped(Category::PreferencePersonal, &["food"]).1,
            Topic::Food
        );
        assert_eq!(mapped(Category::FactPerson, &["family"]).1, Topic::Family);
    }

    #[test]
    fn a_user_category_splits_into_kind_and_topic() {
        let custom = |id: &str| Category::Custom(id.to_string());

        assert_eq!(
            mapped(custom("health.sleep"), &[]),
            (Kind::Fact, Topic::Health, Target::General, true)
        );
        assert_eq!(
            mapped(custom("fact.homelab"), &[]),
            (
                Kind::Fact,
                Topic::Custom("homelab".to_string()),
                Target::General,
                true
            )
        );
        assert_eq!(
            mapped(custom("preference.food"), &[]),
            (Kind::Preference, Topic::Food, Target::Me, true)
        );
        assert_eq!(
            mapped(custom("skill.personal"), &[]),
            (Kind::Procedure, Topic::Lifestyle, Target::General, true)
        );
        assert_eq!(
            mapped(custom("convention.coding"), &[]),
            (Kind::Rule, Topic::Coding, Target::General, true),
            "no project on record, so the nearest project-free kind"
        );
        assert_eq!(
            mapped(custom("goal.health"), &[]),
            (Kind::Goal, Topic::Health, Target::Me, true)
        );
        assert_eq!(
            mapped(custom("decision"), &["travel"]),
            (Kind::Decision, Topic::Travel, Target::General, true)
        );
    }

    #[test]
    fn a_structure_files_under_the_nearest_old_category() {
        assert_eq!(
            category_for(Kind::Preference, &Topic::Coding),
            Category::PreferenceCoding
        );
        assert_eq!(
            category_for(Kind::Rule, &Topic::Food),
            Category::PreferencePersonal
        );
        assert_eq!(
            category_for(Kind::Convention, &Topic::Coding),
            Category::PreferenceCoding
        );
        assert_eq!(
            category_for(Kind::Fact, &Topic::Family),
            Category::FactPerson
        );
        assert_eq!(
            category_for(Kind::Plan, &Topic::Health),
            Category::FactProject
        );
        assert_eq!(
            category_for(Kind::Learning, &Topic::Coding),
            Category::Experience
        );
        assert_eq!(category_for(Kind::Procedure, &Topic::Tech), Category::Skill);
    }

    #[test]
    fn every_mapping_is_a_valid_structure_said_by_the_user() {
        let mut categories: Vec<Category> =
            crate::memories::domain::category::DEFAULT_CATEGORIES.to_vec();
        for id in [
            "health.sleep",
            "x",
            "goal.health",
            "plan",
            "convention",
            "9.9",
        ] {
            categories.push(Category::Custom(id.to_string()));
        }
        for category in categories {
            let structure = legacy_structure(&category, &[]);
            assert!(
                structure.kind().allows(structure.applies_to().target()),
                "{category:?}"
            );
            assert_eq!(structure.origin(), Origin::UserSaid);
        }
    }
}
