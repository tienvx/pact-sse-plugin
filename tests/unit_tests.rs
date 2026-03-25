use pact_sse_plugin::sse_content::{SseEvent, parse_sse_content, format_sse_content, compare_sse_events};
use std::collections::HashMap;

#[test]
fn test_parse_single_event() {
    let input = "data: hello world\n\n";
    let events = SseEvent::parse(input);
    
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "hello world");
    assert_eq!(events[0].event, None);
    assert_eq!(events[0].id, None);
}

#[test]
fn test_parse_event_with_all_fields() {
    let input = "id: 123\nevent: message\nretry: 3000\ndata: hello\n\n";
    let events = SseEvent::parse(input);
    
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, Some("123".to_string()));
    assert_eq!(events[0].event, Some("message".to_string()));
    assert_eq!(events[0].retry, Some(3000));
    assert_eq!(events[0].data, "hello");
}

#[test]
fn test_parse_multiple_events() {
    let input = "data: event1\n\ndata: event2\n\ndata: event3\n\n";
    let events = SseEvent::parse(input);
    
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].data, "event1");
    assert_eq!(events[1].data, "event2");
    assert_eq!(events[2].data, "event3");
}

#[test]
fn test_parse_multiline_data() {
    let input = "data: line1\ndata: line2\ndata: line3\n\n";
    let events = SseEvent::parse(input);
    
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "line1\nline2\nline3");
}

#[test]
fn test_parse_empty_data() {
    let input = "\n\n";
    let events = SseEvent::parse(input);
    
    assert_eq!(events.len(), 0);
}

#[test]
fn test_format_single_event() {
    let event = SseEvent {
        event: Some("message".to_string()),
        id: Some("123".to_string()),
        data: "hello".to_string(),
        retry: Some(3000),
    };
    
    let formatted = event.format();
    assert!(formatted.contains("id: 123"));
    assert!(formatted.contains("event: message"));
    assert!(formatted.contains("retry: 3000"));
    assert!(formatted.contains("data: hello"));
}

#[test]
fn test_parse_sse_content() {
    let content = b"data: test\n\n";
    let events = parse_sse_content(content).unwrap();
    
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "test");
}

#[test]
fn test_parse_invalid_utf8() {
    let content = b"\xff\xfe";
    let result = parse_sse_content(content);
    
    assert!(result.is_err());
}

#[test]
fn test_format_sse_content() {
    let events = vec![
        SseEvent {
            event: Some("message".to_string()),
            id: None,
            data: "hello".to_string(),
            retry: None,
        }
    ];
    
    let formatted = format_sse_content(&events);
    let formatted_str = String::from_utf8_lossy(&formatted);
    
    assert!(formatted_str.contains("event: message"));
    assert!(formatted_str.contains("data: hello"));
}

#[test]
fn test_compare_sse_events_different_count() {
    let expected = vec![
        SseEvent {
            event: None,
            id: None,
            data: "event1".to_string(),
            retry: None,
        }
    ];
    let actual = vec![
        SseEvent {
            event: None,
            id: None,
            data: "event1".to_string(),
            retry: None,
        },
        SseEvent {
            event: None,
            id: None,
            data: "event2".to_string(),
            retry: None,
        }
    ];
    
    let mismatches = compare_sse_events(&expected, &actual, &HashMap::new());
    
    assert!(!mismatches.is_empty());
    assert!(mismatches[0].mismatch.contains("events"));
}

#[test]
fn test_compare_sse_events_matching() {
    let expected = vec![
        SseEvent {
            event: Some("message".to_string()),
            id: Some("123".to_string()),
            data: "hello".to_string(),
            retry: Some(3000),
        }
    ];
    let actual = expected.clone();
    
    let mismatches = compare_sse_events(&expected, &actual, &HashMap::new());
    
    assert!(mismatches.is_empty());
}

#[test]
fn test_compare_sse_events_data_mismatch() {
    let expected = vec![
        SseEvent {
            event: None,
            id: None,
            data: "expected".to_string(),
            retry: None,
        }
    ];
    let actual = vec![
        SseEvent {
            event: None,
            id: None,
            data: "actual".to_string(),
            retry: None,
        }
    ];
    
    let mismatches = compare_sse_events(&expected, &actual, &HashMap::new());
    
    assert!(!mismatches.is_empty());
    assert!(mismatches[0].mismatch.contains("mismatch"));
}
