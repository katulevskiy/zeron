//! Canonical sidebar wire contracts, distinct from browser cache projections.
use serde_json::json;
use zeron_proto::{
    SidebarPinChange, SidebarPreferences, SidebarPreferencesState, SidebarSection,
    SidebarSectionChange,
};

#[test]
fn preferences_keep_camel_case_envelopes_and_snake_case_section_membership() {
    let section = SidebarSection {
        id: "section-1".into(),
        name: "Work".into(),
        session_ids: vec!["session-1".into()],
        collapsed: true,
    };
    let section_json =
        json!({"id": "section-1", "name": "Work", "session_ids": ["session-1"], "collapsed": true});
    assert_eq!(serde_json::to_value(&section).unwrap(), section_json);
    let preferences = SidebarPreferences {
        pinned_session_ids: vec!["session-2".into()],
        sections: vec![section.clone()],
    };
    assert_eq!(
        serde_json::to_value(preferences).unwrap(),
        json!({
            "pinnedSessionIds": ["session-2"], "sections": [section_json.clone()]
        })
    );
    let state = SidebarPreferencesState {
        revision: 7,
        synced: true,
        initialized: true,
        pinned_session_ids: vec!["session-2".into()],
        sections: vec![section],
    };
    assert_eq!(
        serde_json::to_value(state).unwrap(),
        json!({
            "revision": 7, "synced": true, "initialized": true,
            "pinnedSessionIds": ["session-2"], "sections": [section_json]
        })
    );
    let old: SidebarPreferencesState =
        serde_json::from_value(json!({"synced": false, "initialized": false})).unwrap();
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        json!({
            "revision": 0, "synced": false, "initialized": false,
            "pinnedSessionIds": [], "sections": []
        })
    );
    let section: SidebarSection =
        serde_json::from_value(json!({"id": "s", "name": "Empty"})).unwrap();
    assert_eq!(
        serde_json::to_value(section).unwrap(),
        json!({"id": "s", "name": "Empty", "session_ids": [], "collapsed": false})
    );
}

#[test]
fn intents_preserve_action_tags_camel_case_fields_and_nullable_anchors() {
    for (change, value) in [
        (
            SidebarPinChange::Pin {
                session_id: "s".into(),
                after: None,
                before: Some("b".into()),
            },
            json!({"action": "pin", "sessionId": "s", "after": null, "before": "b"}),
        ),
        (
            SidebarPinChange::Move {
                session_id: "s".into(),
                after: Some("a".into()),
                before: None,
            },
            json!({"action": "move", "sessionId": "s", "after": "a", "before": null}),
        ),
        (
            SidebarPinChange::Unpin {
                session_id: "s".into(),
            },
            json!({"action": "unpin", "sessionId": "s"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&change).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<SidebarPinChange>(value).unwrap(),
            change
        );
    }
    for (change, value) in [
        (
            SidebarSectionChange::Create {
                id: "s".into(),
                name: "Work".into(),
            },
            json!({"action": "create", "id": "s", "name": "Work"}),
        ),
        (
            SidebarSectionChange::Rename {
                id: "s".into(),
                name: "Renamed".into(),
            },
            json!({"action": "rename", "id": "s", "name": "Renamed"}),
        ),
        (
            SidebarSectionChange::Collapse {
                id: "s".into(),
                collapsed: true,
            },
            json!({"action": "collapse", "id": "s", "collapsed": true}),
        ),
        (
            SidebarSectionChange::Delete { id: "s".into() },
            json!({"action": "delete", "id": "s"}),
        ),
        (
            SidebarSectionChange::Assign {
                session_id: "chat".into(),
                section_id: None,
            },
            json!({"action": "assign", "sessionId": "chat", "sectionId": null}),
        ),
        (
            SidebarSectionChange::Import { sections: vec![] },
            json!({"action": "import", "sections": []}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&change).unwrap(), value);
        let pin = SidebarPinChange::Section { change };
        let nested = json!({"action": "section", "change": value});
        assert_eq!(serde_json::to_value(&pin).unwrap(), nested);
        assert_eq!(
            serde_json::from_value::<SidebarPinChange>(nested).unwrap(),
            pin
        );
    }
}
