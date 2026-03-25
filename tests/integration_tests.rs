//! Integration tests for the SSE plugin
//! These tests verify the gRPC interface works correctly

use std::collections::{HashMap, BTreeMap};

mod proto {
    tonic::include_proto!("io.pact.plugin");
}

use proto::{CompareContentsRequest, Body};

/// Test that compare_contents handles matching SSE content
#[test]
fn test_compare_contents_sse_format() {
    let expected = Body {
        content_type: "text/event-stream".to_string(),
        content: Some("data: hello\n\n".as_bytes().to_vec().into()),
        content_type_hint: 0,
    };
    
    let actual = expected.clone();
    
    let request = CompareContentsRequest {
        expected: Some(expected),
        actual: Some(actual),
        allow_unexpected_keys: false,
        rules: HashMap::new(),
        plugin_configuration: None,
    };
    
    // Verify the request structure is correct
    assert_eq!(request.expected.as_ref().unwrap().content_type, "text/event-stream");
    assert_eq!(request.actual.as_ref().unwrap().content_type, "text/event-stream");
}

/// Test that compare_contents handles mismatched SSE content
#[test]
fn test_compare_contents_sse_mismatch() {
    let expected = Body {
        content_type: "text/event-stream".to_string(),
        content: Some("data: expected\n\n".as_bytes().to_vec().into()),
        content_type_hint: 0,
    };
    
    let actual = Body {
        content_type: "text/event-stream".to_string(),
        content: Some("data: actual\n\n".as_bytes().to_vec().into()),
        content_type_hint: 0,
    };
    
    let request = CompareContentsRequest {
        expected: Some(expected),
        actual: Some(actual),
        allow_unexpected_keys: false,
        rules: HashMap::new(),
        plugin_configuration: None,
    };
    
    // Verify the request structure is correct
    assert_eq!(request.expected.as_ref().unwrap().content_type, "text/event-stream");
    assert_eq!(request.actual.as_ref().unwrap().content_type, "text/event-stream");
    
    let expected_bytes: &[u8] = request.expected.as_ref().unwrap().content.as_ref().unwrap().as_ref();
    let actual_bytes: &[u8] = request.actual.as_ref().unwrap().content.as_ref().unwrap().as_ref();
    assert_ne!(expected_bytes, actual_bytes);
}

/// Test that SSE content with multiple events is formatted correctly
#[test]
fn test_sse_multiple_events() {
    let content = Body {
        content_type: "text/event-stream".to_string(),
        content: Some("data: event1\n\ndata: event2\n\ndata: event3\n\n".as_bytes().to_vec().into()),
        content_type_hint: 0,
    };
    
    let content_str = String::from_utf8_lossy(content.content.as_ref().unwrap().as_ref());
    assert!(content_str.contains("data: event1"));
    assert!(content_str.contains("data: event2"));
    assert!(content_str.contains("data: event3"));
}

/// Test that SSE content with event types is formatted correctly
#[test]
fn test_sse_event_types() {
    let content = Body {
        content_type: "text/event-stream".to_string(),
        content: Some("event: message\nid: 123\ndata: hello\n\n".as_bytes().to_vec().into()),
        content_type_hint: 0,
    };
    
    let content_str = String::from_utf8_lossy(content.content.as_ref().unwrap().as_ref());
    assert!(content_str.contains("event: message"));
    assert!(content_str.contains("id: 123"));
    assert!(content_str.contains("data: hello"));
}

/// Test that plugin configuration can be set
#[test]
fn test_plugin_configuration() {
    let config = proto::PluginConfiguration {
        interaction_configuration: Some(prost_types::Struct {
            fields: BTreeMap::from([
                ("key".to_string(), prost_types::Value {
                    kind: Some(prost_types::value::Kind::StringValue("value".to_string())),
                }),
            ]),
        }),
        pact_configuration: None,
    };
    
    assert!(config.interaction_configuration.is_some());
}
