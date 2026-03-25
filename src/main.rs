mod proto;
mod sse_content;

use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;

use env_logger::Env;
use futures::Stream;
use log::{debug, info};
use maplit::hashmap;
use pact_models::matchingrules::{RuleList, RuleLogic};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tonic::{Response, Status, transport::Server};
use uuid::Uuid;

use crate::proto::body::ContentTypeHint;
use crate::proto::catalogue_entry::EntryType;
use crate::proto::pact_plugin_server::{PactPlugin, PactPluginServer};
use crate::sse_content::{SseEvent, compare_sse_events, parse_sse_content, format_sse_content};

type MockServerMap = Arc<Mutex<HashMap<String, MockServer>>>;

#[derive(Debug)]
struct MockServer {
    port: u32,
    address: String,
    pact: String,
    results: Vec<proto::MockServerResult>,
}

#[derive(Debug, Default)]
pub struct SsePactPlugin {
    mock_servers: MockServerMap,
}

fn to_object(value: &prost_types::Value) -> Value {
    match &value.kind {
        Some(prost_types::value::Kind::NullValue(_)) => Value::Null,
        Some(prost_types::value::Kind::StringValue(s)) => Value::String(s.clone()),
        Some(prost_types::value::Kind::NumberValue(n)) => serde_json::Number::from_f64(*n).unwrap_or_else(|| serde_json::Number::from(0)).into(),
        Some(prost_types::value::Kind::BoolValue(b)) => Value::Bool(*b),
        Some(prost_types::value::Kind::StructValue(s)) => {
            let map: serde_json::Map<String, Value> = s
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), to_object(v)))
                .collect();
            Value::Object(map)
        }
        Some(prost_types::value::Kind::ListValue(l)) => {
            let arr: Vec<Value> = l.values.iter().map(|v| to_object(v)).collect();
            Value::Array(arr)
        }
        None => Value::Null,
    }
}

#[tonic::async_trait]
impl PactPlugin for SsePactPlugin {
    async fn init_plugin(
        &self,
        request: tonic::Request<proto::InitPluginRequest>,
    ) -> Result<tonic::Response<proto::InitPluginResponse>, Status> {
        let message = request.get_ref();
        debug!("Init request from {}/{}", message.implementation, message.version);

        Ok(Response::new(proto::InitPluginResponse {
            catalogue: vec![
                proto::CatalogueEntry {
                    r#type: EntryType::Transport as i32,
                    key: "sse".to_string(),
                    values: HashMap::new(),
                },
                proto::CatalogueEntry {
                    r#type: EntryType::ContentMatcher as i32,
                    key: "sse".to_string(),
                    values: hashmap! {
                        "content-types".to_string() => "text/event-stream".to_string()
                    },
                },
                proto::CatalogueEntry {
                    r#type: EntryType::ContentGenerator as i32,
                    key: "sse".to_string(),
                    values: hashmap! {
                        "content-types".to_string() => "text/event-stream".to_string()
                    },
                },
            ],
        }))
    }

    async fn update_catalogue(
        &self,
        _request: tonic::Request<proto::Catalogue>,
    ) -> Result<tonic::Response<()>, Status> {
        debug!("Update catalogue request, ignoring");
        Ok(Response::new(()))
    }

