use reqwest::Url;

struct SseClient {
    url: Url,
}

impl SseClient {
    pub fn new<S>(url: S) -> SseClient
    where
        S: Into<Url>,
    {
        SseClient { url: url.into() }
    }

    pub async fn get_events(&self, path: &str) -> anyhow::Result<Vec<SseEvent>> {
        let client = reqwest::Client::new();
        let mut response = client
            .get(self.url.join(path)?)
            .header("Accept", "text/event-stream")
            .send()
            .await?;

        let mut body = String::new();
        while let Some(chunk) = response.chunk().await? {
            body.push_str(&String::from_utf8_lossy(&chunk));
        }

        parse_sse_events(&body)
    }
}

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub id: Option<String>,
    pub event_type: Option<String>,
    pub data: String,
    pub retry: Option<String>,
}

fn parse_sse_events(content: &str) -> anyhow::Result<Vec<SseEvent>> {
    let mut events = Vec::new();
    let mut current = SseEvent {
        id: None,
        event_type: None,
        data: String::new(),
        retry: None,
    };
    let mut has_content = false;

    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if has_content {
                events.push(current);
                current = SseEvent {
                    id: None,
                    event_type: None,
                    data: String::new(),
                    retry: None,
                };
                has_content = false;
            }
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("id:") {
            current.id = Some(value.trim().to_string());
            has_content = true;
        } else if let Some(value) = line.strip_prefix("event:") {
            current.event_type = Some(value.trim().to_string());
            has_content = true;
        } else if let Some(value) = line.strip_prefix("data:") {
            current.data = value.to_string();
            has_content = true;
        } else if let Some(value) = line.strip_prefix("retry:") {
            current.retry = Some(value.trim().to_string());
            has_content = true;
        }
    }

    if has_content {
        events.push(current);
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use crate::{parse_sse_events, SseClient};
    use expectest::prelude::*;
    use pact_consumer::prelude::*;
    use pact_consumer::mock_server::StartMockServerAsync;
    use pact_models::prelude::*;
    use regex::Regex;
    use serde_json::json;

    #[test_log::test(tokio::test(flavor = "multi_thread", worker_threads = 1))]
    async fn test_sse_client_get_events() {
        let mut builder = PactBuilder::new_v4("SseClient", "SseServer")
            .using_plugin("sse", None)
            .await;

        builder
            .interaction("request for SSE events", "", |mut i| async move {
                i.request.path("/events");
                i.response
                    .ok()
                    .header("Cache-Control", "no-cache")
                    .header("Connection", "keep-alive")
                    .header("X-Accel-Buffering", "no")
                    .contents(
                        ContentType::from("text/event-stream"),
                        json!({
                   "id[*]": "matching(number, 100)",
                             "id[user]": "matching(regex, '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', '5d03dc45-96f6-4c0c-b1ad-aa67242058cc')",
                            "retry": "matching(integer, 3000)",
                            "event": "matching(type, 'count')",
                            "data[*]": "matching(type, 'simple text')",
                            "data[count]": "matching(number, 42)",
                            "data[time]": "matching(datetime, 'yyyy-MM-dd', '2024-01-15')"
                        }),
                    )
                    .await;
                i
            })
            .await;

        let mock_server = builder.start_mock_server_async(None, None).await;

        let client = SseClient::new(mock_server.url().clone());
        let events = client.get_events("/events").await.unwrap();

    expect!(events.len()).to(be_equal_to(5));

       // Check that we have the expected event types
       let event_types: Vec<_> = events.iter().map(|e| e.event_type.clone()).collect();
       expect!(event_types.contains(&None)).to(be_true()); // untyped event
       expect!(event_types.contains(&Some("count".to_string()))).to(be_true());
       expect!(event_types.contains(&Some("time".to_string()))).to(be_true());
       expect!(event_types.contains(&Some("user".to_string()))).to(be_true());

       // Check that we have expected data
       let data_values: Vec<_> = events.iter().map(|e| e.data.clone()).collect();
       expect!(data_values.contains(&"simple text".to_string())).to(be_true());
       expect!(data_values.contains(&"42".to_string())).to(be_true());

       // Check date format exists
       let date_re = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
       let has_date = data_values.iter().any(|d| date_re.is_match(d));
       expect!(has_date).to(be_true());

       // Check UUID format exists
       let uuid_re =
           Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
               .unwrap();
       let has_uuid = data_values.iter().any(|d| uuid_re.is_match(d));
       expect!(has_uuid).to(be_true());
   }

    #[test]
    fn test_parse_sse_events_basic() {
        let sse = "id:1\nevent:count\ndata:100\n\nid:2\ndata:hello\n\n";
        let events = parse_sse_events(sse).unwrap();
        expect!(events.len()).to(be_equal_to(2));
        expect!(events[0].event_type.clone()).to(be_some());
       expect!(events[0].event_type.as_ref().unwrap()).to(be_equal_to("count"));
        expect!(&events[0].data).to(be_equal_to("100"));
        expect!(events[1].event_type.as_ref()).to(be_none());
        expect!(&events[1].data).to(be_equal_to("hello"));
    }

    #[test]
    fn test_parse_sse_events_with_retry() {
        let sse = "retry:3000\nevent:ping\ndata:pong\n\n";
        let events = parse_sse_events(sse).unwrap();
        expect!(events.len()).to(be_equal_to(1));
     expect!(events[0].retry.clone()).to(be_some());
       expect!(events[0].retry.as_ref().unwrap()).to(be_equal_to("3000"));
       expect!(events[0].event_type.clone()).to(be_some());
       expect!(events[0].event_type.as_ref().unwrap()).to(be_equal_to("ping"));
    }
}
