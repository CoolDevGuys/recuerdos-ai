//! The `memory://profile` resource: an LLM-written briefing, cached
//! until the memories under it change.
//!
//! Replaces the internals of `ProfileAssembler` (Phase 3) without
//! touching the contract — same resource, same route, same markdown. The
//! assembler is still here, and still used: it is the fallback whenever
//! there is no model, and the thing this is measured against.
//!
//! # Why generate rather than assemble
//!
//! Assembly lists the top memories per category. It is honest and it is
//! free, but it scales badly against a fixed budget: forty coding
//! preferences truncate to eight arbitrary ones, and the eight that
//! survive are eight sentences where one would have done. A model can
//! say "uses pnpm, Vitest and Biome; no barrel files" in a line, which
//! is both shorter and more useful than any eight of the originals.
//!
//! # Why it is cached, and why the read never generates
//!
//! This is read at the start of every session, by every client, of every
//! agent. Generating on read would put a model call in front of every
//! session start — seconds of latency and a bill proportional to how
//! often the user opens their editor. Worse, that model call sits inside
//! the daemon's 30 s request-timeout, so a slow local provider turns a
//! stale digest into a 408 for the read rather than a wait: the failure
//! the shim then reports to the agent as an internal error.
//!
//! So generation is split from serving. [`execute`] is the read path and
//! never calls the model: it renders from whatever digests are cached,
//! stale ones included, and assembles when the cache is cold. [`refresh`]
//! is the write path and the only caller of the model: it regenerates the
//! digests whose memories have moved. A [`MemoryChangeObserver`] wires the
//! two together — the ingest worker and the nightly consolidation run call
//! `refresh` in the background after they change a user's memories, so the
//! cache the read serves is kept current off to the side.
//!
//! [`execute`]: ProfileDigestWriter::execute
//! [`refresh`]: ProfileDigestWriter::refresh
//!
//! # Why it never fails
//!
//! Every failure path falls back to assembly rather than erroring. A
//! provider outage should degrade the profile, not break session start
//! for every agent connected to the daemon. And because the read no longer
//! generates, even a provider that hangs past the request-timeout can only
//! make the served profile stale — never make the read fail.

use crate::consolidation::domain::digest_prompt::{digest_request, parse_digest, select};
use crate::consolidation::domain::profile_digest::{
    DOMAINS, Domain, Fingerprint, ProfileDigestStore, StoredDigest,
};
use crate::identity::domain::user_context::UserContext;
use crate::memories::application::profile_assembler::{DEFAULT_TOKEN_BUDGET, ProfileAssembler};
use crate::memories::domain::memory::Memory;
use crate::memories::domain::memory_change_observer::MemoryChangeObserver;
use crate::memories::domain::memory_repository::MemoryRepository;
use crate::shared::clock::Clock;
use crate::shared::error::Result;
use crate::understanding::domain::chat_model::ChatModel;
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Rough characters per token, as in `ProfileAssembler` — and the same
/// reasoning: a real tokenizer is not worth a dependency for a
/// truncation heuristic.
const CHARS_PER_TOKEN: usize = 4;

/// Rough tokens per English word, for turning the resource's token
/// budget into the word budget the prompt states. Generous, so the model
/// aims comfortably under rather than at the ceiling.
const TOKENS_PER_WORD: usize = 2;

pub struct ProfileDigestWriter {
    memories: Arc<dyn MemoryRepository>,
    digests: Arc<dyn ProfileDigestStore>,
    /// The Phase 3 assembler, kept as the fallback for every path where
    /// a model is unavailable or unhelpful.
    assembler: Arc<ProfileAssembler>,
    /// `None` when no provider is configured, in which case this use
    /// case is a thin pass-through to the assembler.
    model: Option<Arc<dyn ChatModel>>,
    clock: Arc<dyn Clock>,
    token_budget: usize,
}