    async fn compare_contents(
        &self,
        request: tonic::Request<proto::CompareContentsRequest>,
    ) -> Result<tonic::Response<proto::CompareContentsResponse>, Status> {
        let request = request.get_ref();
        debug!("compare_contents request");

        match (request.expected.as_ref(), request.actual.as_ref()) {
            (Some(expected), Some(actual)) => {
                let expected_content = expected
                    .content
                    .as_ref()
                    .ok_or_else(|| Status::aborted("No expected content"))?;
                let actual_content = actual
                    .content
                    .as_ref()
                    .ok_or_else(|| Status::aborted("No actual content"))?;

                let expected_events = parse_sse_content(expected_content)
                    .map_err(|e| Status::aborted(format!("Failed to parse expected SSE: {}", e)))?;
                let actual_events = parse_sse_content(actual_content)
                    .map_err(|e| Status::aborted(format!("Failed to parse actual SSE: {}", e)))?;

                let rules: HashMap<String, RuleList> = request
                    .rules
                    .iter()
                    .map(|(key, rules)| {
                        let rule_list = rules.rule.iter().fold(RuleList::empty(RuleLogic::And), |mut list, rule| {
                            if let Some(values) = &rule.values {
                                let obj = proto::to_object(values);
                                if let Value::Object(mut map) = obj {
                                    map.insert("match".to_string(), Value::String(rule.r#type.clone()));
                                    if let Ok(matching_rule) = pact_models::matchingrules::MatchingRule::from_json(&Value::Object(map)) {
                                        list.add_rule(&matching_rule);
                                    }
                                }
                            }
                            list
                        });
                        (key.clone(), rule_list)
                    })
                    .collect();

                let mismatches = compare_sse_events(&expected_events, &actual_events, &rules);

                Ok(Response::new(proto::CompareContentsResponse {
                    error: String::default(),
                    type_mismatch: None,
                    results: hashmap! {
                        String::default() => proto::ContentMismatches { mismatches }
                    },
                }))
            }
            (None, Some(actual)) => {
                let contents = actual.content.as_ref().unwrap();
                Ok(Response::new(proto::CompareContentsResponse {
                    error: String::default(),
                    type_mismatch: None,
                    results: hashmap! {
                        String::default() => proto::ContentMismatches {
                            mismatches: vec![proto::ContentMismatch {
                                expected: None,
                                actual: Some(contents.clone()),
                                mismatch: format!("Expected no SSE content, but got {} bytes", contents.len()),
                                path: "".to_string(),
                                diff: "".to_string(),
                                mismatch_type: "body".to_string(),
                            }],
                        }
                    },
                }))
            }
            (Some(expected), None) => {
                let contents = expected.content.as_ref().unwrap();
                Ok(Response::new(proto::CompareContentsResponse {
                    error: String::default(),
                    type_mismatch: None,
                    results: hashmap! {
                        String::default() => proto::ContentMismatches {
                            mismatches: vec![proto::ContentMismatch {
                                expected: Some(contents.clone()),
                                actual: None,
                                mismatch: "Expected SSE content, but did not get any".to_string(),
                                path: "".to_string(),
                                diff: "".to_string(),
                                mismatch_type: "body".to_string(),
                            }],
                        }
                    },
                }))
            }
            (None, None) => Ok(Response::new(proto::CompareContentsResponse {
                error: String::default(),
                type_mismatch: None,
                results: hashmap! {},
            })),
        }
    }

    async fn configure_interaction(
        &self,
        request: tonic::Request<proto::ConfigureInteractionRequest>,
    ) -> Result<tonic::Response<proto::ConfigureInteractionResponse>, Status> {
        debug!("configure_interaction request for '{}'", request.get_ref().content_type);

        let contents_config = request.get_ref().contents_config.as_ref().unwrap();
        let config_map = proto::to_object(contents_config);

        let mut interactions = Vec::new();
        let plugin_config = proto::PluginConfiguration::default();

        if let Value::Object(map) = config_map {
            if let Some(response) = map.get("response") {
                if let Value::Object(resp_map) = response {
                    if let Some(Value::String(data)) = resp_map.get("data") {
                        let events = SseEvent::parse(data);
                        let sse_content = format_sse_content(&events);

                        interactions.push(proto::InteractionResponse {
                            contents: Some(proto::Body {
                                content_type: "text/event-stream".to_string(),
                                content: Some(sse_content.into()),
                                content_type_hint: ContentTypeHint::Default as i32,
                            }),
                            rules: HashMap::new(),
                            generators: HashMap::new(),
                            message_metadata: None,
                            plugin_configuration: None,
                            interaction_markup: String::new(),
                            interaction_markup_type: proto::interaction_response::MarkupType::CommonMark as i32,
                            part_name: "response".to_string(),
                            metadata_rules: HashMap::new(),
                            metadata_generators: HashMap::new(),
                        });
                    }
                }
            }
        }

        Ok(Response::new(proto::ConfigureInteractionResponse {
            error: String::default(),
            interaction: interactions,
            plugin_configuration: Some(plugin_config),
        }))
    }

    async fn generate_content(
        &self,
        request: tonic::Request<proto::GenerateContentRequest>,
    ) -> Result<tonic::Response<proto::GenerateContentResponse>, Status> {
        debug!("generate_content request");

        let contents = request.get_ref().contents.as_ref().unwrap();
        let content_bytes = contents.content.as_ref().unwrap();
        let content_str = String::from_utf8_lossy(content_bytes);

        let events = SseEvent::parse(&content_str);
        let generated = format_sse_content(&events);

        Ok(Response::new(proto::GenerateContentResponse {
            contents: Some(proto::Body {
                content_type: "text/event-stream".to_string(),
                content: Some(generated.into()),
                content_type_hint: ContentTypeHint::Default as i32,
            }),
        }))
    }

    async fn start_mock_server(
        &self,
        request: tonic::Request<proto::StartMockServerRequest>,
    ) -> Result<tonic::Response<proto::StartMockServerResponse>, Status> {
        let req = request.get_ref();
        let pact = req.pact.clone();
        let host_interface = req.host_interface.clone();
        let port = if req.port == 0 { 0 } else { req.port };
        info!("start_mock_server request: port={}, pact={}", port, &pact[..pact.len().min(100)]);

        let server_key = Uuid::new_v4().to_string();
        let server_key_for_response = server_key.clone();

        let addr = format!("{}:{}", host_interface, port)
            .parse::<SocketAddr>()
            .map_err(|e| Status::aborted(format!("Invalid address: {}", e)))?;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| Status::aborted(format!("Failed to bind: {}", e)))?;
        let local_addr = listener.local_addr().map_err(|e| Status::aborted(format!("Failed to get local addr: {}", e)))?;

        let mock_server = MockServer {
            port: local_addr.port() as u32,
            address: local_addr.ip().to_string(),
            pact: pact.clone(),
            results: Vec::new(),
        };

        let server_key_for_map = server_key.clone();
        self.mock_servers
            .lock()
            .await
            .insert(server_key_for_map, mock_server);

        tokio::spawn(async move {
            run_sse_mock_server(listener, server_key, pact).await;
        });

        Ok(Response::new(proto::StartMockServerResponse {
            response: Some(proto::start_mock_server_response::Response::Details(
                proto::MockServerDetails {
                    key: server_key_for_response,
                    port: local_addr.port() as u32,
                    address: local_addr.ip().to_string(),
                },
            )),
        }))
    }

    async fn shutdown_mock_server(
        &self,
        request: tonic::Request<proto::ShutdownMockServerRequest>,
    ) -> Result<tonic::Response<proto::ShutdownMockServerResponse>, Status> {
        let server_key = request.get_ref().server_key.clone();
        debug!("shutdown_mock_server request for key: {}", server_key);

        let servers = self.mock_servers.lock().await;
        if let Some(server) = servers.get(&server_key) {
            Ok(Response::new(proto::ShutdownMockServerResponse {
                ok: server.results.is_empty(),
                results: server.results.clone(),
            }))
        } else {
            Ok(Response::new(proto::ShutdownMockServerResponse {
                ok: true,
                results: Vec::new(),
            }))
        }
    }

    async fn get_mock_server_results(
        &self,
        request: tonic::Request<proto::MockServerRequest>,
    ) -> Result<tonic::Response<proto::MockServerResults>, Status> {
        let server_key = request.get_ref().server_key.clone();
        debug!("get_mock_server_results request for key: {}", server_key);

        let servers = self.mock_servers.lock().await;
        if let Some(server) = servers.get(&server_key) {
            Ok(Response::new(proto::MockServerResults {
                ok: server.results.is_empty(),
                results: server.results.clone(),
            }))
        } else {
            Ok(Response::new(proto::MockServerResults {
                ok: true,
                results: Vec::new(),
            }))
        }
    }

    async fn prepare_interaction_for_verification(
        &self,
        _request: tonic::Request<proto::VerificationPreparationRequest>,
    ) -> Result<tonic::Response<proto::VerificationPreparationResponse>, Status> {
        Ok(Response::new(proto::VerificationPreparationResponse {
            response: Some(proto::verification_preparation_response::Response::InteractionData(
                proto::InteractionData {
                    body: Some(proto::Body {
                        content_type: "text/event-stream".to_string(),
                        content: None,
                        content_type_hint: ContentTypeHint::Default as i32,
                    }),
                    metadata: HashMap::new(),
                },
            )),
        }))
    }

    async fn verify_interaction(
        &self,
        _request: tonic::Request<proto::VerifyInteractionRequest>,
    ) -> Result<tonic::Response<proto::VerifyInteractionResponse>, Status> {
        todo!("verify_interaction not yet implemented")
    }
}

async fn run_sse_mock_server(listener: TcpListener, server_key: String, pact: String) {
    info!("SSE mock server started, handling connections");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let server_key = server_key.clone();
                let pact = pact.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_sse_connection(stream, server_key, &pact).await {
                        eprintln!("Error handling SSE connection: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Failed to accept connection: {}", e);
                break;
            }
        }
    }
}

