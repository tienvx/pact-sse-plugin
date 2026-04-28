# SSE Examples

These examples demonstrate using the SSE plugin to support matching requests and responses with Server-Sent Events content.

## Running the consumer tests

Before running the consumer tests, the SSE plugin needs to be built and installed:

```bash
cd ../..
cargo build --release
mkdir -p ~/.pact/plugins/sse-0.1.0
cp target/release/pact-sse-plugin ~/.pact/plugins/sse-0.1.0/
cp pact-plugin.json ~/.pact/plugins/sse-0.1.0/
```

Then run the consumer tests:

```bash
cd examples/sse-consumer-rust
cargo test
```

## Running the provider

```bash
cd examples/sse-provider
cargo run
```

The provider will start on `127.0.0.1:8080` with the following endpoints:
- `GET /events` - Returns SSE events with various types
- `GET /events/with-headers` - Returns SSE events with custom headers

## SSE Event Format

The provider returns events in the standard SSE format:

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

Each event is separated by a blank line and can have:
- `id:` - Event ID
- `event:` - Event type name
- `data:` - Event data payload
- `retry:` - Retry interval in milliseconds
