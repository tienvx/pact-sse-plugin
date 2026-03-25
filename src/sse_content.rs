use pact_matching::matchers::Matches;
use pact_models::matchingrules::RuleList;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
    pub retry: Option<u64>,
}

impl SseEvent {
    pub fn parse(data: &str) -> Vec<Self> {
        let mut events = Vec::new();
        let mut current_event = SseEvent {
            event: None,
            id: None,
            data: String::new(),
            retry: None,
        };

        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                if !current_event.data.is_empty() {
                    events.push(current_event);
                    current_event = SseEvent {
                        event: None,
                        id: None,
                        data: String::new(),
                        retry: None,
                    };
                }
                continue;
            }

            if line.starts_with("data:") {
                let data_value = line.strip_prefix("data:").unwrap_or(line).trim();
                if !current_event.data.is_empty() {
                    current_event.data.push('\n');
                }
                current_event.data.push_str(data_value);
            } else if line.starts_with("event:") {
                current_event.event = Some(line.strip_prefix("event:").unwrap_or(line).trim().to_string());
            } else if line.starts_with("id:") {
                current_event.id = Some(line.strip_prefix("id:").unwrap_or(line).trim().to_string());
            } else if line.starts_with("retry:") {
                current_event.retry = line
                    .strip_prefix("retry:")
                    .unwrap_or(line)
                    .trim()
                    .parse()
                    .ok();
            }
        }

        if !current_event.data.is_empty() {
            events.push(current_event);
        }

        events
    }

    pub fn format(&self) -> String {
        let mut result = String::new();

        if let Some(ref id) = self.id {
            result.push_str(&format!("id: {}\n", id));
        }

        if let Some(ref event) = self.event {
            result.push_str(&format!("event: {}\n", event));
        }

        if let Some(retry) = self.retry {
            result.push_str(&format!("retry: {}\n", retry));
        }

        for line in self.data.lines() {
            result.push_str(&format!("data: {}\n", line));
        }

        result.push('\n');
        result
    }
}

pub fn parse_sse_content(content: &[u8]) -> Result<Vec<SseEvent>, String> {
    let text = String::from_utf8(content.to_vec())
        .map_err(|e| format!("Invalid UTF-8: {}", e))?;
    Ok(SseEvent::parse(&text))
}

pub fn format_sse_content(events: &[SseEvent]) -> Vec<u8> {
    events.iter().map(|e| e.format()).collect::<String>().into_bytes()
}

pub fn compare_sse_events(
    expected: &[SseEvent],
    actual: &[SseEvent],
    rules: &HashMap<String, RuleList>,
) -> Vec<crate::proto::ContentMismatch> {
    let mut mismatches = Vec::new();

    if expected.len() != actual.len() {
        mismatches.push(crate::proto::ContentMismatch {
            expected: Some(format!("{} events", expected.len()).into_bytes()),
            actual: Some(format!("{} events", actual.len()).into_bytes()),
            mismatch: format!(
                "Expected {} events, but got {}",
                expected.len(),
                actual.len()
            ),
            path: "eventCount".to_string(),
            diff: String::new(),
            mismatch_type: "body".to_string(),
        });
    }

    let min_len = expected.len().min(actual.len());

    for (i, (exp, act)) in expected.iter().zip(actual.iter()).take(min_len).enumerate() {
        let event_path = format!("events[{}]", i);

        if let (Some(ref exp_event), Some(ref act_event)) = (&exp.event, &act.event) {
            if exp_event != act_event {
                mismatches.push(crate::proto::ContentMismatch {
                    expected: Some(exp_event.as_bytes().to_vec()),
                    actual: Some(act_event.as_bytes().to_vec()),
                    mismatch: format!("Event type mismatch at index {}", i),
                    path: format!("{}.event", event_path),
                    diff: String::new(),
                    mismatch_type: "body".to_string(),
                });
            }
        } else if exp.event != act.event {
            mismatches.push(crate::proto::ContentMismatch {
                expected: exp.event.clone().map(|s| s.into_bytes()),
                actual: act.event.clone().map(|s| s.into_bytes()),
                mismatch: format!("Event type mismatch at index {}", i),
                path: format!("{}.event", event_path),
                diff: String::new(),
                mismatch_type: "body".to_string(),
            });
        }

        if let (Some(ref exp_id), Some(ref act_id)) = (&exp.id, &act.id) {
            if let Some(rule_list) = rules.get(&format!("{}.id", event_path)) {
                for rule in &rule_list.rules {
                    if let Err(err) = exp_id.matches_with(act_id, rule, false) {
                        mismatches.push(crate::proto::ContentMismatch {
                            expected: Some(exp_id.as_bytes().to_vec()),
                            actual: Some(act_id.as_bytes().to_vec()),
                            mismatch: err.to_string(),
                            path: format!("{}.id", event_path),
                            diff: String::new(),
                            mismatch_type: "body".to_string(),
                        });
                    }
                }
            } else if exp_id != act_id {
                mismatches.push(crate::proto::ContentMismatch {
                    expected: Some(exp_id.as_bytes().to_vec()),
                    actual: Some(act_id.as_bytes().to_vec()),
                    mismatch: format!("Event ID mismatch at index {}", i),
                    path: format!("{}.id", event_path),
                    diff: String::new(),
                    mismatch_type: "body".to_string(),
                });
            }
        } else if exp.id != act.id {
            mismatches.push(crate::proto::ContentMismatch {
                expected: exp.id.clone().map(|s| s.into_bytes()),
                actual: act.id.clone().map(|s| s.into_bytes()),
                mismatch: format!("Event ID mismatch at index {}", i),
                path: format!("{}.id", event_path),
                diff: String::new(),
                mismatch_type: "body".to_string(),
            });
        }

        if let Some(rule_list) = rules.get(&format!("{}.data", event_path)) {
            for rule in &rule_list.rules {
                if let Err(err) = exp.data.matches_with(&act.data, rule, false) {
                    mismatches.push(crate::proto::ContentMismatch {
                        expected: Some(exp.data.as_bytes().to_vec()),
                        actual: Some(act.data.as_bytes().to_vec()),
                        mismatch: err.to_string(),
                        path: format!("{}.data", event_path),
                        diff: String::new(),
                        mismatch_type: "body".to_string(),
                    });
                }
            }
        } else if exp.data != act.data {
            mismatches.push(crate::proto::ContentMismatch {
                expected: Some(exp.data.as_bytes().to_vec()),
                actual: Some(act.data.as_bytes().to_vec()),
                mismatch: format!("Event data mismatch at index {}", i),
                path: format!("{}.data", event_path),
                diff: String::new(),
                mismatch_type: "body".to_string(),
            });
        }
    }

    mismatches
}
