use serde_json::{Value, json};
use tri_ai_runtime::{Error, Limits, ResponseState, Status, Update};
fn event(state: &mut ResponseState, value: Value) -> Result<Update, Error> {
    state.apply_json(&serde_json::to_vec(&value).unwrap())
}
fn created(sequence: u64) -> Value {
    json!({"type":"response.created","sequence_number":sequence,"response":{"id":"resp_fixture","status":"in_progress"}})
}
fn delta(sequence: u64, text: &str) -> Value {
    json!({"type":"response.output_text.delta","sequence_number":sequence,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":text})
}
fn terminal(sequence: u64, status: &str, text: &str) -> Value {
    json!({"type":format!("response.{status}"),"sequence_number":sequence,"response":{"id":"resp_fixture","status":status,"output":[{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}})
}

#[test]
fn text_deltas_and_terminal_snapshot_form_one_complete_reply() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state, delta(1, "你好")).unwrap();
    event(&mut state, delta(2, " world")).unwrap();
    event(&mut state, terminal(3, "completed", "你好 world")).unwrap();
    let reply = state.finish().unwrap();
    assert_eq!(reply.status, Status::Completed);
    assert_eq!(reply.text(), "你好 world");
    assert_eq!(reply.response_id.as_deref(), Some("resp_fixture"));
    assert!(!reply.refused);
}
#[test]
fn keepalive_does_not_move_the_stored_event_cursor() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(
        &mut state,
        json!({"type":"keepalive","sequence_number":999}),
    )
    .unwrap();
    assert_eq!(state.sequence(), Some(0));
    event(&mut state, delta(1, "next")).unwrap();
    assert_eq!(state.sequence(), Some(1));
}
#[test]
fn duplicate_events_are_idempotent_but_conflicting_or_older_events_are_rejected() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    let mut value = delta(1, "once");
    value["obfuscation"] = json!("random-one");
    event(&mut state, value.clone()).unwrap();
    value["obfuscation"] = json!("random-two");
    assert!(matches!(event(&mut state, value), Ok(Update::Ignored)));
    assert!(matches!(
        event(&mut state, delta(1, "different")),
        Err(Error::Sequence)
    ));
    assert!(matches!(
        event(&mut state, delta(0, "old")),
        Err(Error::Sequence)
    ));
    event(&mut state, terminal(2, "completed", "once")).unwrap();
    assert_eq!(state.finish().unwrap().text(), "once");
}
#[test]
fn changing_identity_or_conflicting_terminal_text_is_not_success() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state, delta(1, "original")).unwrap();
    let mut wrong = terminal(2, "completed", "original");
    wrong["response"]["id"] = json!("resp_other");
    assert!(event(&mut state, wrong).is_err());
    assert!(event(&mut state, terminal(2, "completed", "changed")).is_err());
    assert_eq!(state.sequence(), Some(1));
}
#[test]
fn refusal_incomplete_and_provider_failure_are_distinct_from_complete_text() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state,json!({"type":"response.refusal.delta","sequence_number":1,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"Cannot help"})).unwrap();
    event(&mut state,json!({"type":"response.completed","sequence_number":2,"response":{"id":"resp_fixture","status":"completed","output":[{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"refusal","refusal":"Cannot help"}]}]}})).unwrap();
    let reply = state.finish().unwrap();
    assert!(reply.refused);
    assert_eq!(reply.text(), "Cannot help");
    for status in ["incomplete", "failed", "cancelled"] {
        let mut state = ResponseState::new(Limits::default()).unwrap();
        event(&mut state, created(0)).unwrap();
        event(&mut state, delta(1, "partial")).unwrap();
        event(&mut state, terminal(2, status, "partial")).unwrap();
        assert_ne!(state.finish().unwrap().status, Status::Completed);
    }
}
#[test]
fn tool_outputs_are_never_interpreted_as_text_or_executed() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    let mut value = terminal(1, "completed", "");
    value["response"]["output"] =
        json!([{"type":"function_call","name":"parameters.delete","arguments":"{}"}]);
    assert!(matches!(
        event(&mut state, value),
        Err(Error::UnexpectedOutput)
    ));
}
#[test]
fn encrypted_host_checkpoints_can_resume_without_repeating_text() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state, delta(1, "private-marker")).unwrap();
    assert!(!format!("{state:?}").contains("private-marker"));
    let checkpoint = state.checkpoint().unwrap();
    let mut restored = ResponseState::restore(&checkpoint, Limits::default()).unwrap();
    assert_eq!(restored.sequence(), Some(1));
    event(&mut restored, delta(2, " tail")).unwrap();
    event(
        &mut restored,
        terminal(3, "completed", "private-marker tail"),
    )
    .unwrap();
    assert_eq!(restored.finish().unwrap().text(), "private-marker tail");
}
#[test]
fn limits_and_truncation_fail_without_advancing_accepted_cursor() {
    let mut state = ResponseState::new(Limits {
        max_text_bytes: 4,
        ..Limits::default()
    })
    .unwrap();
    event(&mut state, created(0)).unwrap();
    assert!(matches!(
        event(&mut state, delta(1, "hello")),
        Err(Error::Limit)
    ));
    assert_eq!(state.sequence(), Some(0));
    assert!(matches!(state.finish(), Err(Error::Truncated)));
    assert!(
        ResponseState::new(Limits {
            max_event_bytes: 0,
            ..Limits::default()
        })
        .is_err()
    );
}

