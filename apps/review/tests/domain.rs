use serde_json::{Value, json};
use std::path::Path;
use zeron_review::{
    Result,
    config::Policy,
    domain::{self, assess, mutate},
    seed,
    store::Db,
};
fn fixture() -> (Db, Policy) {
    let mut db = Db::open(Path::new(":memory:")).unwrap();
    db.transaction(|db| domain::ingest(db,&json!({"id":"pr:1","number":1,"kind":"pr","title":"Fix","author":"author","direction":"accepted","risk":"low","revision":"a".repeat(40),"baseRevision":"b".repeat(40),"triaged":true,"platforms":["windows"],"createdAt":"2026-10-01T00:00:00Z","checks":[{"name":"core-tests","conclusion":"success","revision":"a".repeat(40),"baseRevision":"b".repeat(40)}]}))).unwrap();
    (db, seed::policy())
}
fn act(db: &mut Db, policy: &Policy, actor: &str, action: &str, mut body: Value) -> Result<Value> {
    if body["revision"].is_null() {
        body["revision"] = db.get("pr:1")?.unwrap()["revision"].clone();
    }
    db.transaction(|db| mutate(db, "pr:1", actor, action, &body, policy))
}
fn approve(db: &mut Db, policy: &Policy) {
    act(
        db,
        policy,
        "demo-reviewer",
        "review",
        json!({"verdict":"pass","summary":"Reviewed regressions"}),
    )
    .unwrap();
    act(db,policy,"demo-validator","validate",json!({"platform":"windows","verdict":"pass","environment":"Windows 11","summary":"Reproduced and retested"})).unwrap();
}
#[test]
fn readiness_requires_independent_evidence_not_votes() {
    let (mut db, policy) = fixture();
    act(
        &mut db,
        &policy,
        "visitor",
        "vote",
        json!({"dimension":"demand","reason":"Needed daily"}),
    )
    .unwrap();
    assert_eq!(
        assess(&db.get("pr:1").unwrap().unwrap(), &policy).state,
        "ready-for-review"
    );
    let reviewed = act(
        &mut db,
        &policy,
        "demo-reviewer",
        "review",
        json!({"verdict":"pass","summary":"Reviewed regressions"}),
    )
    .unwrap();
    assert_eq!(reviewed["state"], "needs-validation");
    let ready=act(&mut db,&policy,"demo-validator","validate",json!({"platform":"windows","verdict":"pass","environment":"Windows 11","summary":"Reproduced and retested"})).unwrap();
    assert_eq!(ready["state"], "ready-to-merge");
}
#[test]
fn authors_and_untrusted_actors_cannot_attest() {
    let (mut db, mut policy) = fixture();
    policy.reviewers.push("author".into());
    assert_eq!(
        act(
            &mut db,
            &policy,
            "author",
            "review",
            json!({"verdict":"pass","summary":"Self approval"})
        )
        .unwrap_err()
        .status,
        403
    );
    assert_eq!(
        act(
            &mut db,
            &policy,
            "visitor",
            "review",
            json!({"verdict":"pass","summary":"Approval"})
        )
        .unwrap_err()
        .status,
        403
    );
}
#[test]
fn revisions_preserve_history_but_invalidate_evidence_and_claims() {
    let (mut db, policy) = fixture();
    approve(&mut db, &policy);
    act(
        &mut db,
        &policy,
        "demo-reviewer",
        "claim",
        json!({"task":"code-review"}),
    )
    .unwrap();
    let changed = db
        .transaction(|db| domain::ingest(db, &json!({"id":"pr:1","revision":"c".repeat(40)})))
        .unwrap();
    assert_ne!(assess(&changed, &policy).state, "ready-to-merge");
    assert_eq!(changed["reviews"].as_array().unwrap().len(), 1);
    assert!(db.claims("pr:1").unwrap().is_empty());
    assert_eq!(
        act(
            &mut db,
            &policy,
            "demo-reviewer",
            "review",
            json!({"revision":"a".repeat(40),"verdict":"pass","summary":"Old review"})
        )
        .unwrap_err()
        .status,
        409
    );
}
#[test]
fn changes_and_failed_tests_remain_blockers_until_resolved() {
    let (mut db, policy) = fixture();
    approve(&mut db, &policy);
    let changed = act(
        &mut db,
        &policy,
        "demo-reviewer",
        "review",
        json!({"verdict":"changes","summary":"Regression"}),
    )
    .unwrap();
    assert_eq!(changed["state"], "changes-requested");
    act(&mut db,&policy,"demo-validator","validate",json!({"platform":"windows","verdict":"fail","environment":"Windows 10","summary":"Still broken"})).unwrap();
    let reviewed = act(
        &mut db,
        &policy,
        "demo-reviewer",
        "review",
        json!({"verdict":"pass","summary":"Review concern resolved"}),
    )
    .unwrap();
    assert!(
        reviewed["blockers"]
            .as_array()
            .unwrap()
            .contains(&json!("windows validation failed"))
    );
    let ready=act(&mut db,&policy,"demo-validator","validate",json!({"platform":"windows","verdict":"pass","environment":"Windows 10","summary":"Resolved on retest"})).unwrap();
    assert_eq!(ready["state"], "ready-to-merge");
}
#[test]
fn claims_conflict_expire_and_rollback() {
    let (mut db, policy) = fixture();
    act(
        &mut db,
        &policy,
        "first",
        "claim",
        json!({"task":"code-review"}),
    )
    .unwrap();
    let count = db.events("pr:1").unwrap().len();
    assert_eq!(
        act(
            &mut db,
            &policy,
            "second",
            "claim",
            json!({"task":"code-review"})
        )
        .unwrap_err()
        .status,
        409
    );
    assert_eq!(
        act(
            &mut db,
            &policy,
            "second",
            "renew",
            json!({"task":"code-review"})
        )
        .unwrap_err()
        .status,
        409
    );
    assert_eq!(db.events("pr:1").unwrap().len(), count);
    db.claim(
        "pr:1",
        "code-review",
        "first",
        &"a".repeat(40),
        "2020-01-01T00:00:00Z",
    )
    .unwrap();
    let claimed = act(
        &mut db,
        &policy,
        "second",
        "claim",
        json!({"task":"code-review"}),
    )
    .unwrap();
    assert_eq!(claimed["claims"][0]["actor"], "second");
}
#[test]
fn missing_skipped_failed_or_old_base_checks_block_readiness() {
    let (mut db, policy) = fixture();
    approve(&mut db, &policy);
    let item = db.get("pr:1").unwrap().unwrap();
    for conclusion in ["skipped", "failure", "neutral", "pending"] {
        let mut changed = item.clone();
        changed["checks"][0]["conclusion"] = conclusion.into();
        assert_ne!(assess(&changed, &policy).state, "ready-to-merge");
    }
    let mut changed = item.clone();
    changed["checks"] = json!([]);
    assert_ne!(assess(&changed, &policy).state, "ready-to-merge");
    let mut changed = item;
    changed["baseRevision"] = "c".repeat(40).into();
    assert_ne!(assess(&changed, &policy).state, "ready-to-merge");
}
#[test]
fn revocations_and_high_risk_are_enforced() {
    let (mut db, mut policy) = fixture();
    approve(&mut db, &policy);
    policy.reviewers.clear();
    assert_ne!(
        assess(&db.get("pr:1").unwrap().unwrap(), &policy).state,
        "ready-to-merge"
    );
    policy = seed::policy();
    let mut changed = db.get("pr:1").unwrap().unwrap();
    changed["risk"] = "high".into();
    db.put(&changed).unwrap();
    assert_ne!(assess(&changed, &policy).state, "ready-to-merge");
    let approved = act(
        &mut db,
        &policy,
        "demo-maintainer",
        "risk-approval",
        json!({"reason":"Rollback coverage checked"}),
    )
    .unwrap();
    assert_eq!(approved["state"], "ready-to-merge");
}
#[test]
fn only_maintainers_can_weaken_triaged_requirements() {
    let (mut db, policy) = fixture();
    act(&mut db,&policy,"demo-triager","triage",json!({"risk":"medium","platforms":["windows","linux"],"priority":"normal","area":"desktop"})).unwrap();
    assert_eq!(
        act(
            &mut db,
            &policy,
            "demo-triager",
            "triage",
            json!({"risk":"low","platforms":["windows"],"priority":"normal","area":"desktop"})
        )
        .unwrap_err()
        .status,
        403
    );
}
#[test]
fn issues_have_roadmap_placement_and_closed_items_are_immutable() {
    let (mut db, policy) = fixture();
    db.transaction(|db| domain::ingest(db,&json!({"id":"issue:2","kind":"issue","number":2,"author":"author","revision":"updated","createdAt":"2026-10-01T00:00:00Z"}))).unwrap();
    let result=db.transaction(|db| mutate(db,"issue:2","demo-triager","triage",&json!({"revision":"updated","risk":"low","area":"desktop","platforms":[],"priority":"normal","plan":"next"}),&policy)).unwrap();
    assert_eq!(result["state"], "next");
    let mut closed = result;
    closed["closed"] = true.into();
    db.put(&closed).unwrap();
    assert_eq!(
        db.transaction(|db| mutate(
            db,
            "issue:2",
            "visitor",
            "claim",
            &json!({"revision":"updated","task":"reproduce"}),
            &policy
        ))
        .unwrap_err()
        .status,
        409
    );
}
#[test]
fn transaction_failures_leave_no_partial_mutations() {
    let (mut db, policy) = fixture();
    let before = db.events("pr:1").unwrap().len();
    let result = db.transaction(|db| {
        mutate(
            db,
            "pr:1",
            "visitor",
            "vote",
            &json!({"revision":"a".repeat(40),"dimension":"demand","reason":"Need this"}),
            &policy,
        )?;
        db.connection
            .execute("INSERT INTO nonexistent VALUES (1)", [])?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(db.get("pr:1").unwrap().unwrap()["votes"], json!({}));
    assert_eq!(db.events("pr:1").unwrap().len(), before);
}
#[test]
fn native_refresh_keeps_local_reviews_and_unchanged_data_is_not_requeued() {
    let (mut db, policy) = fixture();
    act(
        &mut db,
        &policy,
        "demo-reviewer",
        "review",
        json!({"verdict":"pass","summary":"Concurrent local review"}),
    )
    .unwrap();
    db.sent("pr:1").unwrap();
    let source = json!({"id":"pr:1","nativeReviews":[],"syncedAt":zeron_review::now()});
    let refreshed = db.transaction(|db| domain::ingest(db, &source)).unwrap();
    assert_eq!(
        refreshed["reviews"][0]["summary"],
        "Concurrent local review"
    );
    assert!(db.pending().unwrap().is_empty());
}
