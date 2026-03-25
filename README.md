# Pact SSE Plugin

A Pact plugin for Server-Sent Events (SSE) protocol support.

## Features

- **SSE Transport Protocol**: Full SSE transport implementation with mock server and provider verifier
- **Content Matcher**: Content-type matching for `text/event-stream`
- **Content Generator**: SSE event generation with support for multi-line data fields

## SSE Format Support

The plugin supports the standard SSE format as defined in the [WHATWG specification](https://html.spec.whatwg.org/multipage/server-sent-events.html):

```
id: <event-id>
event: <event-type>
retry: <timeout-in-ms>
data: <event-data>
```

## Building

```bash
cargo build --release
```

The binary will be available at `target/release/pact-sse-plugin`.

## Installation

1. Build the plugin:
   ```bash
   cargo build --release
   ```

2. Copy the binary and manifest to the Pact plugins directory:
   ```bash
   mkdir -p ~/.pact/plugins/sse-0.1.0
   cp target/release/pact-sse-plugin ~/.pact/plugins/sse-0.1.0/
   cp pact-plugin.json ~/.pact/plugins/sse-0.1.0/
   ```

## Usage

### Consumer Test Example

```rust
use pact_consumer::{Consumer, Pact, TestServer};

#[tokio::test]
async fn test_sse_interaction() {
    let pact = Pact::new("SSE Consumer", "SSE Provider")
        .given("there are new messages")
        .upon_receiving("a request for SSE stream")
        .for_path("/events")
        .with_method("GET")
        .with_header("Accept", "text/event-stream")
        .will_respond_with(200)
        .with_header("Content-Type", "text/event-stream")
        .with_body(
            "data: {\"message\": \"Hello World\"}\n\n",
            "text/event-stream"
        );

    let mock_server = pact.mock_server().await;

    // Use an SSE client to connect to the mock server
    // and verify the events received
}
```

### SSE Event Configuration

The plugin supports configuring SSE events in the interaction:

```json
{
  "response": {
    "data": "id: 123\nevent: message\ndata: {\"text\": \"Hello\"}\n\n"
  }
}
```

## Matching Rules

The plugin supports Pact matching rules on SSE event fields:

- `events[0].data` - Match the data of the first event
- `events[0].event` - Match the event type
- `events[0].id` - Match the event ID
- `eventCount` - Match the number of events

Example:
```rust
.with_body(
    "data: {\"count\": 42}\n\n",
    "text/event-stream",
    hashmap! {
        "events[0].data".to_string() => vec![
            json!({ "match" => "regex", "regex" => r#"{"count": \d+}"# })
        ]
    }
)
```

## License

MIT