#[test]
fn synchronous_snapshots_do_not_fabricate_a_stream_cursor() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    let response = json!({"id":"resp_fixture","object":"response","status":"completed","output":[{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":"snapshot"}]}]});
    assert_eq!(
        state
            .apply_snapshot(&serde_json::to_vec(&response).unwrap())
            .unwrap(),
        Some(Status::Completed)
    );
    assert_eq!(state.sequence(), None);
    let restored = ResponseState::restore(&state.checkpoint().unwrap(), Limits::default()).unwrap();
    assert_eq!(restored.finish().unwrap().text(), "snapshot");
}
#[test]
fn retrieval_snapshots_preserve_the_last_stored_event_cursor() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state, delta(1, "head")).unwrap();
    state.apply_snapshot(&serde_json::to_vec(&json!({"id":"resp_fixture","object":"response","status":"in_progress","output":[]})).unwrap()).unwrap();
    assert_eq!(state.sequence(), Some(1));
    let mut complete = terminal(2, "completed", "head tail")["response"].clone();
    complete["object"] = json!("response");
    state
        .apply_snapshot(&serde_json::to_vec(&complete).unwrap())
        .unwrap();
    assert_eq!(state.sequence(), Some(1));
    assert_eq!(state.finish().unwrap().text(), "head tail");
}
#[test]
fn done_events_cannot_change_output_kind_and_unknown_progress_cannot_change_identity() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state,json!({"type":"response.refusal.delta","sequence_number":1,"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":"no"})).unwrap();
    assert!(event(&mut state,json!({"type":"response.output_text.done","sequence_number":2,"item_id":"msg_fixture","output_index":0,"content_index":0,"text":"no"})).is_err());
    assert!(event(&mut state,json!({"type":"response.some_future_progress","sequence_number":2,"response":{"id":"resp_other"}})).is_err());
}
#[test]
fn unsupported_output_items_are_rejected_before_a_terminal_event() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    assert!(matches!(
        event(
            &mut state,
            json!({"type":"response.output_item.added","sequence_number":1,"item":{"type":"function_call","name":"untrusted"}})
        ),
        Err(Error::UnexpectedOutput)
    ));
}

#[test]
fn repeated_failed_snapshots_retain_private_partial_output() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    event(&mut state, delta(1, "partial")).unwrap();
    event(
        &mut state,
        json!({"type":"error","sequence_number":2,"message":"PRIVATE-ERROR-BODY"}),
    )
    .unwrap();
    let snapshot = serde_json::to_vec(
        &json!({"id":"resp_fixture","object":"response","status":"failed","output":[]}),
    )
    .unwrap();
    assert_eq!(
        state.apply_snapshot(&snapshot).unwrap(),
        Some(Status::Failed)
    );
    assert_eq!(state.sequence(), Some(2));
    assert_eq!(state.finish().unwrap().text(), "partial");
}
#[test]
fn interleaved_output_parts_finish_in_output_order_without_exposing_reasoning() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    let mut later = delta(1, "second");
    later["output_index"] = json!(2);
    later["item_id"] = json!("msg_second");
    event(&mut state, later).unwrap();
    event(&mut state, delta(2, "first")).unwrap();
    event(&mut state,json!({"type":"response.completed","sequence_number":3,"response":{"id":"resp_fixture","status":"completed","output":[{"id":"msg_fixture","type":"message","role":"assistant","content":[{"type":"output_text","text":"first"}]},{"type":"reasoning","summary":[{"text":"private reasoning"}]},{"id":"msg_second","type":"message","role":"assistant","content":[{"type":"output_text","text":"second"}]}]}})).unwrap();
    let reply = state.finish().unwrap();
    assert_eq!(reply.text(), "first\nsecond");
    assert_eq!(reply.parts.len(), 2);
}
#[test]
fn a_terminal_snapshot_can_contain_text_larger_than_an_individual_delta_event() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    let part = "x".repeat(48 * 1024);
    event(&mut state, delta(1, &part)).unwrap();
    event(&mut state, delta(2, &part)).unwrap();
    let complete = part.repeat(2);
    event(&mut state, terminal(3, "completed", &complete)).unwrap();
    assert_eq!(state.finish().unwrap().text(), complete);
}
#[test]
fn ordinary_events_still_reject_oversized_payloads_without_moving_the_cursor() {
    let mut state = ResponseState::new(Limits::default()).unwrap();
    event(&mut state, created(0)).unwrap();
    assert!(matches!(
        event(&mut state, delta(1, &"x".repeat(70 * 1024))),
        Err(Error::Limit)
    ));
    assert_eq!(state.sequence(), Some(0));
}
#[test]
fn repeated_json_retrievals_have_independent_response_byte_budgets() {
    let mut state = ResponseState::new(Limits {
        max_stream_bytes: 512,
        ..Limits::default()
    })
    .unwrap();
    let pending = serde_json::to_vec(
        &json!({"id":"resp_fixture","object":"response","status":"in_progress","output":[]}),
    )
    .unwrap();
    for _ in 0..12 {
        assert_eq!(state.apply_snapshot(&pending).unwrap(), None);
    }
    let mut complete = terminal(1, "completed", "complete")["response"].clone();
    complete["object"] = json!("response");
    state
        .apply_snapshot(&serde_json::to_vec(&complete).unwrap())
        .unwrap();
    assert_eq!(state.finish().unwrap().text(), "complete");
}
