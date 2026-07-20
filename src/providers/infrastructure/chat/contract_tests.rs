//! One suite, run against all three chat providers.
//!
//! The clients differ in wire format and in nothing else that matters:
//! a caller gets JSON back, a rate limit is retryable, a bad key is not.
//! Testing each one separately would let those guarantees drift — the
//! bug you find in production is "OpenRouter's 429 dead-lettered the job
//! but Anthropic's didn't", which no per-file test would have caught.
//!
//! So the shared behaviours live in [`for_each_provider`], and each
//! provider only contributes the two things that are genuinely its own:
//! how to build it, and how to phrase a successful reply.
//!
//! HTTP is mocked with `wiremock` rather than recorded from the real
//! services: recordings go stale silently, and the assertions here are
//! about *our* handling of a response shape, not about whether Anthropic
//! still returns it.

use super::anthropic_chat_model::AnthropicChatModel;
use super::ollama_chat_model::OllamaChatModel;
use super::openai_compat_chat_model::OpenAiCompatChatModel;
use crate::understanding::domain::chat_model::{ChatError, ChatModel, StructuredRequest};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The JSON schema and request every case uses. Deliberately trivial:
/// these tests are about transport and error handling, not about whether
/// a model can follow a complicated schema.
fn request() -> StructuredRequest {
    StructuredRequest::new(
        "You extract memories.",
        "I prefer pnpm.",
        "candidates",
        json!({
            "type": "object",
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"],
        }),
    )
}

/// What a provider needs to contribute to the shared suite.
struct Provider {
    name: &'static str,
    /// The endpoint the client is expected to POST to.
    path: &'static str,
    /// Builds the client against a mock server, with a short timeout.
    build: fn(&str) -> Arc<dyn ChatModel>,
    /// Wraps the model's raw output in this provider's success
    /// envelope. Takes a `&str` rather than a `Value` so a case can
    /// inject output that is *not* valid JSON, which is the whole point
    /// of the repair tests.
    success: fn(&str) -> Value,
}

/// The answer every success case expects back.
const OK_JSON: &str = r#"{"ok": true}"#;

/// Output no amount of parsing can rescue — no braces, no brackets.
const PROSE: &str = "I'm not sure what you want here.";

/// Short enough that the timeout case finishes fast, long enough that a
/// loaded CI box does not trip it on the *success* cases.
const TEST_TIMEOUT: Duration = Duration::from_millis(700);

fn providers() -> Vec<Provider> {
    vec![
        Provider {
            name: "anthropic",
            path: "/v1/messages",
            build: |base| {
                Arc::new(
                    AnthropicChatModel::new("claude-haiku-4-5", "test-key", base)
                        .unwrap()
                        .with_timeout(TEST_TIMEOUT)
                        .unwrap(),
                )
            },
            success: |raw| {
                // Anthropic's tool input is a JSON value, not a string —
                // the API validated it before we ever see it. Output that
                // is not parseable therefore cannot arrive as a tool call
                // at all; it arrives as prose, which is what this models.
                match serde_json::from_str::<Value>(raw) {
                    Ok(input) => json!({
                        "stop_reason": "tool_use",
                        "content": [{"type": "tool_use", "name": "record_result", "input": input}],
                    }),
                    Err(_) => json!({
                        "stop_reason": "end_turn",
                        "content": [{"type": "text", "text": raw}],
                    }),
                }
            },
        },
        Provider {
            name: "openai-compat",
            path: "/chat/completions",
            build: |base| {
                Arc::new(
                    OpenAiCompatChatModel::new("gpt-test", "test-key", base)
                        .unwrap()
                        .with_timeout(TEST_TIMEOUT)
                        .unwrap(),
                )
            },
            success: |raw| {
                json!({
                    "choices": [{
                        "finish_reason": "stop",
                        "message": {"role": "assistant", "content": raw},
                    }]
                })
            },
        },
        Provider {
            name: "ollama",
            path: "/api/chat",
            build: |base| {
                Arc::new(
                    OllamaChatModel::new("qwen3", base)
                        .unwrap()
                        .with_timeout(TEST_TIMEOUT)
                        .unwrap(),
                )
            },
            success: |raw| {
                json!({
                    "message": {"role": "assistant", "content": raw},
                })
            },
        },
    ]
}

