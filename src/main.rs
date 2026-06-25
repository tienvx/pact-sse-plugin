use core::pin::Pin;
use core::task::{Context, Poll};
use std::io;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::Stream;
use maplit::hashmap;
use pact_models::matchingrules::{MatchingRule, RuleList, RuleLogic};
use pact_models::prelude::ContentType;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tonic::{transport::Server, Response};
use tracing::{debug, info, warn, Level};
use uuid::Uuid;

use crate::proto::body::ContentTypeHint;
use crate::proto::catalogue_entry::EntryType;
use crate::proto::init_plugin_response::Response as InitResponse;
use crate::proto::pact_plugin_server::{PactPlugin, PactPluginServer};
use crate::proto::start_mock_server_response::Response as MockServerResponse;
use crate::proto::verification_preparation_response::Response as PrepResponse;
use crate::proto::verify_interaction_response::Response as VerifyResponse;
use crate::proto::plugin_host_client::PluginHostClient;
use crate::sse_content::{compare_sse_contents, generate_sse_content, setup_sse_contents};

mod parser;
#[allow(dead_code)]
mod proto;
mod sse_content;
#[allow(dead_code)]
mod utils;

/// Logger name prefixes that are gRPC transport internals and should not be forwarded via Log RPC.
const TRANSPORT_TARGET_PREFIXES: &[&str] = &[
    "h2::",
    "tower::",
    "tonic::",
    "hyper_util::",
    "hyper::",
];

fn is_transport_target(target: &str) -> bool {
    TRANSPORT_TARGET_PREFIXES.iter().any(|p| target.starts_with(p))
}

struct LogForwarder {
    client: Mutex<Option<PluginHostClient<tonic::transport::Channel>>>,
    plugin_instance_id: String,
}

impl LogForwarder {
    async fn send_log(
        &self,
        level: &str,
        message: &str,
        target: &str,
        test_run_id: &str,
        timestamp_ms: i64,
    ) {
        let mut client = self.client.lock().await;
        if let Some(ref mut client) = *client {
            let msg = crate::proto::LogMessage {
                plugin_instance_id: self.plugin_instance_id.clone(),
                test_run_id: test_run_id.to_string(),
                level: level.to_string(),
                message: message.to_string(),
                target: target.to_string(),
                timestamp_ms,
            };
            if let Err(e) = client
                .log(tonic::Request::new(msg))
                .await
            {
                eprintln!("[log-forwarder] Failed to send log via RPC: {}", e);
            }
        }
    }
}

use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

struct TracingLogLayer {
    forwarder: Arc<LogForwarder>,
}

impl<S> tracing_subscriber::layer::Layer<S> for TracingLogLayer
where
    S: tracing::Subscriber,
{
    fn on_record(
        &self,
        _span: &tracing::Id,
        _values: &tracing::span::Record<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
    }

    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let level = event.metadata().level();
        let target = event.metadata().target();

        if *level == Level::TRACE || is_transport_target(target) {
            return;
        }

        let level_str = match *level {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN",
            Level::INFO => "INFO",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };

        let mut visitor = StringVisitor(String::new());
        event.record(&mut visitor);
        let message = visitor.0;

        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let forwarder = self.forwarder.clone();
        let _plugin_instance_id = forwarder.plugin_instance_id.clone();

        tokio::task::spawn(async move {
            forwarder
                .send_log(level_str, &message, target, "", timestamp_ms)
                .await;
        });
    }
}

struct StringVisitor(String);

impl<'a> tracing::field::Visit for StringVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if self.0.is_empty() {
            self.0 = value.to_owned();
        } else {
            self.0.push_str(&format!(" {}={} ", field.name(), value));
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if self.0.is_empty() {
            self.0 = format!("{}", field.name());
        } else {
            self.0.push_str(&format!(" {}={:?} ", field.name(), value));
        }
    }
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
            .map(Some)
    }
}

#[derive(Debug, Default)]
pub struct SsePactPlugin {}

