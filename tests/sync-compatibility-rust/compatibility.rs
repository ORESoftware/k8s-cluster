use daedalus_client::Plan;
use daedalus_interfaces::{DaedalusClientLogEntry, DAEDALUS_CLIENT_LOG_ENTRIES_TABLE};
use daedalus_sync::MemorySyncStore;

#[tokio::test]
async fn actual_client_and_interface_crates_share_the_sync_document_boundary() {
    let store = MemorySyncStore::new("operator-1", "native-1").unwrap();
    let plan = Plan {
        id: "plan-1".into(),
        title: "fixture".into(),
        goal: "machine the integration fixture".into(),
        process_family: "subtractive".into(),
        status: "draft".into(),
        created_at: "2026-07-22T12:00:00Z".into(),
        updated_at: "2026-07-22T12:00:00Z".into(),
    };
    let log_entry = DaedalusClientLogEntry {
        id: "entry-1".into(),
        dd_user_id: None,
        session_id: Some("session-1".into()),
        trace_id: None,
        commit_id: None,
        environment: "test".into(),
        level: "info".into(),
        message: "plan cached".into(),
        stack: None,
        url: None,
        source: "client".into(),
        category: Some("sync.compatibility".into()),
        metadata: serde_json::json!({ "plan_id": plan.id }),
        client_timestamp: "2026-07-22T12:00:01Z".into(),
        created_at: "2026-07-22T12:00:01Z".into(),
        is_soft_deleted: false,
    };

    let plan_change = store
        .put_document(Plan::SYNC_COLLECTION, &plan.id, &plan, 1000)
        .await
        .unwrap();
    let log_change = store
        .put_document(
            DAEDALUS_CLIENT_LOG_ENTRIES_TABLE,
            &log_entry.id,
            &log_entry,
            1001,
        )
        .await
        .unwrap();

    assert_eq!(plan_change.collection, "plans");
    assert_eq!(
        plan_change.payload.unwrap()["process_family"],
        "subtractive"
    );
    assert_eq!(log_change.payload.unwrap()["metadata"]["plan_id"], "plan-1");
}