/// Runs `case` against each provider, naming which one failed.
///
/// A plain loop rather than a macro-generated test per provider: when
/// this fails, the message you want is "openai-compat: expected
/// Transient", and a loop gives that for free.
async fn for_each_provider<F, Fut>(case: F)
where
    F: Fn(Provider, MockServer) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for provider in providers() {
        let server = MockServer::start().await;
        case(provider, server).await;
    }
}

#[tokio::test]
async fn a_successful_call_returns_the_models_json() {
    for_each_provider(|provider, server| async move {
        Mock::given(method("POST"))
            .and(path(provider.path))
            .respond_with(ResponseTemplate::new(200).set_body_json((provider.success)(OK_JSON)))
            .mount(&server)
            .await;

        let value = (provider.build)(&server.uri())
            .complete_structured(&request())
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", provider.name));

        assert_eq!(value, json!({"ok": true}), "{}", provider.name);
    })
    .await;
}

#[tokio::test]
async fn rate_limits_and_server_errors_are_retryable() {
    // The single most consequential classification here. A 429 reported
    // as permanent dead-letters a batch of memories during a traffic
    // spike, and the user never learns their memory was dropped.
    for status in [429u16, 500, 502, 503] {
        for_each_provider(|provider, server| async move {
            Mock::given(method("POST"))
                .and(path(provider.path))
                .respond_with(ResponseTemplate::new(status).set_body_string("slow down"))
                .mount(&server)
                .await;

            let error = (provider.build)(&server.uri())
                .complete_structured(&request())
                .await
                .expect_err("should fail");

            assert!(
                matches!(error, ChatError::Transient(_)),
                "{} on HTTP {status}: expected Transient, got {error:?}",
                provider.name
            );
        })
        .await;
    }
}

#[tokio::test]
async fn credential_and_request_errors_are_permanent() {
    // The mirror image: retrying a bad API key three times per job turns
    // a typo into a slow, silent backlog instead of an immediate error.
    for status in [400u16, 401, 403, 404] {
        for_each_provider(|provider, server| async move {
            Mock::given(method("POST"))
                .and(path(provider.path))
                .respond_with(ResponseTemplate::new(status).set_body_string("nope"))
                .mount(&server)
                .await;

            let error = (provider.build)(&server.uri())
                .complete_structured(&request())
                .await
                .expect_err("should fail");

            assert!(
                matches!(error, ChatError::Permanent(_)),
                "{} on HTTP {status}: expected Permanent, got {error:?}",
                provider.name
            );
            assert!(
                error.to_string().contains(&status.to_string()),
                "{}: the status should survive into the message: {error}",
                provider.name
            );
        })
        .await;
    }
}

#[tokio::test]
async fn a_hanging_provider_times_out_and_stays_retryable() {
    for_each_provider(|provider, server| async move {
        Mock::given(method("POST"))
            .and(path(provider.path))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json((provider.success)(OK_JSON))
                    // Comfortably past TEST_TIMEOUT.
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;

        let error = (provider.build)(&server.uri())
            .complete_structured(&request())
            .await
            .expect_err("a hung request must not block an ingestion worker forever");

        assert!(
            matches!(error, ChatError::Transient(_)),
            "{}: a timeout says nothing about the request's validity, so it must \
             stay retryable — got {error:?}",
            provider.name
        );
    })
    .await;
}

#[tokio::test]
async fn an_unreachable_provider_is_retryable() {
    for provider in providers() {
        // A port nothing is listening on: connection refused, which is
        // what a restarting Ollama or a blipped network looks like.
        let error = (provider.build)("http://127.0.0.1:1")
            .complete_structured(&request())
            .await
            .expect_err("should fail");

        assert!(
            matches!(error, ChatError::Transient(_)),
            "{}: expected Transient, got {error:?}",
            provider.name
        );
    }
}