#[tonic::async_trait]
impl PactPlugin for SsePactPlugin {
    async fn init_plugin(
        &self,
        request: tonic::Request<proto::InitPluginRequest>,
    ) -> Result<tonic::Response<proto::InitPluginResponse>, tonic::Status> {
        let message = request.get_ref();
        debug!(
            implementation = %message.implementation,
            version = %message.version,
            plugin_instance_id = %message.plugin_instance_id,
            host_capabilities = ?message.host_capabilities,
            "Init request received"
        );
        Ok(Response::new(proto::InitPluginResponse {
            response: Some(InitResponse::Success(proto::InitPluginSuccess {
                catalogue: vec![
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
                plugin_capabilities: vec![],
            })),
        }))
    }

    async fn update_catalogue(
        &self,
        _request: tonic::Request<proto::Catalogue>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        debug!("Update catalogue request, ignoring");
        Ok(Response::new(()))
    }

    async fn compare_contents(
        &self,
        request: tonic::Request<proto::CompareContentsRequest>,
    ) -> Result<tonic::Response<proto::CompareContentsResponse>, tonic::Status> {
        let request = request.get_ref();
        debug!("compare_contents request");

        let rules = request
            .rules
            .iter()
            .map(|(key, rules)| {
                let rules =
                    rules
                        .rule
                        .iter()
                        .fold(RuleList::empty(RuleLogic::And), |mut list, rule| {
                            if let Value::Object(mut map) =
                                crate::proto::to_object(rule.values.as_ref().unwrap())
                            {
                                map.insert("match".to_string(), Value::String(rule.r#type.clone()));
                                debug!(rule = ?map, "Creating matching rule");
                                list.add_rule(
                                    &MatchingRule::from_json(&Value::Object(map)).unwrap(),
                                );
                            }
                            list
                        });
                (key.clone(), rules)
            })
            .collect();

        match (request.expected.as_ref(), request.actual.as_ref()) {
            (Some(expected), Some(actual)) => {
                let expected_sse = std::str::from_utf8(expected.content.as_ref().unwrap())
                    .map_err(|err| {
                        tonic::Status::aborted(format!("Failed to parse expected SSE: {}", err))
                    })?;
                let actual_sse =
                    std::str::from_utf8(actual.content.as_ref().unwrap()).map_err(|err| {
                        tonic::Status::aborted(format!("Failed to parse actual SSE: {}", err))
                    })?;
                compare_sse_contents(
                    expected_sse,
                    actual_sse,
                    request.allow_unexpected_keys,
                    &rules,
                )
                .map_err(|err| {
                    tonic::Status::aborted(format!("Failed to compare SSE contents: {}", err))
                })
            }
            (None, Some(actual)) => {
                let contents = actual.content.as_ref().unwrap();
                Ok(Response::new(proto::CompareContentsResponse {
                    error: String::default(),
                    type_mismatch: None,
                    results: hashmap! {
                      String::default() => proto::ContentMismatches {
                        mismatches: vec![
                          proto::ContentMismatch {
                            expected: None,
                            actual: Some(contents.clone()),
                            mismatch: format!("Expected no SSE content, but got {} bytes", contents.len()),
                            path: "".to_string(),
                            diff: "".to_string(),
                            mismatch_type: "body".to_string(),
                          }
                        ]
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
                        mismatches: vec![
                          proto::ContentMismatch {
                            expected: Some(contents.clone()),
                            actual: None,
                            mismatch: "Expected SSE content, but did not get any".to_string(),
                            path: "".to_string(),
                            diff: "".to_string(),
                            mismatch_type: "body".to_string(),
                          }
                        ]
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
    ) -> Result<tonic::Response<proto::ConfigureInteractionResponse>, tonic::Status> {
        debug!(
            content_type = %request.get_ref().content_type,
            "Received configure_contents request"
        );
        setup_sse_contents(&request)
            .map_err(|err| tonic::Status::aborted(format!("Invalid SSE definition: {}", err)))
    }

    async fn generate_content(
        &self,
        request: tonic::Request<proto::GenerateContentRequest>,
    ) -> Result<tonic::Response<proto::GenerateContentResponse>, tonic::Status> {
        debug!("Received generate_content request");
        generate_sse_content(&request)
            .map(|contents| {
                debug!(bytes = contents.value().map(|v| v.len()).unwrap_or(0), "Generated contents");
                Response::new(proto::GenerateContentResponse {
                    contents: Some(proto::Body {
                        content_type: contents
                            .content_type()
                            .unwrap_or(ContentType::from("text/event-stream"))
                            .to_string(),
                        content: Some(contents.value().unwrap().to_vec()),
                        content_type_hint: ContentTypeHint::Default as i32,
                    }),
                })
            })
            .map_err(|err| {
                tonic::Status::aborted(format!("Failed to generate SSE contents: {}", err))
            })
    }

    async fn start_mock_server(
        &self,
        _request: tonic::Request<proto::StartMockServerRequest>,
    ) -> Result<tonic::Response<proto::StartMockServerResponse>, tonic::Status> {
        Ok(Response::new(proto::StartMockServerResponse {
            response: Some(MockServerResponse::Error(
                "Mock server not implemented for SSE plugin".to_string(),
            )),
        }))
    }

    async fn shutdown_mock_server(
        &self,
        _request: tonic::Request<proto::MockServerRequest>,
    ) -> Result<tonic::Response<proto::MockServerResults>, tonic::Status> {
        Ok(Response::new(proto::MockServerResults {
            ok: true,
            results: vec![],
        }))
    }

    async fn get_mock_server_results(
        &self,
        _request: tonic::Request<proto::MockServerRequest>,
    ) -> Result<tonic::Response<proto::MockServerResults>, tonic::Status> {
        Ok(Response::new(proto::MockServerResults {
            ok: true,
            results: vec![],
        }))
    }

    async fn prepare_interaction_for_verification(
        &self,
        request: tonic::Request<proto::VerificationPreparationRequest>,
    ) -> Result<tonic::Response<proto::VerificationPreparationResponse>, tonic::Status> {
        let req = request.get_ref();
        debug!(
            interaction_type = ?req.interaction_contents
                .as_ref()
                .map(|ic| &ic.interaction_type),
            "Prepare interaction for verification"
        );

        Ok(Response::new(proto::VerificationPreparationResponse {
            response: Some(PrepResponse::InteractionData(proto::InteractionData {
                body: Some(proto::Body {
                    content_type: "text/event-stream".to_string(),
                    content: None,
                    content_type_hint: ContentTypeHint::Default as i32,
                }),
                metadata: hashmap! {},
            })),
        }))
    }

    async fn verify_interaction(
        &self,
        request: tonic::Request<proto::VerifyInteractionRequest>,
    ) -> Result<tonic::Response<proto::VerifyInteractionResponse>, tonic::Status> {
        let req = request.get_ref();
        debug!("Verify interaction request");

        let interaction_data = req.interaction_data.as_ref();
        let interaction_contents = req.interaction_contents.as_ref();

        let plugin_config = interaction_contents
            .and_then(|ic| ic.plugin_configuration.as_ref());

        let rules = if let Some(config) = plugin_config {
            if let Some(ref interaction_config) = config.interaction_configuration {
                let mut rule_map = std::collections::HashMap::new();
                for (key, value) in &interaction_config.fields {
                    if key.ends_with("rules") {
                        if let Some(list_value) = &value.kind {
                            if let prost_types::value::Kind::ListValue(lv) = list_value {
                                for rule_val in &lv.values {
                                    if let Some(prost_types::value::Kind::StructValue(sv)) =
                                        &rule_val.kind
                                    {
                                        let rule_type = sv
                                            .fields
                                            .get("type")
                                            .and_then(|v| {
                                                if let Some(
                                                    prost_types::value::Kind::StringValue(s),
                                                ) = &v.kind
                                                {
                                                    Some(s.clone())
                                                } else {
                                                    None
                                                }
                                            })
                                            .unwrap_or_default();
                                        let mut map = serde_json::Map::new();
                                        for (k, v) in &sv.fields {
                                            map.insert(
                                                k.clone(),
                                                crate::utils::from_value(v),
                                            );
                                        }
                                        map.insert(
                                            "match".to_string(),
                                            Value::String(rule_type.clone()),
                                        );
                                        if let Ok(matching_rule) =
                                            MatchingRule::from_json(&Value::Object(map))
                                        {
                                            let path = key.trim_end_matches("rules");
                                            let entry = rule_map
                                                .entry(path.to_string())
                                                .or_insert_with(|| {
                                                    RuleList::empty(RuleLogic::And)
                                                });
                                            entry.add_rule(&matching_rule);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                rule_map
            } else {
                std::collections::HashMap::new()
            }
        } else {
            std::collections::HashMap::new()
        };

        let expected_body = interaction_data
            .and_then(|id| id.body.as_ref())
            .and_then(|b| b.content.as_ref());

        let actual_body = req
            .interaction_data
            .as_ref()
            .and_then(|id| id.body.as_ref())
            .and_then(|b| b.content.as_ref());

        let mismatches = if let (Some(expected_bytes), Some(actual_bytes)) =
            (expected_body, actual_body)
        {
            let expected_sse = std::str::from_utf8(expected_bytes)
                .unwrap_or_else(|_| "error parsing expected SSE");
            let actual_sse = std::str::from_utf8(actual_bytes)
                .unwrap_or_else(|_| "error parsing actual SSE");

            match compare_sse_contents(expected_sse, actual_sse, true, &rules) {
                Ok(resp) => {
                    let mut items = Vec::new();
                    for (_, path_mismatches) in resp.into_inner().results {
                        for mm in path_mismatches.mismatches {
                            items.push(proto::VerificationResultItem {
                                result: Some(
                                    proto::verification_result_item::Result::Mismatch(mm),
                                ),
                            });
                        }
                    }
                    items
                }
                Err(e) => {
                    vec![proto::VerificationResultItem {
                        result: Some(proto::verification_result_item::Result::Error(
                            e.to_string(),
                        )),
                    }]
                }
            }
        } else if expected_body.is_some() && actual_body.is_none() {
            vec![proto::VerificationResultItem {
                result: Some(proto::verification_result_item::Result::Mismatch(
                    proto::ContentMismatch {
                        expected: expected_body.cloned(),
                        actual: None,
                        mismatch: "Expected SSE content, but did not get any".to_string(),
                        path: "".to_string(),
                        diff: "".to_string(),
                        mismatch_type: "body".to_string(),
                    },
                )),
            }]
        } else {
            vec![]
        };

        Ok(Response::new(proto::VerifyInteractionResponse {
            response: Some(VerifyResponse::Result(proto::VerificationResult {
                success: mismatches.is_empty(),
                response_data: None,
                mismatches,
                output: vec![],
            })),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plugin_instance_id =
        std::env::var("PACT_PLUGIN_INSTANCE_ID").unwrap_or_else(|_| Uuid::new_v4().to_string());

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_target(true)
        .with_file(true)
        .with_line_number(true);

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| {
            tracing_subscriber::EnvFilter::try_new(
                "info,pact_sse_plugin=DEBUG,h2=warn,tower=warn,tonic=warn,hyper=warn,hyper_util=warn",
            )
        })
        .unwrap();

    let forwarder = Arc::new(LogForwarder {
        client: Mutex::new(None),
        plugin_instance_id: plugin_instance_id.clone(),
    });

    let log_layer = TracingLogLayer {
        forwarder: forwarder.clone(),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(log_layer)
        .init();

    let addr: SocketAddr = "0.0.0.0:0".parse()?;
    let listener = TcpListener::bind(addr).await?;
    let address = listener.local_addr()?;

    let server_key = Uuid::new_v4().to_string();
    println!(
        "{{\"port\":{}, \"serverKey\":\"{}\"}}",
        address.port(),
        server_key
    );
    let _ = io::stdout().flush();

    info!(
        port = address.port(),
        instance_id = %plugin_instance_id,
        "SSE plugin server listening"
    );

    if let Ok(host_addr) = std::env::var("PACT_PLUGIN_HOST") {
        info!(host = %host_addr, "Connecting to PluginHost for log forwarding");
        match PluginHostClient::connect(format!("http://{}", host_addr)).await {
            Ok(client) => {
                info!("Connected to PluginHost for log forwarding");
                *forwarder.client.lock().await = Some(client);
            }
            Err(e) => {
                warn!(error = %e, "Failed to connect to PluginHost, log forwarding disabled");
            }
        }
    } else {
        debug!("PACT_PLUGIN_HOST not set, log forwarding via RPC disabled");
    }

    let plugin = SsePactPlugin::default();
    Server::builder()
        .add_service(PactPluginServer::new(plugin))
        .serve_with_incoming(TcpIncoming { inner: listener })
        .await?;

    Ok(())
}