impl ProfileDigestWriter {
    pub fn new(
        memories: Arc<dyn MemoryRepository>,
        digests: Arc<dyn ProfileDigestStore>,
        assembler: Arc<ProfileAssembler>,
        model: Option<Arc<dyn ChatModel>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            memories,
            digests,
            assembler,
            model,
            clock,
            token_budget: DEFAULT_TOKEN_BUDGET,
        }
    }

    /// The read path. Renders the profile from the cached digests and
    /// never calls the model, so it cannot be slowed — or timed out — by a
    /// provider. [`refresh`] is what keeps the cache it reads current.
    ///
    /// A cached digest is served whether or not it is still current: a
    /// digest one memory out of date is a better answer on the read path
    /// than a model call would be, and the background refresh will have it
    /// caught up shortly. Only when nothing is cached at all does this
    /// assemble, so a cold cache still returns the user their memories
    /// rather than a blank page.
    ///
    /// [`refresh`]: Self::refresh
    pub async fn execute(&self, context: &UserContext) -> Result<String> {
        // Without a model there is no digest cache to serve from — the
        // assembler is the whole profile, not a fallback.
        if self.model.is_none() {
            return self.assembler.execute(context);
        }

        let now = self.clock.now();
        let stored = self.memories.list(context, false)?;
        let active: Vec<&Memory> = stored
            .iter()
            .filter(|memory| memory.is_active_at(now))
            .collect();

        if active.is_empty() {
            // The assembler's empty-profile text tells an agent what to
            // do about it, which is more useful than a blank page.
            return self.assembler.execute(context);
        }

        let mut sections: Vec<(Domain, String)> = Vec::new();
        for domain in DOMAINS {
            let has_memories = active
                .iter()
                .any(|memory| Domain::of(memory.category()) == *domain);
            if !has_memories {
                continue;
            }

            match self.digests.find(context, *domain) {
                // A cached digest, current or stale. An empty one is the
                // model having said there was nothing worth an assistant's
                // attention here; that is an answer, so the domain is
                // simply omitted rather than assembled.
                Ok(Some(cached)) if !cached.content.is_empty() => {
                    sections.push((*domain, cached.content));
                }
                Ok(_) => {}
                Err(error) => {
                    // A broken cache should cost this domain its section,
                    // not fail the whole read.
                    tracing::warn!(%error, domain = domain.as_str(), "could not read a cached digest");
                }
            }
        }

        if sections.is_empty() {
            // The cache is cold — no domain has been generated yet, or a
            // read raced ahead of the first background refresh. Assembly
            // at least shows the user their memories until it lands.
            tracing::debug!("no cached digest to serve; falling back to assembly");
            return self.assembler.execute(context);
        }

        Ok(render(context.handle(), &sections, now, self.char_budget()))
    }

    /// The background path: regenerate the digests whose memories have
    /// moved, and cache them for the read path to serve.
    ///
    /// Called off to the side — by the ingest worker after it stores
    /// memories, and by the nightly consolidation run for every user it
    /// touches — never on the read. A domain whose fingerprint still
    /// matches is left alone and costs no model call, so refreshing a user
    /// nothing changed for is cheap; only stale domains are regenerated.
    ///
    /// Errors are swallowed per domain exactly as on the old read path: a
    /// provider outage should leave the last good digest in place, not
    /// propagate out of a background task.
    pub async fn refresh(&self, context: &UserContext) -> Result<()> {
        let Some(model) = self.model.as_ref() else {
            return Ok(());
        };

        let now = self.clock.now();
        let stored = self.memories.list(context, false)?;
        let active: Vec<&Memory> = stored
            .iter()
            .filter(|memory| memory.is_active_at(now))
            .collect();

        if active.is_empty() {
            return Ok(());
        }

        for domain in DOMAINS {
            let theirs: Vec<&Memory> = active
                .iter()
                .filter(|memory| Domain::of(memory.category()) == *domain)
                .copied()
                .collect();
            if theirs.is_empty() {
                continue;
            }

            // `digest_for` regenerates and caches when the fingerprint has
            // moved, and returns the cached digest untouched otherwise. We
            // want the caching side effect; the returned content is the
            // read path's business, so it is dropped here.
            let _ = self.digest_for(context, *domain, &theirs, model, now).await;
        }

        Ok(())
    }

    /// One domain's digest: the cached one if it still applies, a fresh
    /// one otherwise, or `None` if it could not be produced.
    async fn digest_for(
        &self,
        context: &UserContext,
        domain: Domain,
        memories: &[&Memory],
        model: &Arc<dyn ChatModel>,
        now: DateTime<Utc>,
    ) -> Option<String> {
        let fingerprint = Fingerprint::of(memories);

        match self.digests.find(context, domain) {
            Ok(Some(cached)) if cached.covers(&fingerprint) => {
                tracing::debug!(domain = domain.as_str(), "reusing a cached digest");
                return Some(cached.content);
            }
            Ok(_) => {}
            Err(error) => {
                // A broken cache should cost a regeneration, not the
                // profile.
                tracing::warn!(%error, "could not read a cached digest");
            }
        }

        let request = digest_request(domain, &select(memories), self.word_budget());
        let answer = match model.complete_structured(&request).await {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(%error, domain = domain.as_str(), "could not generate a digest");
                // Better a stale digest than none: the memories it was
                // built from are mostly still true.
                return self.stale(context, domain);
            }
        };

        let Some(content) = parse_digest(&answer) else {
            // The model was asked to return nothing when there is
            // nothing to say, so this is a legitimate answer rather than
            // a failure — and caching it stops the empty answer being
            // re-asked on every session start.
            let digest = StoredDigest {
                content: String::new(),
                fingerprint: fingerprint.render(),
                generated_at: now,
            };
            self.store(context, domain, &digest);
            return None;
        };

        let digest = StoredDigest {
            content,
            fingerprint: fingerprint.render(),
            generated_at: now,
        };
        self.store(context, domain, &digest);

        tracing::info!(domain = domain.as_str(), "regenerated a profile digest");
        Some(digest.content)
    }

    /// The cached digest regardless of whether it is current.
    fn stale(&self, context: &UserContext, domain: Domain) -> Option<String> {
        match self.digests.find(context, domain) {
            Ok(Some(cached)) if !cached.content.is_empty() => {
                tracing::debug!(domain = domain.as_str(), "serving a stale digest");
                Some(cached.content)
            }
            _ => None,
        }
    }

    /// A cache write failure is not worth failing a read over — it costs
    /// a regeneration next time, which is exactly what would have
    /// happened anyway.
    fn store(&self, context: &UserContext, domain: Domain, digest: &StoredDigest) {
        if let Err(error) = self.digests.save(context, domain, digest) {
            tracing::warn!(%error, domain = domain.as_str(), "could not cache a digest");
        }
    }

    fn char_budget(&self) -> usize {
        self.token_budget * CHARS_PER_TOKEN
    }

    /// Split across the domains, so two full-length sections cannot
    /// together blow the budget the resource promises.
    fn word_budget(&self) -> usize {
        self.token_budget / TOKENS_PER_WORD / DOMAINS.len()
    }
}