#[tokio::test]
async fn each_provider_authenticates_the_way_its_api_expects() {
    // Anthropic uses x-api-key, OpenAI-compatible servers use a bearer
    // token, and Ollama uses neither. Getting this wrong produces a 401
    // that looks like a bad key rather than a bad client.
    let cases: Vec<(&str, &str, Box<dyn Fn(&MockServer) -> Mock>)> = vec![
        (
            "anthropic",
            "/v1/messages",
            Box::new(|_| {
                Mock::given(method("POST"))
                    .and(path("/v1/messages"))
                    .and(header("x-api-key", "test-key"))
                    .and(header("anthropic-version", "2023-06-01"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                        "stop_reason": "tool_use",
                        "content": [{"type": "tool_use", "input": {"ok": true}}],
                    })))
            }),
        ),
        (
            "openai-compat",
            "/chat/completions",
            Box::new(|_| {
                Mock::given(method("POST"))
                    .and(path("/chat/completions"))
                    .and(header("authorization", "Bearer test-key"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                        "choices": [{"message": {"content": "{\"ok\": true}"}}]
                    })))
            }),
        ),
    ];

    for (name, _path, build_mock) in cases {
        let server = MockServer::start().await;
        build_mock(&server).mount(&server).await;

        let provider = providers()
            .into_iter()
            .find(|p| p.name == name)
            .expect("known provider");

        (provider.build)(&server.uri())
            .complete_structured(&request())
            .await
            .unwrap_or_else(|e| panic!("{name} did not send the expected credentials: {e}"));
    }
}

#[tokio::test]
async fn text_providers_recover_from_one_malformed_reply() {
    // Only the text-based providers can reach this state. Anthropic's
    // forced tool use returns an already-validated JSON value, so there
    // is no string for it to get wrong — its equivalent failure is
    // "no tool_use block", covered separately below.
    for provider in providers().into_iter().filter(|p| p.name != "anthropic") {
        let server = MockServer::start().await;

        // First call: prose instead of JSON. Second: the real answer.
        Mock::given(method("POST"))
            .and(path(provider.path))
            .respond_with(ResponseTemplate::new(200).set_body_json((provider.success)(PROSE)))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(provider.path))
            .respond_with(ResponseTemplate::new(200).set_body_json((provider.success)(OK_JSON)))
            .mount(&server)
            .await;

        let value = (provider.build)(&server.uri())
            .complete_structured(&request())
            .await
            .unwrap_or_else(|e| panic!("{}: repair should have rescued this: {e}", provider.name));

        assert_eq!(value, json!({"ok": true}), "{}", provider.name);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "{}: exactly one repair attempt",
            provider.name
        );
    }
}

#[tokio::test]
async fn text_providers_give_up_after_the_second_malformed_reply() {
    for provider in providers().into_iter().filter(|p| p.name != "anthropic") {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(provider.path))
            .respond_with(ResponseTemplate::new(200).set_body_json((provider.success)(PROSE)))
            .mount(&server)
            .await;

        let error = (provider.build)(&server.uri())
            .complete_structured(&request())
            .await
            .expect_err("should fail");

        assert!(
            matches!(error, ChatError::Malformed(_)),
            "{}: got {error:?}",
            provider.name
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "{}: a third call would spend money to learn nothing",
            provider.name
        );
    }
}

#[tokio::test]
async fn anthropic_reports_a_missing_tool_call_rather_than_guessing() {
    // Anthropic's analogue of a malformed reply: HTTP 200, but the model
    // answered in prose instead of calling the tool it was forced to.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "I don't think there's anything to record."}],
        })))
        .mount(&server)
        .await;

    let provider = providers()
        .into_iter()
        .find(|p| p.name == "anthropic")
        .unwrap();
    let error = (provider.build)(&server.uri())
        .complete_structured(&request())
        .await
        .expect_err("should fail");

    assert!(matches!(error, ChatError::Malformed(_)), "got {error:?}");
    assert!(error.to_string().contains("tool_use"), "{error}");
}

#[tokio::test]
async fn the_schema_and_the_material_both_reach_the_provider() {
    // A dropped schema still yields plausible-looking JSON, so nothing
    // downstream would notice — which is exactly why it is asserted here
    // on the wire rather than trusted.
    for_each_provider(|provider, server| async move {
        Mock::given(method("POST"))
            .and(path(provider.path))
            .respond_with(ResponseTemplate::new(200).set_body_json((provider.success)(OK_JSON)))
            .mount(&server)
            .await;

        (provider.build)(&server.uri())
            .complete_structured(&request())
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8(requests[0].body.clone()).unwrap();

        assert!(
            body.contains("I prefer pnpm."),
            "{}: the material never reached the model: {body}",
            provider.name
        );
        assert!(
            body.contains("You extract memories."),
            "{}: the instructions never reached the model: {body}",
            provider.name
        );
        assert!(
            body.contains("\"ok\""),
            "{}: the schema never reached the model: {body}",
            provider.name
        );
    })
    .await;
}
