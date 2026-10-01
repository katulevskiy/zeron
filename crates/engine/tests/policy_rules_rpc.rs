//! The device's standing permission rules over RPC: list, add, remove.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use support::*;
use zeron_proto::{ActionKind, PolicyRule, RuleEffect};
use zeron_rpc::methods;

fn rule(pattern: &str, effect: RuleEffect) -> PolicyRule {
    PolicyRule {
        kind: Some(ActionKind::Exec),
        pattern: pattern.into(),
        effect,
    }
}

#[tokio::test]
async fn rules_can_be_listed_added_in_front_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let handler: Handler = Arc::new(|_, _, _, _| {});
    let env = assemble(&dir.path().join("data"), Default::default(), handler);
    let client = zeron_rpc::memory_client(env.core.rpc_service());
    let call = |method: &'static str, params: Value| {
        let client = &client;
        async move { client.call_as::<Value>(method, params).await }
    };
    let listed = call(methods::LIST_POLICY_RULES, json!({})).await.unwrap();
    assert_eq!(listed["rules"], json!([]));

    let allow = rule("cargo *", RuleEffect::Allow);
    let deny = rule("cargo publish*", RuleEffect::Deny);
    call(methods::ADD_POLICY_RULE, json!({ "rule": allow })).await.unwrap();
    let after = call(methods::ADD_POLICY_RULE, json!({ "rule": deny })).await.unwrap();
    // The newer, narrower rule is first, so it wins.
    assert_eq!(after["rules"], json!([deny, allow]));

    let empty = call(methods::ADD_POLICY_RULE, json!({ "rule": rule("  ", RuleEffect::Allow) })).await;
    assert!(empty.is_err(), "a rule needs a pattern");

    let after = call(methods::REMOVE_POLICY_RULE, json!({ "rule": deny })).await.unwrap();
    assert_eq!(after["rules"], json!([allow]));
    // The file is the source of truth: a fresh listing agrees.
    let listed = call(methods::LIST_POLICY_RULES, json!({})).await.unwrap();
    assert_eq!(listed["rules"], json!([allow]));
}