/// The writer is the sole subscriber to memory changes: a change is what
/// makes a digest stale, and refreshing it is exactly this. Keeping the
/// model call here, behind a background trigger, is what holds it off the
/// read path.
#[async_trait::async_trait]
impl MemoryChangeObserver for ProfileDigestWriter {
    async fn memories_changed(&self, context: &UserContext) {
        if let Err(error) = self.refresh(context).await {
            // Best-effort by contract: the read path serves the last good
            // digest regardless, so a failed refresh is a logged warning,
            // not a caller's problem.
            tracing::warn!(%error, user = context.handle(), "could not refresh the profile digest");
        }
    }
}

fn render(
    handle: &str,
    sections: &[(Domain, String)],
    now: DateTime<Utc>,
    char_budget: usize,
) -> String {
    let mut output = format!(
        "# Memory profile: {handle} (updated {})\n",
        now.format("%Y-%m-%d")
    );

    for (domain, content) in sections {
        let section = format!("\n## {}\n\n{}\n", domain.heading(), content.trim());

        // Whole sections or nothing. Truncating mid-sentence produces a
        // profile that reads as though the model was cut off, which an
        // agent has no way to tell from the model being wrong.
        if output.len() + section.len() > char_budget {
            tracing::debug!(
                domain = domain.as_str(),
                "dropping a digest section to stay inside the profile budget"
            );
            continue;
        }
        output.push_str(&section);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consolidation::application::test_doubles::InMemoryProfileDigestStore;
    use crate::memories::application::test_doubles::{Fixture, fixed_clock, new_memory};
    use crate::memories::domain::category::Category;
    use crate::understanding::application::scripted_chat_model::ScriptedChatModel;
    use serde_json::json;

    fn writer(
        fixture: &Fixture,
        model: Option<ScriptedChatModel>,
    ) -> (ProfileDigestWriter, Option<Arc<ScriptedChatModel>>) {
        let model = model.map(Arc::new);
        let digests: Arc<dyn ProfileDigestStore> = Arc::new(InMemoryProfileDigestStore::default());

        (
            ProfileDigestWriter::new(
                Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                digests,
                Arc::new(ProfileAssembler::new(
                    Arc::clone(&fixture.memories) as Arc<dyn MemoryRepository>,
                    fixed_clock(),
                )),
                model
                    .as_ref()
                    .map(|model| Arc::clone(model) as Arc<dyn ChatModel>),
                fixed_clock(),
            ),
            model,
        )
    }

    fn save(fixture: &Fixture, category: Category, content: &str) {
        let mut memory = new_memory(content);
        memory.category = category;
        fixture
            .saver()
            .execute(&fixture.alex, memory, "test")
            .unwrap();
    }

    fn coding_reply() -> serde_json::Value {
        json!({"digest": "- Uses pnpm, never npm or yarn\n- No barrel files"})
    }

    fn personal_reply() -> serde_json::Value {
        json!({"digest": "- Vegetarian"})
    }

    #[tokio::test]
    async fn the_digest_replaces_the_listing_but_not_the_contract() {
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");
        save(&fixture, Category::PreferencePersonal, "is vegetarian");

        let (writer, _) = writer(
            &fixture,
            Some(
                ScriptedChatModel::new()
                    .queue(coding_reply())
                    .queue(personal_reply()),
            ),
        );

        // The background refresh generates and caches; the read serves it.
        writer.refresh(&fixture.alex).await.unwrap();
        let profile = writer.execute(&fixture.alex).await.unwrap();

        // Same shape the assembler produced, so no client notices.
        assert!(profile.starts_with("# Memory profile: alex"), "{profile}");
        assert!(profile.contains("## How they work"), "{profile}");
        assert!(
            profile.contains("Uses pnpm, never npm or yarn"),
            "{profile}"
        );
        assert!(profile.contains("## About them"), "{profile}");
        assert!(profile.contains("Vegetarian"), "{profile}");
    }

    #[tokio::test]
    async fn an_unchanged_memory_set_is_not_regenerated() {
        // The reason it is cached at all: without it every change would
        // pay for a model call it did not need. A second refresh over an
        // unchanged set must reuse the digest rather than rebuild it.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let (writer, model) = writer(
            &fixture,
            Some(ScriptedChatModel::new().queue(coding_reply())),
        );
        let model = model.unwrap();

        writer.refresh(&fixture.alex).await.unwrap();
        let first = writer.execute(&fixture.alex).await.unwrap();
        writer.refresh(&fixture.alex).await.unwrap();
        let second = writer.execute(&fixture.alex).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(
            model.call_count(),
            1,
            "the digest was regenerated for an unchanged memory set"
        );
    }

    #[tokio::test]
    async fn the_read_path_never_calls_the_model_even_when_the_cache_is_stale() {
        // The whole point of the split: a read must not put a model call —
        // and so the request timeout — in front of session start. A stale
        // cache is served as-is; only a background refresh regenerates.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let (writer, model) = writer(
            &fixture,
            Some(ScriptedChatModel::new().queue(coding_reply())),
        );
        let model = model.unwrap();

        writer.refresh(&fixture.alex).await.unwrap();
        assert_eq!(model.call_count(), 1);

        // A new memory makes the cached coding digest stale. The read must
        // still not call the model — even though there are no further
        // scripted replies, so a regeneration attempt would error.
        save(&fixture, Category::PreferenceCoding, "prefers vitest");
        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert_eq!(
            model.call_count(),
            1,
            "the read path regenerated a stale digest instead of serving it"
        );
        assert!(
            profile.contains("Uses pnpm"),
            "the stale digest was not served: {profile}"
        );
    }

    #[tokio::test]
    async fn a_cold_cache_read_assembles_rather_than_generating() {
        // Before any refresh has run there is nothing to serve. The read
        // must return the user their memories via assembly, not block on a
        // model and not error.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let (writer, model) = writer(
            &fixture,
            Some(ScriptedChatModel::new().queue(coding_reply())),
        );

        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert_eq!(
            model.unwrap().call_count(),
            0,
            "the read path generated instead of assembling a cold cache"
        );
        assert!(
            profile.contains("prefers pnpm"),
            "assembly should list the memory verbatim: {profile}"
        );
    }

    #[tokio::test]
    async fn saving_a_memory_regenerates_only_the_domain_it_belongs_to() {
        // The whole reason the digest is split in two. A new coding
        // preference must not cost a rewrite of the personal profile.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");
        save(&fixture, Category::PreferencePersonal, "is vegetarian");

        let (writer, model) = writer(
            &fixture,
            Some(
                ScriptedChatModel::new()
                    .queue(coding_reply())
                    .queue(personal_reply())
                    // Only one further reply is queued: if the personal
                    // domain regenerated too, the model would run dry.
                    .queue(json!({"digest": "- Uses pnpm and Vitest"})),
            ),
        );

        writer.refresh(&fixture.alex).await.unwrap();
        save(&fixture, Category::PreferenceCoding, "prefers vitest");
        writer.refresh(&fixture.alex).await.unwrap();
        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert_eq!(model.unwrap().call_count(), 3);
        assert!(profile.contains("Uses pnpm and Vitest"), "{profile}");
        assert!(
            profile.contains("Vegetarian"),
            "the cached personal digest was lost: {profile}"
        );
    }

    #[tokio::test]
    async fn without_a_model_it_falls_back_to_assembly() {
        // The degraded default. The resource must still answer, and with
        // something useful.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let (writer, _) = writer(&fixture, None);

        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert!(profile.starts_with("# Memory profile: alex"), "{profile}");
        assert!(
            profile.contains("prefers pnpm"),
            "assembly should list the memory verbatim: {profile}"
        );
    }

    #[tokio::test]
    async fn a_provider_outage_during_refresh_leaves_the_last_good_digest() {
        // A refresh that cannot reach the provider must not wipe the cache:
        // the read path goes on serving the last good digest.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let (writer, _) = writer(
            &fixture,
            Some(ScriptedChatModel::new().queue(coding_reply())),
        );
        writer.refresh(&fixture.alex).await.unwrap();
        assert!(
            writer
                .execute(&fixture.alex)
                .await
                .unwrap()
                .contains("Uses pnpm")
        );

        // A new memory makes the cache stale, and the model is now dry —
        // the scripted double errors on the next call. The refresh should
        // swallow that and leave the cached digest in place.
        save(&fixture, Category::PreferenceCoding, "prefers vitest");
        writer.refresh(&fixture.alex).await.unwrap();
        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert!(
            profile.contains("Uses pnpm"),
            "a stale digest beats no profile: {profile}"
        );
    }

    #[tokio::test]
    async fn an_empty_store_gets_the_assemblers_advice_and_costs_nothing() {
        let fixture = Fixture::new();
        let (writer, model) = writer(&fixture, Some(ScriptedChatModel::new()));

        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert!(profile.contains("No memories stored yet"), "{profile}");
        assert_eq!(model.unwrap().call_count(), 0);
    }

    #[tokio::test]
    async fn a_model_with_nothing_to_say_does_not_get_re_asked() {
        // "Nothing worth an assistant's attention" is a legitimate
        // answer. Not caching it would re-ask on every session start.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");

        let (writer, model) = writer(
            &fixture,
            Some(ScriptedChatModel::new().queue(json!({"digest": ""}))),
        );
        let model = model.unwrap();

        writer.refresh(&fixture.alex).await.unwrap();
        writer.refresh(&fixture.alex).await.unwrap();
        let profile = writer.execute(&fixture.alex).await.unwrap();

        assert_eq!(model.call_count(), 1, "an empty digest was re-requested");
        assert!(profile.contains("# Memory profile: alex"), "{profile}");
    }

    #[tokio::test]
    async fn the_profile_stays_inside_its_budget_with_a_verbose_model() {
        // The budget is rent paid on every session. A model that ignores
        // the word limit must not be able to spend it.
        let fixture = Fixture::new();
        save(&fixture, Category::PreferenceCoding, "prefers pnpm");
        save(&fixture, Category::PreferencePersonal, "is vegetarian");

        let essay = "word ".repeat(5_000);
        let (writer, _) = writer(
            &fixture,
            Some(
                ScriptedChatModel::new()
                    .queue(json!({"digest": essay}))
                    .queue(json!({"digest": essay})),
            ),
        );

        writer.refresh(&fixture.alex).await.unwrap();
        let profile = writer.execute(&fixture.alex).await.unwrap();

        let budget = DEFAULT_TOKEN_BUDGET * CHARS_PER_TOKEN;
        assert!(
            profile.len() <= budget,
            "profile was {} chars, budget is {budget}",
            profile.len()
        );
    }

    #[tokio::test]
    async fn one_users_digest_is_never_served_to_another() {
        let fixture = Fixture::new();
        save(
            &fixture,
            Category::PreferenceCoding,
            "alex's private preference",
        );

        let (writer, _) = writer(
            &fixture,
            Some(
                ScriptedChatModel::new()
                    .queue(json!({"digest": "- alex's secret"}))
                    .queue(json!({"digest": "- sam's own thing"})),
            ),
        );

        writer.refresh(&fixture.alex).await.unwrap();
        let alex = writer.execute(&fixture.alex).await.unwrap();
        assert!(alex.contains("alex's secret"));

        // Sam has no memories at all, so they get the empty-profile text
        // rather than anything of alex's.
        writer.refresh(&fixture.sam).await.unwrap();
        let sam = writer.execute(&fixture.sam).await.unwrap();
        assert!(
            !sam.contains("alex's secret"),
            "another user's digest leaked: {sam}"
        );
    }

    #[tokio::test]
    async fn superseded_and_expired_memories_are_not_in_the_prompt() {
        let fixture = Fixture::new();
        let old = fixture.save(&fixture.alex, "deploys on flyio");
        let new = fixture.save(&fixture.alex, "deploys on hetzner");
        fixture
            .memories
            .update(
                &fixture.alex,
                &old.clone()
                    .supersede(new.id(), crate::memories::application::test_doubles::now()),
                "test",
            )
            .unwrap();

        let (writer, model) = writer(
            &fixture,
            Some(ScriptedChatModel::new().queue(coding_reply())),
        );
        writer.refresh(&fixture.alex).await.unwrap();

        let prompt = model.unwrap().prompt(0);
        assert!(prompt.contains("hetzner"), "{prompt}");
        assert!(
            !prompt.contains("flyio"),
            "a superseded memory reached the digest prompt: {prompt}"
        );
    }
}