async fn handle_sse_connection(stream: TcpStream, _server_key: String, _pact: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (read_half, mut write_half) = stream.into_split();

    let response_headers = "HTTP/1.1 200 OK\r\n\
        Content-Type: text/event-stream\r\n\
        Cache-Control: no-cache\r\n\
        Connection: keep-alive\r\n\
        \r\n";

    write_half.write_all(response_headers.as_bytes()).await?;
    write_half.flush().await?;

    let default_event = SseEvent {
        event: Some("message".to_string()),
        id: None,
        data: "Hello from SSE mock server!".to_string(),
        retry: Some(3000),
    };

    write_half.write_all(default_event.format().as_bytes()).await?;
    write_half.flush().await?;

    let _ = read_half;

    Ok(())
}

struct TcpIncoming {
    inner: TcpListener,
}

impl Stream for TcpIncoming {
    type Item = Result<TcpStream, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner)
            .poll_accept(cx)
            .map_ok(|(stream, _)| stream)
            .map(|v| Some(v))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env = Env::new().filter("LOG_LEVEL");
    env_logger::init_from_env(env);

    let addr: SocketAddr = "0.0.0.0:0".parse()?;
    let listener = TcpListener::bind(addr).await?;
    let address = listener.local_addr()?;

    let server_key = Uuid::new_v4().to_string();
    println!(r#"{{"port":{}, "serverKey":"{}"}}"#, address.port(), server_key);
    let _ = std::io::stdout().flush();

    let plugin = SsePactPlugin {
        mock_servers: Arc::new(Mutex::new(HashMap::new())),
    };

    Server::builder()
        .add_service(PactPluginServer::new(plugin))
        .serve_with_incoming(TcpIncoming { inner: listener })
        .await?;

    Ok(())
}
