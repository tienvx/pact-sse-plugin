# SSE Plugin for Pact

A Pact plugin for creating and matching Server-Sent Events (SSE) payloads.

## Building the plugin

```bash
cargo build --release
```

This creates the binary `pact-sse-plugin` in `target/release/`.

## Installing the plugin

Copy the binary and `pact-plugin.json` manifest to `$HOME/.pact/plugins/sse-0.1.0/`:

```bash
mkdir -p ~/.pact/plugins/sse-0.1.0
cp target/release/pact-sse-plugin ~/.pact/plugins/sse-0.1.0/
cp pact-plugin.json ~/.pact/plugins/sse-0.1.0/
```

## SSE Matching Definitions

The plugin matches SSE events using matching rule definitions. The fields can be specified by:

### Event Fields

- `event` - Match the event type value
- `retry` - Match the retry field value

### Data Fields

- `data[type]` - Match data for events of a specific type (e.g., `data[count]`)
- `data[*]` - Match data for events without a type
- `data.type.*` - Alternative dot notation for `data[type]`

### ID Fields

- `id[type]` - Match id for events of a specific type (e.g., `id[user]`)
- `id[*]` - Match id for events without a type
- `id.type.*` - Alternative dot notation for `id[type]`

## Example Usage

### PHP Consumer Test

```php
$matcher = new Matcher(plugin: true);

$response->setBody(new Text(
    json_encode([
        'id[*]' => $matcher->number(100),
        'id[user][*]' => $matcher->uuid('5d03dc45-96f6-4c0c-b1ad-aa67242058cc'),
        'retry' => $matcher->integer(100),
        'event' => $matcher->type('count'),
        'data[*]' => $matcher->type('simple text'),
        'data[count]' => $matcher->number(100),
        'data[time]' => $matcher->datetime('yyyy-MM-dd', '2000-01-01'),
    ]),
    'text/event-stream'
));
```

### Rust Consumer Test

```rust
builder.interaction("request for SSE events", "", |mut i| async move {
    i.request.path("/events");
    i.response
        .ok()
        .contents(ContentType::from("text/event-stream"), json!({
            "id[*]": "matching(number, 100)",
            "data[*]": "matching(type, 'simple text')",
            "data[count]": "matching(number, 42)",
            "data[time]": "matching(datetime, 'yyyy-MM-dd', '2024-01-15')"
        })).await;
    i
}).await;
```

### SSE Format

```
id:100
data:simple text

event:count
data:42

event:time
data:2024-01-15

id:5d03dc45-96f6-4c0c-b1ad-aa67242058cc
event:user
data:user payload
```

## Matching Rules Explanation

1. `data.count.*` or `data[count]` - All data fields of events with type `count` should match the specified rule
2. `data.*` or `data[*]` - All data fields of events without a type should match the specified rule
3. `id.user.*` or `id[user]` - All id fields of events with type `user` should match the specified rule
4. `id.*` or `id[*]` - All id fields of events without a type should match the specified rule
5. `event` - The event type field should match the specified rule
6. `retry` - The retry field should match the specified rule

## Examples

See the `examples/` directory for:
- `sse-consumer-rust/` - Rust consumer test example
- `sse-provider/` - Rust SSE provider example
